#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "The macOS server must be built on macOS (ScreenCaptureKit / Apple SDK required)." >&2
  exit 1
fi

rustup target add aarch64-apple-darwin
cargo build --release -p localdesk-macos-server --target aarch64-apple-darwin
