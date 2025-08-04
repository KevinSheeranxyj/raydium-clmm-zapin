#!/usr/bin/env bash

set -eux

# 定义一个清理函数
cleanup() {
  echo "Cleaning up..."
  rm -f temp.tpl
}
# 设置 trap，无论脚本以何种方式退出，都会执行 cleanup 函数
trap cleanup EXIT

# for local debugging (by declare ENV, SERVICE_NAME and IMAGE manually)
set +eu
if [[ -n "$ENV" ]]; then
  echo "ENV has been set to $ENV"
  export ENV
else
  set -eux
  export ENV=${CI_COMMIT_BRANCH##*-}
fi
set +eu
if [[ -n "$IMAGE" ]]; then
  echo "IMAGE has been set to $IMAGE"
  export IMAGE
else
  set -eux
  IMAGE_NAME=${SERVICE_NAME}
  IMAGE_TAG=${SERVICE_VERSION}-${CI_COMMIT_REF_SLUG}-${CI_COMMIT_SHORT_SHA}
  export IMAGE=${REGISTRY_URL}/${IMAGE_NAME}:${IMAGE_TAG}
fi

CONFIG_FILE=kubernetes/config.json
NAMESPACE=$(jq -r ".${ENV}.namespace" $CONFIG_FILE)
export NAMESPACE
ENV_CONSUL_PREFIX=$(jq -r ".${ENV}.consulPrefix" $CONFIG_FILE)
export ENV_CONSUL_PREFIX
EFS_HOST=$(jq -r ".${ENV}.efsHost" $CONFIG_FILE)
export EFS_HOST
EFS_ENV_PREFIX=$(jq -r ".${ENV}.efsEnvPrefix" $CONFIG_FILE)
export EFS_ENV_PREFIX
IS_INIT=$(jq -r ".isInit" $CONFIG_FILE)
REPLICAS=$(jq -r ".replicas" $CONFIG_FILE)
export REPLICAS
POD_MANAGEMENT_POLICY=$(jq -r ".podManagementPolicy" $CONFIG_FILE)
export POD_MANAGEMENT_POLICY

echo "start to deploy $SERVICE_NAME on namespace $NAMESPACE in $ENV"

if [ "$IS_INIT" = "true" ]; then
  echo "Initial service setup."
  cd kubernetes && ./init.sh "$ENV"
else
  echo "IS_INIT is not set to true - skipping initial setup."
fi
cd ../

echo "start to render and apply application.yaml.tpl"
j2 kubernetes/application.yaml.tpl >temp.tpl
j2 -f=json temp.tpl $CONFIG_FILE | tee /dev/stderr | kubectl -n "$NAMESPACE" apply -f -
rm temp.tpl

# 脚本结束时，trap 将调用 cleanup 函数，从而执行 rm temp.tpl
