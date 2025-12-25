#!/usr/bin/env bash

# 此脚本用于自动发现SERVICES和ENV

# 通用的处理service绝对路径的函数, 供 build.sh, deploy.sh 脚本使用. 需被其它脚本使用, 因此放在最前面, 避免被跳过
resolve_service_absolute_dir() {
  local SERVICE="$1"
  local SERVICE_DIR
  SERVICE_DIR=$(awk -v k="$SERVICE" '$1 == k { print $2; exit }' service_to_dir.properties)
  if [ -z "$SERVICE_DIR" ]; then
    SERVICE_DIR="$SERVICE"
  fi
  echo "$CI_PROJECT_DIR/$SERVICE_DIR"
}

#------在SERVICES已设定或分支不符时直接return 0, 父shell仍继续执行------
# 如果SERVICES已经存在, 则无需再做后续自动发现, 直接返回(父shell仍继续执行)
# 若SERVICES存在, 则表明是在devops-environment内执行deploy脚本, 无需再自动发现所有SERVICE_NAME组成的SERVICES了, 且ENV也是已知的
if [[ -n "$SERVICES" ]]; then
  echo "Ignore discover services, SERVICES: $SERVICES"
  return 0
fi

# 分支校验, 若不在指定分支则跳过
case "$CI_COMMIT_BRANCH" in
dev | testd | teste | testf | staging | production) ;;
*)
  echo "Ignore discover services, CI_COMMIT_BRANCH: $CI_COMMIT_BRANCH"
  return 0
  ;;
esac
#------在SERVICES已设定或分支不符时直接return 0, 父shell仍继续执行------

# 声明一个存储SERVICE_NAME的数组
SERVICE_ARRAY=()

# 获取当前执行脚本的绝对路径
SCRIPT_PATH=$(readlink -f "$0")
# 获取当前执行脚本所在目录的绝对路径
SCRIPT_DIR=$(dirname "$SCRIPT_PATH")
# 获取脚本文件名
SCRIPT_NAME=$(basename "$SCRIPT_PATH")
echo "SCRIPT_NAME: $SCRIPT_NAME, SCRIPT_PATH: $SCRIPT_PATH, SCRIPT_DIR: $SCRIPT_DIR"

function process_service() {
  local dir="$1"
  if [[ ! -f "$SCRIPT_DIR/load_env.sh" ]]; then
    echo "Warning: $SCRIPT_DIR/load_env.sh not found."
    return 0
  fi
  source "$SCRIPT_DIR/load_env.sh" "$dir"
  if [[ -n "${SERVICE_NAME}" ]]; then
    SERVICE_ARRAY+=("${SERVICE_NAME}")
  fi
}

# 首先检查当前目录下的.gitlab-ci.yml
if [[ -f ".gitlab-ci.yml" ]]; then
  echo "processing .gitlab-ci.yml on current directory: $PWD"
  process_service "."
fi

# 再遍历当前目录下的所有子目录，对存在.gitlab-ci.yml的目录执行处理
for d in */; do
  if [[ -f "${d}.gitlab-ci.yml" ]]; then
    echo "processing .gitlab-ci.yml on ${d%/}"
    process_service "${d%/}"
  fi
done

# 将SERVICE_ARRAY转换为逗号分隔的字符串并导出
SERVICES=$(
  IFS=,
  echo "${SERVICE_ARRAY[*]}"
)
export SERVICES
export ENV="$CI_COMMIT_BRANCH"
echo "discovered services on ENV: $ENV, SERVICES: $SERVICES"
