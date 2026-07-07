#!/usr/bin/env bash
set -e

echo "RMS Memory MCP - Installation Script"
echo "------------------------------------"

# Determine OS
OS="$(uname -s)"
case "${OS}" in
    Linux*)     MACHINE_OS="unknown-linux-gnu";;
    Darwin*)    MACHINE_OS="apple-darwin";;
    *)          echo "Unsupported OS: ${OS}"; exit 1;;
esac

# Determine Architecture
ARCH="$(uname -m)"
case "${ARCH}" in
    x86_64)     MACHINE_ARCH="x86_64";;
    arm64)      MACHINE_ARCH="aarch64";;
    aarch64)    MACHINE_ARCH="aarch64";;
    *)          echo "Unsupported architecture: ${ARCH}"; exit 1;;
esac

# On macOS, we provide a universal binary
if [ "${MACHINE_OS}" = "apple-darwin" ]; then
    TARGET="universal-apple-darwin"
else
    TARGET="${MACHINE_ARCH}-${MACHINE_OS}"
fi

echo "Detected target: ${TARGET}"

# Get the latest release from GitHub API
REPO="max-ramas/rms-memory-mcp"
LATEST_RELEASE_URL="https://api.github.com/repos/${REPO}/releases/latest"

echo "Fetching latest release information..."
# Extract the tag name (e.g. v1.0.0)
TAG=$(curl -fsSL $LATEST_RELEASE_URL | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')

if [ -z "$TAG" ]; then
    echo "Error: Could not determine latest release from GitHub."
    echo "Please ensure the repository is public or you have provided the correct URL."
    exit 1
fi

echo "Latest release: ${TAG}"

DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TAG}/rms-memory-${TARGET}.tar.gz"
echo "Downloading ${DOWNLOAD_URL}..."

TEMP_DIR=$(mktemp -d)
curl -fsSL "$DOWNLOAD_URL" -o "${TEMP_DIR}/rms-memory.tar.gz"

echo "Extracting..."
tar -xzf "${TEMP_DIR}/rms-memory.tar.gz" -C "${TEMP_DIR}"

BIN_DIR="${HOME}/.local/bin"
mkdir -p "$BIN_DIR"

echo "Installing to ${BIN_DIR}/rms-memory"
mv "${TEMP_DIR}/rms-memory" "${BIN_DIR}/rms-memory"
chmod +x "${BIN_DIR}/rms-memory"

# Clean up
rm -rf "$TEMP_DIR"

# Add to PATH if not already present
if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
    echo ""
    echo "WARNING: ${BIN_DIR} is not in your PATH."
    echo "Please add 'export PATH=\"\$HOME/.local/bin:\$PATH\"' to your ~/.bashrc or ~/.zshrc"
    echo ""
fi

echo "Installation successful!"
echo "Running rms-memory install to hook into IDEs..."
"${BIN_DIR}/rms-memory" install
