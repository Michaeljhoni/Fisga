#!/usr/bin/env bash
# Runs cargo inside the official Rust Linux image (useful when developing from
# Windows/macOS). Usage: scripts/docker-cargo.sh test --workspace
set -euo pipefail
cd "$(dirname "$0")/.."
# Git Bash on Windows needs the native path (C:/...) for the bind mount.
root="$(pwd -W 2>/dev/null || pwd)"
MSYS_NO_PATHCONV=1 exec docker run --rm \
  -v "$root:/src" -w /src \
  -v ldm-cargo-registry:/usr/local/cargo/registry \
  -v ldm-target:/target -e CARGO_TARGET_DIR=/target \
  -e CARGO_TERM_COLOR=never \
  rust:1-slim-bookworm cargo "$@"
