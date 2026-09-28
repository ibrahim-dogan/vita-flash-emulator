#!/usr/bin/env bash
# Builds flashvita.vpk inside Docker and copies it into ./dist.
set -euo pipefail
cd "$(dirname "$0")"

DOCKER_BUILDKIT=1 docker build \
    -f docker/Dockerfile.psvita \
    -t flashvita-emu:psvita \
    .

mkdir -p dist
container_id="$(docker create flashvita-emu:psvita)"
docker cp "${container_id}:/out/." dist/
docker rm "${container_id}" > /dev/null

echo "Done. Artifacts in ./dist:"
ls -la dist
