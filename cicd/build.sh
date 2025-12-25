#!/usr/bin/env bash

echo "CI_PROJECT_DIR: $CI_PROJECT_DIR"

# 获取当前执行脚本的绝对路径
SCRIPT_PATH=$(readlink -f "$0")
# 获取当前执行脚本所在目录的绝对路径
SCRIPT_DIR=$(dirname "$SCRIPT_PATH")
# 获取脚本文件名
SCRIPT_NAME=$(basename "$SCRIPT_PATH")
echo "SCRIPT_NAME: $SCRIPT_NAME, SCRIPT_PATH: $SCRIPT_PATH, SCRIPT_DIR: $SCRIPT_DIR"

source "$SCRIPT_DIR/discover_services.sh"

cd "$SCRIPT_DIR"

if [ -z "$SERVICES" ]; then
  echo "请提供一个字符串。用法: $0 string"
  exit 1
fi

IFS=',' read -ra SERVICES_ARRAY <<<"$SERVICES"

set -eu

# 将特定逻辑抽象到一个函数中
do_specific_before_action() {
  local ENV="$1"
  local SERVICE="$2"

  echo "SERVICE_ABSOLUTE_DIR: $SERVICE_ABSOLUTE_DIR"
  cd "$SERVICE_ABSOLUTE_DIR"
  mkdir -p build
  cp -r "$CI_PROJECT_DIR/common/dal/sql/" build/ || true
}

ACTION=build
for SERVICE in "${SERVICES_ARRAY[@]}"; do
  echo "============== $ACTION $SERVICE ..... =============="
  cd "$SCRIPT_DIR" && SERVICE_ABSOLUTE_DIR=$(resolve_service_absolute_dir "$SERVICE")
  echo "SERVICE: $SERVICE, SCRIPT_DIR: $SCRIPT_DIR, SERVICE_ABSOLUTE_DIR: $SERVICE_ABSOLUTE_DIR, PWD: $PWD"
  source "$SCRIPT_DIR/load_env.sh" "$SERVICE_ABSOLUTE_DIR"
  do_specific_before_action "$ENV" "$SERVICE"
  cd "$SERVICE_ABSOLUTE_DIR/cicd" && ./"$ACTION".sh
  echo "============== $ACTION $SERVICE completed =============="
done
