#!/bin/sh
# Focused deterministic checks only: no Rust build, GUI launch or application data.
set -eu
cd "$(dirname "$0")"
mkdir -p build/lifecycle-tests
xcrun clang -target "$(uname -m)-apple-macos13.0" -Wall -Wextra -Werror -c Tests/LifecycleFFI.c -o build/lifecycle-tests/ffi.o
swiftc -Onone -module-cache-path build/lifecycle-tests/module-cache -target "$(uname -m)-apple-macos13.0" \
    -import-objc-header Tests/lifecycle_ffi.h \
    $(printf '%s\n' Sources/*.swift | sed '/\/ExperimentApp.swift$/d') Tests/LifecycleTests.swift Tests/DownloadWorkflowChecks.swift Tests/DownloadListChecks.swift Tests/ScheduleWorkflowChecks.swift Tests/BrowserWorkflowChecks.swift \
    build/lifecycle-tests/ffi.o -o build/lifecycle-tests/LifecycleTests
build/lifecycle-tests/LifecycleTests
