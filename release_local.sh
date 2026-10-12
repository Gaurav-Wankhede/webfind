#!/bin/bash
set -e

# Local Release Builder & Publisher for WebFind
# Builds optimized binary, packages it, generates checksums, cleans build bloat, and uploads to GitHub.

TAG=$(git describe --tags --abbrev=0 2>/dev/null || echo "v0.5.0")
TARGET="aarch64-apple-darwin"
DIST_DIR="dist"

echo "==> Building optimized release binary locally for ${TAG}..."
cargo build --release

echo "==> Packaging binary..."
mkdir -p "${DIST_DIR}"
cp target/release/webfind "${DIST_DIR}/webfind"

# Clean target directory immediately to save disk space
echo "==> Cleaning heavy build directory..."
cargo clean

cd "${DIST_DIR}"
tar -czf "webfind-${TARGET}.tar.gz" webfind
shasum -a 256 "webfind-${TARGET}.tar.gz" > "webfind-${TARGET}.tar.gz.sha256"
rm -f webfind
cd ..

echo "==> Uploading assets directly to GitHub Release ${TAG}..."
gh release upload "${TAG}" "${DIST_DIR}/webfind-${TARGET}.tar.gz" "${DIST_DIR}/webfind-${TARGET}.tar.gz.sha256" --clobber

echo "==> Cleaning local dist artifacts..."
rm -rf "${DIST_DIR}"

echo "==> Release ${TAG} assets uploaded successfully with zero CI cost!"
