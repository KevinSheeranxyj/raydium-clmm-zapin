#!/usr/bin/env bash

# 参数检查
if [ $# -lt 1 ]; then
  echo "Usage: $0 ENV [IP_PREFIX]"
  exit 1
fi

# 接收参数
ENV=$1
IP_PREFIX=$2

# 获取ENV ID
case $ENV in
"dev")
  ENV_ID=3
  ;;
"testd")
  ENV_ID=1
  ;;
"teste")
  ENV_ID=2
  ;;
*)
  ENV_ID=0
  ;;
esac

# 检查ENV是否有效
if [ -z "$ENV_ID" ]; then
  echo "Invalid ENV. Must be one of production, staging, dev, testd, teste, testf."
  exit 1
fi

# 计算VALUE
VALUE=$((255 - ENV_ID * 40 - IP_PREFIX))

# 设置IP
if [ -z "$IP_PREFIX" ]; then
  IP="None"
else
  IP="10.43.$VALUE.1"
fi

# 输出结果
# echo "ENV_ID: $ENV_ID"
# echo "VALUE: $VALUE"
echo $IP
