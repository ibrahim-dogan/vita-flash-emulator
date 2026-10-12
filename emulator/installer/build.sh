#!/usr/bin/env bash
# Builds the bubble installer (installer.c) into ../static/vita/installer.bin,
# which cargo-vita packs into RuffleVita's VPK. Needs VitaSDK, e.g.:
#   docker run --rm -v "$PWD/..:/src" -w /src/installer rufflevita:psvita ./build.sh
set -euo pipefail
cd "$(dirname "$0")"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

arm-vita-eabi-gcc -O2 -Wall -Wl,-q installer.c -o "$TMP/installer.elf" \
    -lScePromoterUtil_stub -lSceSysmodule_stub -lSceAppMgr_stub \
    -lSceIofilemgr_stub -lSceLibKernel_stub -lSceKernelThreadMgr_stub \
    -lSceDisplay_stub -lSceSysmem_stub
vita-elf-create "$TMP/installer.elf" "$TMP/installer.velf"
# No "-s": unsafe homebrew, which the promoter needs.
vita-make-fself -c "$TMP/installer.velf" ../static/vita/installer.bin
echo "wrote static/vita/installer.bin"
