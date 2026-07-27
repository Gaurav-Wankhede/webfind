#!/bin/sh
# WebFind MCP server launcher for agentic CLIs.
# Builds the release binary if it doesn't exist, then runs it in stdio MCP mode.
set -e

ROOT_DIR=$(cd "$(dirname "$0")/.." && pwd)
BIN="$ROOT_DIR/target/release/webfind"

if [ ! -x "$BIN" ]; then
  echo "Building webfind release binary..." >&2
  cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml" >&2
fi

exec "$BIN" serve --transport stdio
