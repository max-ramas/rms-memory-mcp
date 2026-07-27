#!/usr/bin/env bash
# Verify all four workspace crates for VERSION exist on crates.io.
set -euo pipefail

VERSION="${1:-1.1.0}"
PACKAGES=(
  rms-memory-core
  rms-memory-index
  rms-memory-vault
  rms-memory-mcp
)

failed=0
for pkg in "${PACKAGES[@]}"; do
  url="https://crates.io/api/v1/crates/${pkg}/${VERSION}"
  if curl -fsS "${url}" >/dev/null; then
    echo "OK  ${pkg} ${VERSION}"
  else
    echo "MISSING  ${pkg} ${VERSION}  (${url})" >&2
    failed=1
  fi
done

if [ "${failed}" -ne 0 ]; then
  exit 1
fi

echo "==> All ${#PACKAGES[@]} crates at ${VERSION} are on crates.io."
