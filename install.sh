#!/bin/sh
set -e

# WebFind Universal Shell Installer
# Supports macOS (Apple Silicon / Intel) and Linux (x86_64)

REPO="Gaurav-Wankhede/webfind"
TAG="${WEBFIND_VERSION:-latest}"

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Darwin)
    case "$ARCH" in
      arm64|aarch64) TARGET="aarch64-apple-darwin" ;;
      x86_64) TARGET="x86_64-apple-darwin" ;;
      *) echo "Unsupported architecture: $ARCH on $OS"; exit 1 ;;
    esac
    ;;
  Linux)
    case "$ARCH" in
      x86_64) TARGET="x86_64-unknown-linux-gnu" ;;
      *) echo "Unsupported architecture: $ARCH on $OS"; exit 1 ;;
    esac
    ;;
  *)
    echo "Unsupported OS: $OS. Please install via cargo: cargo install webfind"
    exit 1
    ;;
esac

if [ "$TAG" = "latest" ]; then
  DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/webfind-${TARGET}.tar.gz"
else
  DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TAG}/webfind-${TARGET}.tar.gz"
fi

INSTALL_DIR="${WEBFIND_INSTALL_DIR:-/usr/local/bin}"

# Fallback to ~/.local/bin if /usr/local/bin is not writable
if [ ! -w "$INSTALL_DIR" ]; then
  INSTALL_DIR="$HOME/.local/bin"
  mkdir -p "$INSTALL_DIR"
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "==> Downloading webfind for ${TARGET}..."
curl -fsSL "$DOWNLOAD_URL" -o "${TMP_DIR}/webfind.tar.gz"

echo "==> Extracting to ${INSTALL_DIR}..."
tar -xzf "${TMP_DIR}/webfind.tar.gz" -C "${TMP_DIR}"
chmod +x "${TMP_DIR}/webfind"
mv "${TMP_DIR}/webfind" "${INSTALL_DIR}/webfind"

echo "==> Successfully installed webfind to ${INSTALL_DIR}/webfind!"
if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
  echo "==> Note: Please ensure ${INSTALL_DIR} is in your PATH."
fi
