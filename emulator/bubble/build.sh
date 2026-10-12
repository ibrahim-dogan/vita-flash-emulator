#!/usr/bin/env bash
# Builds the bubble forwarder (forwarder.c) into ../assets/bubble/forwarder.bin,
# which the Vita build embeds. Needs VitaSDK; the Docker build runs this.
set -euo pipefail
cd "$(dirname "$0")"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

arm-vita-eabi-gcc -O2 -Wall -Wl,-q forwarder.c -o "$TMP/forwarder.elf" \
    -lSceAppMgr_stub -lSceIofilemgr_stub -lSceLibKernel_stub -lSceKernelThreadMgr_stub
vita-elf-create "$TMP/forwarder.elf" "$TMP/forwarder.velf"
mkdir -p ../assets/bubble
vita-make-fself -s "$TMP/forwarder.velf" ../assets/bubble/forwarder.bin
echo "wrote assets/bubble/forwarder.bin"
