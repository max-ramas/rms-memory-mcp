#!/usr/bin/env bash
# Builds a macOS installer (.pkg) that installs the `rms-memory` binary into /usr/local/bin.
# Unsigned (no Apple Developer ID yet): after a browser download macOS asks to confirm once
# (right-click > Open); `curl` downloads and the one-line installer are not affected.
#
# Usage: package-macos-pkg.sh <version> <path to rms-memory binary> <out dir> [arch-suffix]
# Output: <out dir>/rms_memory_mcp_<version>_aarch64-apple-darwin.pkg
set -euo pipefail
VERSION="${1:?version}"; BIN="${2:?binary}"; OUT="${3:?out dir}"; TARGET="${4:-aarch64-apple-darwin}"
[[ -x "$BIN" ]] || { echo "not an executable: $BIN" >&2; exit 1; }
HOST_ARCH="arm64"; [[ "$TARGET" == x86_64-* ]] && HOST_ARCH="x86_64"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/root/usr/local/bin" "$OUT"
install -m 0755 "$BIN" "$WORK/root/usr/local/bin/rms-memory"
pkgbuild --root "$WORK/root" --identifier com.rms-ds.rms-memory-mcp --version "$VERSION" \
  --install-location / --ownership recommended "$WORK/component.pkg" >/dev/null
cat > "$WORK/distribution.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
  <title>RMS Memory MCP $VERSION</title>
  <options customize="never" require-scripts="false" hostArchitectures="$HOST_ARCH"/>
  <domains enable_localSystem="true"/>
  <pkg-ref id="com.rms-ds.rms-memory-mcp"/>
  <choices-outline><line choice="default"><line choice="com.rms-ds.rms-memory-mcp"/></line></choices-outline>
  <choice id="default"/>
  <choice id="com.rms-ds.rms-memory-mcp" visible="false"><pkg-ref id="com.rms-ds.rms-memory-mcp"/></choice>
  <pkg-ref id="com.rms-ds.rms-memory-mcp" version="$VERSION" onConclusion="none">component.pkg</pkg-ref>
</installer-gui-script>
XML
FINAL="$OUT/rms_memory_mcp_${VERSION}_${TARGET}.pkg"
productbuild --distribution "$WORK/distribution.xml" --package-path "$WORK" "$FINAL" >/dev/null
echo "$FINAL"
