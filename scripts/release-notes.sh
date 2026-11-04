#!/usr/bin/env bash
# Prints the CHANGELOG.md section of one version as Markdown (fails when missing or empty).
#   scripts/release-notes.sh 1.3.3 > notes.md
set -euo pipefail
VERSION="${1:?usage: release-notes.sh <version>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BODY="$(awk -v v="$VERSION" '
    $0 ~ "^## \\[" v "\\]" { found = 1; next }
    found && /^## \[/ { exit }
    found { print }
' "$ROOT/CHANGELOG.md" | sed -e '/./,$!d')"
if [[ -z "${BODY//[[:space:]]/}" ]]; then
    echo "CHANGELOG.md has no non-empty section '## [$VERSION]'." >&2
    exit 1
fi
printf '## %s\n\n%s\n' "$VERSION" "$BODY"
