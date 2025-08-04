#!/usr/bin/env bash

set -eux

# 服务初始化脚本，仅首次部署时执行。当 config.json 配置为 `"isInit": true` 时，会触发执行
NAMESPACE=$(jq -r ".${ENV}.namespace" config.json)
export NAMESPACE

#------uncomment for fixed IP------
#kubectl -n "$NAMESPACE" get svc "$SERVICE_NAME" -oyaml || true
#echo "start to delete service above to initialize it"
#kubectl -n "$NAMESPACE" delete svc "$SERVICE_NAME" || true
#------uncomment for fixed IP------
