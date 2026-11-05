#!/usr/bin/env bash
# RMS Memory MCP installer. Downloads the release from the RMS update center
# (https://update.rms-ds.com) and verifies size and sha256 against the manifest.
# If `minisign` is installed the manifest signature is verified as well; the
# built-in `rms-memory update` always verifies it. Falls back to GitHub Releases
# when the update center is unreachable or has no build for this machine.
set -euo pipefail

UPDATE_URL="${RMS_MEMORY_UPDATE_URL:-https://update.rms-ds.com}"
UPDATE_URL="${UPDATE_URL%/}"
PRODUCT="rms-memory-mcp"
REPO="max-ramas/rms-memory-mcp"
# Production release key (rel-2026a) and the CI key; override with RMS_MEMORY_UPDATE_PUBKEY.
PUBKEYS=("RWTzQ/pE7Yj4phFhGzE5d1fPXTUULnZzQYpeULlnjPSr/g0qrGFVrl3m" "RWQMrrP84Dr0wbPBA3FAoYdqHLNc2nfuBxENI19mmBn+dENmYT7ShULQ")
[ -n "${RMS_MEMORY_UPDATE_PUBKEY:-}" ] && PUBKEYS+=("${RMS_MEMORY_UPDATE_PUBKEY##*$'\n'}")

echo "RMS Memory MCP - Installation Script"
echo "------------------------------------"

case "$(uname -s)" in
    Linux*)  PLATFORM="linux";  MACHINE_OS="unknown-linux-gnu";;
    Darwin*) PLATFORM="darwin"; MACHINE_OS="apple-darwin";;
    *) echo "Unsupported OS: $(uname -s)"; exit 1;;
esac
case "$(uname -m)" in
    x86_64)        ARCH="amd64"; MACHINE_ARCH="x86_64";;
    arm64|aarch64) ARCH="arm64"; MACHINE_ARCH="aarch64";;
    *) echo "Unsupported architecture: $(uname -m)"; exit 1;;
esac
TARGET="${MACHINE_ARCH}-${MACHINE_OS}"
if [ "$TARGET" = "x86_64-apple-darwin" ]; then
    echo "Error: Prebuilt binaries are not provided for macOS Intel (x86_64)."
    echo "Please use Cargo to install from source:  cargo install rms-memory-mcp"
    exit 1
fi
echo "Detected target: ${TARGET}"

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
    else shasum -a 256 "$1" | cut -d' ' -f1; fi
}
json_field() { # json_field <file> <key>  (flat string/number field, no jq needed)
    sed -n "s/.*\"$2\"[[:space:]]*:[[:space:]]*\"\{0,1\}\([^\",}]*\)\"\{0,1\}.*/\1/p" "$1" | head -n1
}

TEMP_DIR=$(mktemp -d)
trap 'rm -rf "$TEMP_DIR"' EXIT
ARCHIVE="${TEMP_DIR}/rms-memory.tar.gz"

from_update_center() {
    local latest="${TEMP_DIR}/latest.json" manifest="${TEMP_DIR}/release.json"
    curl -fsSL "${UPDATE_URL}/v1/${PRODUCT}/latest?channel=stable&platform=${PLATFORM}&arch=${ARCH}&format=tar.gz" -o "$latest" || return 1
    local version release_url
    version=$(json_field "$latest" version)
    release_url=$(json_field "$latest" release_url)
    [ -n "$version" ] && [ -n "$release_url" ] || return 1
    curl -fsSL "$release_url" -o "$manifest" || return 1
    curl -fsSL "${release_url}.minisig" -o "${manifest}.minisig" || return 1
    if command -v minisign >/dev/null 2>&1; then
        local ok=0 key
        for key in "${PUBKEYS[@]}"; do
            minisign -Vm "$manifest" -x "${manifest}.minisig" -P "$key" >/dev/null 2>&1 && ok=1 && break
        done
        [ "$ok" = 1 ] || { echo "Error: release signature is not valid for a trusted key."; exit 1; }
        echo "Manifest signature verified (minisign)."
    else
        echo "Note: minisign not installed; trusting TLS + sha256. 'rms-memory update' verifies signatures."
    fi
    local file="rms_memory_mcp_${version}_${TARGET}.tar.gz" sha size
    # sha256/size come from the (signed) manifest entry for this platform/arch.
    local entry
    entry=$(tr -d '\n' < "$manifest" | sed 's/},[[:space:]]*{/}\n{/g' | grep "\"platform\"[[:space:]]*:[[:space:]]*\"${PLATFORM}\"" | grep "\"arch\"[[:space:]]*:[[:space:]]*\"${ARCH}\"" | grep '"tar.gz"' | head -n1)
    [ -n "$entry" ] || return 1
    sha=$(printf '%s' "$entry" | sed -n 's/.*"sha256"[[:space:]]*:[[:space:]]*"\([0-9a-f]\{64\}\)".*/\1/p')
    file=$(printf '%s' "$entry" | sed -n 's/.*"file"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
    case "$file" in */*|""|..) echo "Error: unsafe file name in manifest."; exit 1;; esac
    echo "Downloading ${UPDATE_URL}/dl/${PRODUCT}/${version}/${file}..."
    curl -fsSL "${UPDATE_URL}/dl/${PRODUCT}/${version}/${file}" -o "$ARCHIVE" || return 1
    [ "$(sha256_of "$ARCHIVE")" = "$sha" ] || { echo "Error: checksum mismatch."; exit 1; }
    echo "Installed version: ${version} (from the RMS update center)"
}

from_github() {
    echo "Falling back to GitHub Releases..."
    local tag version
    tag=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')
    case "$tag" in v[0-9]*) version="${tag#v}";; *) echo "Error: could not determine the latest release."; exit 1;; esac
    local url="https://github.com/${REPO}/releases/download/${tag}/rms_memory_mcp_${version}_${TARGET}.tar.gz"
    echo "Downloading ${url}..."
    curl -fsSL "$url" -o "$ARCHIVE"
}

if ! from_update_center; then
    echo "Update center unavailable or has no build for ${PLATFORM}/${ARCH}."
    from_github
fi

echo "Extracting..."
tar -xzf "$ARCHIVE" -C "$TEMP_DIR"
BIN_DIR="${HOME}/.local/bin"
mkdir -p "$BIN_DIR"
echo "Installing to ${BIN_DIR}/rms-memory"
mv "${TEMP_DIR}/rms-memory" "${BIN_DIR}/rms-memory"
chmod +x "${BIN_DIR}/rms-memory"

if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
    echo ""
    echo "WARNING: ${BIN_DIR} is not in your PATH."
    echo "Please add 'export PATH=\"\$HOME/.local/bin:\$PATH\"' to your ~/.bashrc or ~/.zshrc"
    echo ""
fi

echo "Installation successful! Future updates: rms-memory update"
echo "Running rms-memory install to hook into IDEs..."
"${BIN_DIR}/rms-memory" install
