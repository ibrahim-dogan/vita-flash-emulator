#!/usr/bin/env bash
# Builds the VPK with cargo-vita (run inside the Docker image, see ../build.sh).
set -euo pipefail
export RUSTFLAGS="${RUSTFLAGS:-} -C target-cpu=cortex-a9"
# RUFFLEVITA_FEATURES, e.g. "prof_ops" for a profiling build (docs/PERFORMANCE_PLAN.md).
cargo +nightly vita build vpk --profile=vita ${RUFFLEVITA_FEATURES:+--features "$RUFFLEVITA_FEATURES"}
