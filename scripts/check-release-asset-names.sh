#!/usr/bin/env bash
# Contract: every published MCP/GUI release asset name is versioned.
# Canon (Phase 0 / 1.0.9):
#   MCP portable: rms_memory_mcp_<version>_<target>.{tar.gz,zip}
#   MCP deb/rpm:  rms_memory_mcp_<version>-1_… / rms_memory_mcp_<version>-1.…
#   GUI:          rms_memory_gui_<version>[_-]…
#
# This script does not download artifacts. It (1) asserts the naming formulas for
# the release matrix and (2) greps release workflows/installers for forbidden
# unversioned templates / expected_version override.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${1:-}"
if [[ -z "$VERSION" ]]; then
  VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
fi
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "VERSION must look like 1.0.9, got: ${VERSION:-<empty>}" >&2
  exit 1
fi

fail=0
ok() { echo "OK  $*"; }
bad() { echo "FAIL $*" >&2; fail=1; }

# --- Expected portable archive names (must match .github/workflows/release.yml matrix) ---
declare -a EXPECTED_PORTABLE=(
  "rms_memory_mcp_${VERSION}_x86_64-unknown-linux-gnu.tar.gz"
  "rms_memory_mcp_${VERSION}_aarch64-unknown-linux-gnu.tar.gz"
  "rms_memory_mcp_${VERSION}_aarch64-apple-darwin.tar.gz"
  "rms_memory_mcp_${VERSION}_x86_64-pc-windows-msvc.zip"
)

for name in "${EXPECTED_PORTABLE[@]}"; do
  if [[ "$name" =~ ^rms_memory_mcp_${VERSION}_.+(\.tar\.gz|\.zip)$ ]]; then
    ok "portable formula: $name"
  else
    bad "portable formula rejected: $name"
  fi
done

# Example package names after cargo-deb / cargo-generate-rpm rename
for name in \
  "rms_memory_mcp_${VERSION}-1_amd64.deb" \
  "rms_memory_mcp_${VERSION}-1.x86_64.rpm"
do
  case "$name" in
    rms_memory_mcp_"${VERSION}"-*) ok "package formula: $name" ;;
    *) bad "package formula rejected: $name" ;;
  esac
done

# GUI canon examples (product ships via GUI repo; contract is shared)
for name in \
  "rms_memory_gui_${VERSION}_amd64.AppImage" \
  "rms_memory_gui_${VERSION}_amd64.deb" \
  "rms_memory_gui_${VERSION}-1.x86_64.rpm" \
  "rms_memory_gui_${VERSION}_aarch64.dmg"
do
  case "$name" in
    rms_memory_gui_"${VERSION}"[_-]*) ok "gui formula: $name" ;;
    *) bad "gui formula rejected: $name" ;;
  esac
done

# --- Workflow / installer must not contain unversioned *file* templates ---
# Artifact `name:` may stay target-only (ephemeral CI id). Published paths must
# include version before target: …_${VERSION}_${{ matrix.target }}.…
wf="$ROOT/.github/workflows/release.yml"
if grep -nE 'rms_memory_mcp_\$\{\{ matrix\.target \}\}\.' "$wf" >/dev/null 2>&1; then
  bad "release.yml still has unversioned portable file template (target without version)"
elif grep -nE 'rms_memory_mcp_\$\{VERSION\}_\$\{\{ matrix\.target \}\}' "$wf" >/dev/null 2>&1 \
  || grep -nE 'rms_memory_mcp_\$\{\{ needs\.plan\.outputs\.version \}\}_\$\{\{ matrix\.target \}\}' "$wf" >/dev/null 2>&1; then
  ok "release.yml portable file names include version + target"
else
  bad "release.yml missing versioned portable file template"
fi

if grep -nE 'expected_version' "$wf" >/dev/null 2>&1; then
  bad "release.yml still references expected_version override"
else
  ok "release.yml has no expected_version"
fi

# Package step must interpolate VERSION into archive name
if grep -nE 'rms_memory_mcp_\$\{VERSION\}_' "$wf" >/dev/null 2>&1; then
  ok "release.yml portable name includes \${VERSION}"
else
  bad "release.yml portable name does not include \${VERSION}"
fi

for installer in "$ROOT/scripts/install.sh" "$ROOT/scripts/install.ps1"; do
  if grep -nE 'rms_memory_mcp_\$\{?(TARGET|target)' "$installer" >/dev/null 2>&1; then
    # allow if VERSION also present on same construction — check for unversioned pattern
    if grep -nE 'rms_memory_mcp_\$\{TARGET\}' "$installer" >/dev/null 2>&1 \
      || grep -nE 'rms_memory_mcp_\$target' "$installer" >/dev/null 2>&1; then
      bad "$(basename "$installer") builds unversioned asset URL"
    else
      ok "$(basename "$installer") has no unversioned TARGET-only URL"
    fi
  else
    ok "$(basename "$installer") has no unversioned TARGET-only URL"
  fi
  if grep -nE 'rms_memory_mcp_.*VERSION|VERSION.*rms_memory_mcp' "$installer" >/dev/null 2>&1 \
    || grep -nEi 'rms_memory_mcp_\$\{?version' "$installer" >/dev/null 2>&1; then
    ok "$(basename "$installer") references versioned MCP asset names"
  else
    bad "$(basename "$installer") does not reference versioned MCP asset names"
  fi
done

# GUI workflow (sibling repo) — optional if checked out beside this repo
GUI_WF="$(cd "$ROOT/.." && pwd)/rms-memory-gui/.github/workflows/release.yml"
if [[ -f "$GUI_WF" ]]; then
  if grep -nE 'rms_memory_gui_\$\{VERSION\}' "$GUI_WF" >/dev/null 2>&1; then
    ok "gui release.yml asserts rms_memory_gui_\${VERSION}"
  else
    bad "gui release.yml missing rms_memory_gui_\${VERSION} assert"
  fi
else
  echo "SKIP gui release.yml (sibling repo not present)"
fi

if [[ "$fail" -ne 0 ]]; then
  echo "Release asset-name contract FAILED" >&2
  exit 1
fi
echo "Release asset-name contract PASSED (version=$VERSION)"
