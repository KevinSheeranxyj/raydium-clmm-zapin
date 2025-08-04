#!/usr/bin/env bash

set -eux

IMAGE_NAME=${SERVICE_NAME}
IMAGE_TAG=${SERVICE_VERSION}-${CI_COMMIT_REF_SLUG}-${CI_COMMIT_SHORT_SHA}
IMAGE=${REGISTRY_URL}/${IMAGE_NAME}:${IMAGE_TAG}

docker login --username="${REGISTRY_USERNAME}" --password="${REGISTRY_PASSWORD}" "${REGISTRY_URL}"

if docker manifest inspect "$IMAGE" >/dev/null 2>&1; then
  echo "docker image exists, skip build"
  exit 0
fi

echo 'start to build image'
export DOCKER_DEFAULT_PLATFORM=linux/amd64
echo "current dir: $(pwd)"
./build_binary.sh
echo "current dir: $(pwd)"
docker build --pull -t "${IMAGE}" ../build
docker push "${IMAGE}"
