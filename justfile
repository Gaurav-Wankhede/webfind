# Canonical Deployment & Release Automation for WebFind
# SSOT: The version is dynamically resolved from Cargo.toml via `kavach toml`
#
# Cross-platform: every recipe derives its target triple and archive format from
# the host, so the same recipes work on Linux, macOS and Windows (Git Bash).

set shell := ["bash", "-uc"]

# Show available commands
default:
    @just --list

# Extract single source of truth version from Cargo.toml
version := `kavach toml "package.version" Cargo.toml`

# Host target triple, for naming release artifacts.
# `rustc -vV` is the portable way to ask the toolchain for its own host.
host := `rustc -vV | sed -n 's/^host: //p'`

# Release artifact naming, derived per-run so the same recipe works on
# Linux, macOS and Windows. The archive format is chosen from the host triple:
# `zip` on Windows, `tar.gz` elsewhere. Windows users run this through Git Bash
# or WSL, where `tar` and `shasum` are available.
#
# NOTE: computed inside the recipe rather than as an `if` expression, because
# `just` expression conditionals are not portable across just versions.

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
    @echo "==> Building optimized release binary for v{{version}} (host: {{host}})..."
    cargo build --release
    @mkdir -p dist
    cp target/release/webfind dist/
    @echo "==> Packaging distribution archive..."
    @if case "{{host}}" in *windows*) true;; *) false;; esac; then \
        cd dist && zip -q "webfind-{{host}}.zip" webfind && cd ..; \
    else \
        cd dist && tar -czf "webfind-{{host}}.tar.gz" webfind && cd ..; \
    fi
    @cd dist && { shasum -a 256 "webfind-{{host}}.tar.gz" > "webfind-{{host}}.tar.gz.sha256" 2>/dev/null || shasum -a 256 "webfind-{{host}}.zip" > "webfind-{{host}}.zip.sha256"; }
    @rm -f dist/webfind
    @echo "==> Distribution archive ready in dist/:"
    @ls -1 dist/


# 4. Deploy release asset to GitHub (tag v{{version}})
deploy-github:
    @echo "==> Uploading release assets to GitHub Release v{{version}}..."
    @ext=$(cat .ext); artifact=$(cat .artifact)
    @gh release upload "v{{version}}" "dist/$artifact" "dist/$artifact.sha256" --clobber

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
