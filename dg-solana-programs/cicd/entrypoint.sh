#!/usr/bin/env bash

set -eux

# Check if render_config.py exists and is executable
RENDER_CONFIG_PYTHON_PATH=/usr/local/bin/render_config.py
if [[ -f $RENDER_CONFIG_PYTHON_PATH ]]; then
  echo "start to render config from consul by Python"
  python3 $RENDER_CONFIG_PYTHON_PATH
else
  echo "start to render config by j2 CLI"
  render_config.sh
fi

# specific
#------some scripts------
# ......
#------some scripts------

# 创建输入输出设备
if [ ! -p /log/fifo ]; then
  mkfifo /log/fifo
else
  echo "The FIFO file already exists."
fi
# 运行multilog 从FIFO设备读入并滚动落盘
nohup multilog s16777215 n10 /log/app/ </log/fifo &
# 运行容器主程序，将标准输出和错误输出到管道，再由tee发向标准输出和FIFO设备
# specific
exec "$APP_HOME"/main --conf="/etc/app/application.toml" 2>&1 | tee /dev/fd/2 >/log/fifo
