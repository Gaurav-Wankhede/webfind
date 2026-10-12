# Canonical Deployment & Release Automation for WebFind
# SSOT: The version is dynamically resolved from Cargo.toml via `kavach toml`

set shell := ["bash", "-uc"]

# Show available commands
default:
    @just --list

# Extract single source of truth version from Cargo.toml
version := `kavach toml "package.version" Cargo.toml`

# 1. Clean local build artifacts and packaging caches
clean:
    cargo clean
    rm -rf dist/

# 2. Check compilation and execute test suite
check:
    cargo check
    cargo test

# 3. Compile optimized release binary locally and package tarball with sha256 checksum
build-release:
    @echo "==> Building optimized release binary for v{{version}}..."
    cargo build --release
    @mkdir -p dist
    cp target/release/webfind dist/
    @echo "==> Packaging distribution tarball..."
    cd dist && tar -czf webfind-aarch64-apple-darwin.tar.gz webfind
    cd dist && shasum -a 256 webfind-aarch64-apple-darwin.tar.gz > webfind-aarch64-apple-darwin.tar.gz.sha256
    rm -f dist/webfind
    @echo "==> Cleaned raw binary. Distribution tarball ready in dist/:"
    @cat dist/webfind-aarch64-apple-darwin.tar.gz.sha256

# 4. Deploy release asset to GitHub (tag v{{version}})
deploy-github:
    @echo "==> Uploading release assets to GitHub Release v{{version}}..."
    gh release upload "v{{version}}" dist/webfind-aarch64-apple-darwin.tar.gz dist/webfind-aarch64-apple-darwin.tar.gz.sha256 --clobber

# 5. Publish crate directly to Crates.io
deploy-crates-io:
    @echo "==> Publishing webfind v{{version}} to Crates.io..."
    cargo publish

# 6. Synchronize Homebrew Formula in Gaurav-Wankhede/homebrew-tap
deploy-homebrew:
    @echo "==> Synchronizing Homebrew tap with v{{version}}..."
    ./scripts/update_brew_formula.sh "{{version}}"

# 7. Master One-Command Deployment: Builds, packages, publishes to Crates.io, uploads to GitHub, updates Homebrew, and cleans target/
deploy-all: check build-release deploy-github deploy-crates-io deploy-homebrew clean
    @echo "==> WebFind v{{version}} successfully synchronized across all platforms (Crates.io, GitHub, Homebrew)!"
