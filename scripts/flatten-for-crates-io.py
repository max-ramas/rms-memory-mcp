#!/usr/bin/env python3
"""Flatten workspace path crates into a single-package tree for crates.io.

Local development keeps the workspace (publish = false members). crates.io
only receives the public umbrella `rms-memory-mcp`; Cargo cannot publish path
deps that are not on the registry, so this script builds a staging tree where
core/index/vault sources are modules inside the umbrella package.
"""

from __future__ import annotations

import re
import shutil
import sys
from pathlib import Path

INTERNAL = [
    ("rms-memory-core", "rms_memory_core"),
    ("rms-memory-index", "rms_memory_index"),
    ("rms-memory-vault", "rms_memory_vault"),
]

SKIP_DEP_NAMES = {name for name, _ in INTERNAL}


def parse_deps_block(cargo_toml: str, section: str = "dependencies") -> dict[str, str]:
    """Extract dependency name -> raw TOML value text from a section."""
    pattern = rf"^\[{re.escape(section)}\]\n(.*?)(?=^\[|\Z)"
    match = re.search(pattern, cargo_toml, flags=re.MULTILINE | re.DOTALL)
    if not match:
        return {}
    block = match.group(1)
    deps: dict[str, str] = {}
    for line in block.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            continue
        name, _, value = line.partition("=")
        name = name.strip()
        value = value.strip()
        deps[name] = value
    return deps


def merge_dep_values(existing: str | None, new: str) -> str:
    """Prefer the richer feature set when the same dep appears twice."""
    if existing is None:
        return new
    if "features" in new and "features" not in existing:
        return new
    if "full" in new and "full" not in existing:
        return new
    # Prefer longer / more specific tables.
    if len(new) > len(existing):
        return new
    return existing


def collect_external_deps(root: Path) -> dict[str, str]:
    deps: dict[str, str] = {}
    manifests = [root / "Cargo.toml"] + [
        root / "crates" / pkg / "Cargo.toml" for pkg, _ in INTERNAL
    ]
    for manifest in manifests:
        text = manifest.read_text(encoding="utf-8")
        for name, value in parse_deps_block(text, "dependencies").items():
            if name in SKIP_DEP_NAMES:
                continue
            if "path" in value:
                continue
            deps[name] = merge_dep_values(deps.get(name), value)
    return deps


def rewrite_rs(text: str, module_name: str, sibling_modules: list[str]) -> str:
    """Rewrite crate-local and cross-crate paths for the flattened module tree."""
    # crate::X inside former package -> crate::<module>::X
    text = re.sub(r"\bcrate::", f"crate::{module_name}::", text)
    for sibling in sibling_modules:
        if sibling == module_name:
            continue
        # rms_memory_core::X -> crate::rms_memory_core::X
        text = re.sub(rf"\b{sibling}::", f"crate::{sibling}::", text)
        # use rms_memory_core::... already covered
        # use rms_memory_core; -> use crate::rms_memory_core;
        text = re.sub(
            rf"\buse {sibling};",
            f"use crate::{sibling};",
            text,
        )
        text = re.sub(
            rf"\buse {sibling} as ",
            f"use crate::{sibling} as ",
            text,
        )
    return text


def copy_internal_crate(root: Path, stage: Path, pkg: str, module: str, all_modules: list[str]) -> None:
    src = root / "crates" / pkg / "src"
    dest = stage / "src" / module
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(src, dest)
    for path in dest.rglob("*.rs"):
        original = path.read_text(encoding="utf-8")
        path.write_text(rewrite_rs(original, module, all_modules), encoding="utf-8")


def write_root_lib(root: Path, stage: Path) -> None:
    original = (root / "src" / "lib.rs").read_text(encoding="utf-8")
    # Drop workspace re-exports; replace with local modules.
    header = """/// RMS Memory MCP Server (crates.io single-package flatten).
///
/// Workspace members are path-only during development. This staging tree
/// inlines them as modules so only `rms-memory-mcp` is published.
"""
    mods = "\n".join(
        f'#[path = "{module}/lib.rs"]\npub mod {module};' for _, module in INTERNAL
    )
    reexports = """
pub use rms_memory_core::{audit, config_manager, document, link, path_policy, workspace};
pub use rms_memory_index::{
    code_indexer, code_parser, graph, graph_store, index_lock, indexer, jobs, retrieval,
    semantic_graph, store, vault_graph, wiki,
};
pub use rms_memory_vault::{document_service, import, project_migrate, project_service};
"""
    # Keep everything after the original pub use block.
    rest_match = re.search(
        r"pub mod tools;",
        original,
    )
    if not rest_match:
        raise SystemExit("src/lib.rs: expected `pub mod tools;`")
    rest = original[rest_match.start() :]
    # Rewrite any remaining rms_memory_* crate paths in the umbrella sources.
    # (umbrella lib itself mostly uses re-exports; other src/ files may use crates.)
    (stage / "src" / "lib.rs").write_text(
        header + mods + "\n" + reexports + "\n" + rest,
        encoding="utf-8",
    )


