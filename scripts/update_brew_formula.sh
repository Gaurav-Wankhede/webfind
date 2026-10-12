#!/bin/bash
set -e

# Dynamically updates Gaurav-Wankhede/homebrew-tap Formula/webfind.rb
# Uses exact version from Cargo.toml and exact SHA256 of the packaged artifact.

VERSION="${1}"
if [ -z "${VERSION}" ]; then
  VERSION=$(kavach toml "package.version" Cargo.toml)
fi

SHA256="${2}"
if [ -z "${SHA256}" ]; then
  if [ -f "dist/webfind-aarch64-apple-darwin.tar.gz.sha256" ]; then
    SHA256=$(awk '{print $1}' dist/webfind-aarch64-apple-darwin.tar.gz.sha256)
  else
    echo "[ERROR] Missing checksum. Provide as argument or run build-release first."
    exit 1
  fi
fi

TAP_DIR="/tmp/homebrew-tap-sync"
rm -rf "${TAP_DIR}"

echo "==> Cloning Gaurav-Wankhede/homebrew-tap..."
git clone https://github.com/Gaurav-Wankhede/homebrew-tap.git "${TAP_DIR}"

cat << FORMULA > "${TAP_DIR}/Formula/webfind.rb"
class Webfind < Formula
  desc "Self-hosted native search engine for AI agents — pure CLI, zero cost"
  homepage "https://github.com/Gaurav-Wankhede/webfind"
  version "${VERSION}"
  license "MIT"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/Gaurav-Wankhede/webfind/releases/download/v${VERSION}/webfind-aarch64-apple-darwin.tar.gz"
    sha256 "${SHA256}"
  end

  def install
    bin.install "webfind"
  end

  test do
    assert_match "webfind ${VERSION}", shell_output("#{bin}/webfind --version")
  end
end
FORMULA

cd "${TAP_DIR}"
git add Formula/webfind.rb
if git diff --staged --quiet; then
  echo "==> Homebrew formula is already up to date with version ${VERSION}."
else
  git commit -m "chore(formula): sync webfind to v${VERSION}"
  git push origin main
  echo "==> Successfully synchronized Homebrew tap to v${VERSION} (${SHA256})!"
fi

rm -rf "${TAP_DIR}"
