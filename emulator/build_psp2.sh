#!/usr/bin/env bash
# Builds the VPK with cargo-vita (run inside the Docker image, see ../build.sh).
set -euo pipefail
export RUSTFLAGS="${RUSTFLAGS:-} -C target-cpu=cortex-a9"
cargo +nightly vita build vpk --profile=vita
