#!/usr/bin/env bash

set -eux

CODE_DIR_ABSOLUTE_PATH=$(readlink -f "$(pwd)/..")
BUILD_DIR=$(readlink -f "$CODE_DIR_ABSOLUTE_PATH/build")

echo "当前目录: $(pwd)"
echo "代码目录: $CODE_DIR_ABSOLUTE_PATH"
echo "构建目录: $BUILD_DIR"

cp -rf Dockerfile config/ entrypoint.sh "$BUILD_DIR"

echo "======复制源代码到构建目录之前======"
ls "$BUILD_DIR"

# 使用更安全的复制方式
rsync -avhP --exclude 'build' --exclude '.git' --exclude 'cicd' --exclude 'cicd_wrapper' "$CODE_DIR_ABSOLUTE_PATH/" "$BUILD_DIR/"

echo "======复制源代码到构建目录之后======"
ls "$BUILD_DIR"

cd "$BUILD_DIR"
echo "当前目录: $(pwd)"

# specific
cargo build --release
# TODO please update build scripts accordingly
# anchor build

# unit tests
tsc --version
# TODO please update unit tests scripts accordingly
# cd unit-tests
# cmd...
# node ts
