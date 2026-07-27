#!/usr/bin/env bash
# Yank orphaned internal workspace crate versions on crates.io.
#
# Context: publish-workspace-crates.sh uploaded rms-memory-{core,index,vault}
# 1.0.9 but rms-memory-mcp 1.0.9 never published (index lag). End users still
# install rms-memory-mcp 1.0.8 via `cargo install`. The 1.0.9 internals are
# useless without a matching umbrella and should be yanked.
#
# Usage:
#   export CARGO_REGISTRY_TOKEN=...   # crates.io token (owner of the crates)
#   bash scripts/yank-orphan-workspace-crates.sh 1.0.9
#
# Unyank (if needed):
#   cargo yank --vers 1.0.9 -p rms-memory-core --undo
set -euo pipefail

VERSION="${1:-}"
YES=false
if [ "${VERSION}" = "--yes" ] || [ "${VERSION}" = "-y" ]; then
  echo "Usage: $0 [--yes] <version-to-yank>" >&2
  exit 1
fi
if [ "${1:-}" = "--yes" ] || [ "${1:-}" = "-y" ]; then
  YES=true
  VERSION="${2:-}"
fi
if [ -z "${VERSION}" ]; then
  echo "Usage: $0 [--yes] <version-to-yank>" >&2
  echo "Example: $0 1.0.9" >&2
  exit 1
fi

if [ -z "${CARGO_REGISTRY_TOKEN:-}" ]; then
  echo "CARGO_REGISTRY_TOKEN is not set." >&2
  exit 1
fi

PACKAGES=(
  rms-memory-core
  rms-memory-index
  rms-memory-vault
)

echo "Will yank ${VERSION} for: ${PACKAGES[*]}"
echo "rms-memory-mcp is NOT yanked (users still on 1.0.8 until 1.1.0 publishes)."
if [ "${YES}" != true ]; then
  read -r -p "Continue? [y/N] " confirm
  if [ "${confirm}" != "y" ] && [ "${confirm}" != "Y" ]; then
    echo "Aborted."
    exit 0
  fi
fi

for pkg in "${PACKAGES[@]}"; do
  echo "==> cargo yank --vers ${VERSION} ${pkg}"
  cargo yank --vers "${VERSION}" "${pkg}"
done

echo "==> Done. Verify on https://crates.io/crates/rms-memory-core/${VERSION}"
