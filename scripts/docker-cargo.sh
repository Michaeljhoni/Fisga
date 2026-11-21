#!/usr/bin/env bash
# Runs cargo inside a Linux container (useful when developing from Windows/macOS).
# The image is the official Rust image plus GTK4 (scripts/Dockerfile.build), built
# on first use. Usage: scripts/docker-cargo.sh test --workspace
# FISGA_TARGET_VOLUME names the build cache volume: one per checkout lets
# several checkouts build at the same time.
set -euo pipefail
cd "$(dirname "$0")/.."
# Git Bash on Windows needs the native path (C:/...) for the bind mount.
root="$(pwd -W 2>/dev/null || pwd)"
image="${FISGA_BUILD_IMAGE:-fisga-build}"
docker build -q -t "$image" -f scripts/Dockerfile.build scripts >/dev/null
MSYS_NO_PATHCONV=1 exec docker run --rm \
  -v "$root:/src" -w /src \
  -v fisga-cargo-registry:/usr/local/cargo/registry \
  -v "${FISGA_TARGET_VOLUME:-fisga-target}:/target" -e CARGO_TARGET_DIR=/target \
  -e CARGO_TERM_COLOR=never \
  -e FISGA_LANG="${FISGA_LANG:-pt}" \
  "$image" cargo "$@"
