#!/usr/bin/env bash
# Verify rms-memory-mcp VERSION exists on crates.io.
#
# Only the root package is published (see publish-workspace-crates.sh):
# workspace members (core/index/vault/cli) stay path-only with publish = false.
# crates.io rejects curl's default User-Agent with 403, so set one.
set -euo pipefail

VERSION="${1:-1.1.0}"
PACKAGES=(
  rms-memory-mcp
)

failed=0
for pkg in "${PACKAGES[@]}"; do
  url="https://crates.io/api/v1/crates/${pkg}/${VERSION}"
  if curl -fsS -A "rms-memory-release-check/${VERSION}" "${url}" >/dev/null; then
    echo "OK  ${pkg} ${VERSION}"
  else
    echo "MISSING  ${pkg} ${VERSION}  (${url})" >&2
    failed=1
  fi
done

if [ "${failed}" -ne 0 ]; then
  exit 1
fi

echo "==> All ${#PACKAGES[@]} published crate(s) at ${VERSION} are on crates.io."
