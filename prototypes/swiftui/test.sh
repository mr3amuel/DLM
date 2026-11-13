#!/bin/sh
set -eu
cd "$(dirname "$0")"
project=$(pwd)
repo=$(cd ../.. && pwd)
. "$repo/scripts/env.sh"
target_dir="${CARGO_TARGET_DIR:-$project/build/rust}"
CARGO_TARGET_DIR="$target_dir" cargo test --locked --manifest-path bridge/Cargo.toml
CARGO_TARGET_DIR="$target_dir" cargo build --locked --release --manifest-path bridge/Cargo.toml
mkdir -p build
swiftc -O -module-cache-path build/unit-tests-module-cache -target "$(uname -m)-apple-macos13.0" \
    -import-objc-header include/dlm_experiment.h \
    $(printf '%s\n' Sources/*.swift | sed '/\/ExperimentApp.swift$/d') Tests/NativeUnitTests.swift \
    -L "$target_dir/release" -ldlm_swift_experiment \
    -framework Security -framework SystemConfiguration -liconv -lresolv \
    -o build/NativeUnitTests
build/NativeUnitTests
