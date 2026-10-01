#!/usr/bin/env bash
# Builds rufflevita.vpk inside Docker and copies it into ./dist.
set -euo pipefail
cd "$(dirname "$0")"

DOCKER_BUILDKIT=1 docker build \
    -f docker/Dockerfile.psvita \
    --build-arg RUFFLEVITA_FEATURES="${RUFFLEVITA_FEATURES:-}" \
    -t rufflevita:psvita \
    .

mkdir -p dist
container_id="$(docker create rufflevita:psvita)"
docker cp "${container_id}:/out/." dist/
docker rm "${container_id}" > /dev/null

echo "Done. Artifacts in ./dist:"
ls -la dist
