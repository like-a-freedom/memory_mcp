#!/usr/bin/env bash
# ort-sys rc.12 has no Intel macOS prebuilt. Keep this at its ONNX API version.
set -euo pipefail
repo_root=$(pwd)
source_dir=$(mktemp -d)
trap 'rm -rf "$source_dir"' EXIT
git -C "$source_dir" init -q
git -C "$source_dir" remote add origin https://github.com/microsoft/onnxruntime.git
git -C "$source_dir" fetch --depth 1 origin 058787ceead760166e3c50a0a4cba8a833a6f53f
git -C "$source_dir" checkout --detach FETCH_HEAD
cd "$source_dir"
bash build.sh --config Release --build_shared_lib --parallel 3 --skip_tests \
  --compile_no_warning_as_error \
  --cmake_extra_defines CMAKE_OSX_ARCHITECTURES=x86_64 onnxruntime_BUILD_UNIT_TESTS=OFF
mkdir -p "$repo_root/.ci/onnxruntime/lib"
cp -L build/MacOS/Release/libonnxruntime*.dylib "$repo_root/.ci/onnxruntime/lib/"
cp LICENSE ThirdPartyNotices.txt "$repo_root/.ci/onnxruntime/"
