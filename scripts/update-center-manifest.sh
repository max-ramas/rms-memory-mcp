#!/usr/bin/env bash
# Writes and signs release.json for the RMS update center from the built archives.
#   scripts/update-center-manifest.sh <version> <artifacts-dir> <out-dir> <rms-release-binary> <key-file> [notes.md]
# Only archives that exist are listed (manual runs may build a subset of targets).
# Needs RMS_RELEASE_KEY_PASSWORD in the environment when the key is encrypted.
set -euo pipefail
VERSION="${1:?version}"; ART="${2:?artifacts dir}"; OUT="${3:?out dir}"; TOOL="${4:?rms-release}"; KEY="${5:?key file}"; NOTES="${6:-}"
mkdir -p "$OUT"

args=()
add() { # add <platform/arch/format> <target> <ext>
    local f
    f="$(find "$ART" -name "rms_memory_mcp_${VERSION}_$2.$3" -type f | head -n1)"
    [[ -n "$f" ]] || { echo "skip $2 (not built)"; return 0; }
    cp "$f" "$OUT/"
    args+=(--artifact "$1=$OUT/$(basename "$f")")
}
add linux/amd64/tar.gz   x86_64-unknown-linux-gnu  tar.gz
add linux/arm64/tar.gz   aarch64-unknown-linux-gnu tar.gz
add darwin/arm64/tar.gz  aarch64-apple-darwin      tar.gz
add windows/amd64/zip    x86_64-pc-windows-msvc    zip
[[ ${#args[@]} -gt 0 ]] || { echo "no archives found in $ART" >&2; exit 1; }

notes=()
if [[ -n "$NOTES" ]]; then notes=(--notes-file "$NOTES"); fi
"$TOOL" manifest --product rms-memory-mcp --version "$VERSION" --channel stable \
    --key "$KEY" "${notes[@]}" "${args[@]}" --out "$OUT"
