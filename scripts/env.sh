#!/bin/sh
# Source this in project commands; no global locale or compiler changes.
export LANG=en_US.UTF-8
export LC_ALL=en_US.UTF-8
export CC="$(xcrun --find clang)"
export CXX="$(xcrun --find clang++)"
export LIBCLANG_PATH="$(xcode-select -p)/Toolchains/XcodeDefault.xctoolchain/usr/lib"
export PATH="$PWD/.tools/bin:$PATH"

export MACOSX_DEPLOYMENT_TARGET=11.0

export RUSTUP_TOOLCHAIN=1.94.0