def rewrite_umbrella_sources(stage: Path, all_modules: list[str]) -> None:
    skip_roots = {stage / "src" / module for _, module in INTERNAL}
    for path in (stage / "src").rglob("*.rs"):
        if any(skip in path.parents or path.parent == skip for skip in skip_roots):
            # Internal modules already rewritten.
            if path.parent.name in {m for _, m in INTERNAL} or any(
                p.name in {m for _, m in INTERNAL} for p in path.parents
            ):
                # Only skip files under src/<internal>/
                rel = path.relative_to(stage / "src")
                if rel.parts and rel.parts[0] in {m for _, m in INTERNAL}:
                    continue
        text = path.read_text(encoding="utf-8")
        new = text
        for module in all_modules:
            new = re.sub(rf"\b{module}::", f"crate::{module}::", new)
            new = re.sub(rf"\buse {module};", f"use crate::{module};", new)
        # Avoid double crate::crate::
        new = new.replace("crate::crate::", "crate::")
        if new != text:
            path.write_text(new, encoding="utf-8")


def write_cargo_toml(root: Path, stage: Path, deps: dict[str, str]) -> None:
    root_toml = (root / "Cargo.toml").read_text(encoding="utf-8")
    # Extract [package] through end of package metadata (before [dependencies] or [[bin]]).
    pkg_match = re.search(
        r"^\[package\]\n(.*?)(?=^\[package\.metadata|^\[dependencies\]|^\[\[bin\]\])",
        root_toml,
        flags=re.MULTILINE | re.DOTALL,
    )
    if not pkg_match:
        raise SystemExit("root Cargo.toml: missing [package]")
    package_body = pkg_match.group(0)
    # Include package.metadata.generate-rpm if present.
    meta_match = re.search(
        r"^\[package\.metadata\.generate-rpm\]\n(.*?)(?=^\[|\Z)",
        root_toml,
        flags=re.MULTILINE | re.DOTALL,
    )
    meta = meta_match.group(0) if meta_match else ""

    bin_match = re.search(
        r"^\[\[bin\]\]\n(.*?)(?=^\[|\Z)",
        root_toml,
        flags=re.MULTILINE | re.DOTALL,
    )
    bin_section = bin_match.group(0).rstrip() + "\n" if bin_match else ""

    profile_match = re.search(
        r"^\[profile\.release\]\n(.*?)(?=^\[|\Z)",
        root_toml,
        flags=re.MULTILINE | re.DOTALL,
    )
    profile = profile_match.group(0).rstrip() + "\n" if profile_match else ""

    lines = [
        "# Generated by scripts/flatten-for-crates-io.py — do not edit by hand.",
        "# Single-package publish tree: workspace members are inlined as modules.",
        "",
        package_body.rstrip(),
        "",
    ]
    if meta:
        lines.extend([meta.rstrip(), ""])
    lines.append("[dependencies]")
    for name in sorted(deps):
        lines.append(f"{name} = {deps[name]}")
    lines.append("")
    if bin_section:
        lines.append(bin_section)
    if profile:
        lines.append(profile)

    (stage / "Cargo.toml").write_text("\n".join(lines) + "\n", encoding="utf-8")


def copy_umbrella_tree(root: Path, stage: Path) -> None:
    # Copy package-owned sources and assets (not the workspace crates/ or target/).
    for name in ("src", "templates", "README.md", "LICENSE", "CHANGELOG.md"):
        src = root / name
        if not src.exists():
            continue
        dest = stage / name
        if src.is_dir():
            if dest.exists():
                shutil.rmtree(dest)
            shutil.copytree(
                src,
                dest,
                ignore=shutil.ignore_patterns("*.bak*", ".DS_Store"),
            )
        else:
            shutil.copy2(src, dest)


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(f"Usage: {sys.argv[0]} <repo-root> <staging-dir>")
    root = Path(sys.argv[1]).resolve()
    stage = Path(sys.argv[2]).resolve()
    stage.mkdir(parents=True, exist_ok=True)

    all_modules = [module for _, module in INTERNAL]
    copy_umbrella_tree(root, stage)
    for pkg, module in INTERNAL:
        copy_internal_crate(root, stage, pkg, module, all_modules)
    write_root_lib(root, stage)
    rewrite_umbrella_sources(stage, all_modules)
    deps = collect_external_deps(root)
    write_cargo_toml(root, stage, deps)

    # Include files referenced by generate-rpm / package (best-effort).
    print(f"Flattened publish tree at {stage}", flush=True)


if __name__ == "__main__":
    main()
