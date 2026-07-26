#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

IMAGE_NAME="moviehouse"
IMAGE_TAG="${1:-latest}"

echo "Building Docker image: ${IMAGE_NAME}:${IMAGE_TAG}"
docker build -t "${IMAGE_NAME}:${IMAGE_TAG}" "$PROJECT_ROOT"

echo ""
echo "Build complete."
docker images "${IMAGE_NAME}:${IMAGE_TAG}" --format "  Image: {{.Repository}}:{{.Tag}}  Size: {{.Size}}"
