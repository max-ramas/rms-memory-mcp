#!/usr/bin/env bash
# Publish ONLY rms-memory-mcp to crates.io.
#
# Workspace members (core/index/vault/cli) stay path-only with publish = false.
# Cargo cannot publish an umbrella that depends on unpublished path crates, so
# we flatten those sources into a single-package staging tree first.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STAGE="$(mktemp -d "${TMPDIR:-/tmp}/rms-memory-mcp-publish.XXXXXX")"
cleanup() {
  rm -rf "${STAGE}"
}
trap cleanup EXIT

echo "==> Flattening workspace into single-package staging tree..."
python3 "${ROOT}/scripts/flatten-for-crates-io.py" "${ROOT}" "${STAGE}"

cd "${STAGE}"
echo "==> Generating Cargo.lock for staging tree..."
cargo generate-lockfile

echo "==> Publishing rms-memory-mcp only..."
set +e
output="$(cargo publish --locked 2>&1)"
status=$?
set -e
printf '%s\n' "${output}"
if [ "${status}" -eq 0 ]; then
  echo "==> rms-memory-mcp published."
  exit 0
fi
if printf '%s\n' "${output}" | grep -q 'already exists on crates.io'; then
  echo "==> rms-memory-mcp already on crates.io; nothing to do."
  exit 0
fi
exit "${status}"
