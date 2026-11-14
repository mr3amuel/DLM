#!/bin/sh
set -eu
cd "$(dirname "$0")"
project=$(pwd)
repo=$(cd ../.. && pwd)
. "$repo/scripts/env.sh"
target_dir="${CARGO_TARGET_DIR:-$project/build/rust}"
case "${1:-release}" in
    release) profile=release; cargo_profile=--release; optimization=-O; name="DLM Native Development" ;;
    debug) profile=debug; cargo_profile=; optimization=-Onone; name="DLM Native Development Debug" ;;
    *) echo "Usage: $0 [debug|release]" >&2; exit 2 ;;
esac
configuration="${DLM_NATIVE_CONFIGURATION:-development}"
case "$configuration" in
    development) bundle_id=com.dlm.native-development; host=com.dlm.native_development.chrome; features=native-chrome; key=chrome-public-key.txt; firefox=browser-native@dlm.local ;;
    production) name=DLM; bundle_id=com.dlm.native; host=com.dlm.native.chrome; features=native-production; key=chrome-production-public-key.txt; firefox=browser-production@dlm.local
        [ "$profile" = release ] && [ "${DLM_NATIVE_SANDBOX:-0}" = 0 ] || { echo "Production requires Release, non-sandbox configuration" >&2; exit 2; } ;;
    *) echo "Unknown DLM_NATIVE_CONFIGURATION" >&2; exit 2 ;;
esac
# Production panic/source strings must not retain the build machine's home path.
# Encoded flags preserve repository paths containing spaces and existing flags.
if [ "$configuration" = production ]; then
    CARGO_ENCODED_RUSTFLAGS="$(python3 - "$repo" <<'PYFLAGS'
import os
import sys
separator = chr(31)
encoded = os.environ.get("CARGO_ENCODED_RUSTFLAGS")
flags = encoded.split(separator) if encoded else os.environ.get("RUSTFLAGS", "").split()
home = os.path.expanduser("~")
for source, replacement in [(home, "/dlm/build-user"),
                            (os.environ.get("CARGO_HOME", home + "/.cargo"), "/dlm/cargo"),
                            (sys.argv[1], "/dlm/source")]:
    flags.append("--remap-path-prefix=" + source + "=" + replacement)
print(separator.join(flags))
PYFLAGS
)"
    export CARGO_ENCODED_RUSTFLAGS
fi
architecture="${DLM_NATIVE_ARCH:-$(uname -m)}"
case "$architecture" in
    arm64) rust_target=aarch64-apple-darwin ;;
    x86_64) rust_target=x86_64-apple-darwin ;;
    *) echo "Unsupported architecture" >&2; exit 2 ;;
esac
# Fail closed: dependencies must be built on the matching architecture for now.
[ "$architecture" = "$(uname -m)" ] || { echo "Build on a matching $architecture macOS runner; cross compilation is not configured" >&2; exit 2; }
if [ "${DLM_NATIVE_SANDBOX:-0}" = 1 ]; then name="$name Sandbox"; fi
CARGO_TARGET_DIR="$target_dir" cargo build --locked $cargo_profile --target "$rust_target" --manifest-path bridge/Cargo.toml
bundle="$project/build/$name.app"
mkdir -p "$bundle/Contents/MacOS"
cp Info.plist "$bundle/Contents/Info.plist"
plutil -replace CFBundleIdentifier -string "$bundle_id" "$bundle/Contents/Info.plist"
if [ "$configuration" = production ]; then
    plutil -replace CFBundleName -string DLM "$bundle/Contents/Info.plist"
    plutil -replace CFBundleDisplayName -string DLM "$bundle/Contents/Info.plist"
    plutil -insert DLMProduction -bool true "$bundle/Contents/Info.plist"
    plutil -replace CFBundleShortVersionString -string "$(cat release-version.txt)" "$bundle/Contents/Info.plist"
    plutil -replace CFBundleVersion -string "$(cat release-build.txt)" "$bundle/Contents/Info.plist"
fi
plutil -insert DLMArchitecture -string "$architecture" "$bundle/Contents/Info.plist"
plutil -insert LSArchitecturePriority -json "[\"$architecture\"]" "$bundle/Contents/Info.plist"
swiftc "$optimization" -whole-module-optimization -target "${architecture}-apple-macos13.0" \
  -import-objc-header include/dlm_experiment.h Sources/*.swift \
  -L "$target_dir/$rust_target/$profile" -ldlm_swift_experiment \
  -framework Security -framework SystemConfiguration -liconv -lresolv \
  -o "$bundle/Contents/MacOS/DLMSwiftExperiment"
if [ "$profile" = release ]; then strip -x "$bundle/Contents/MacOS/DLMSwiftExperiment"; fi
resources="$bundle/Contents/Resources/DLMChromeExtension"
helper="$bundle/Contents/Helpers/dlm-native-chrome-host"
mkdir -p "$resources" "$(dirname "$helper")"
cp -R Resources/. "$bundle/Contents/Resources/"
notices="$bundle/Contents/Resources/ThirdPartyNotices"
mkdir -p "$notices"
cp -R "$repo/third-party/." "$notices/"
cp "$repo/THIRD_PARTY_NOTICES.md" "$repo/SOURCE_AVAILABILITY.md" "$notices/"
python3 scripts/export_brand_colors.py Sources/Design.swift "$project/build/BrandColors.xcassets"
xcrun actool --compile "$bundle/Contents/Resources" --platform macosx \
  --minimum-deployment-target 13.0 --output-partial-info-plist "$project/build/brand-colors-info.plist" \
  "$project/build/BrandColors.xcassets"
# Native branding is a required source resource, independent of UI frameworks.
cp -R "$repo/browser-extension/shared/." "$resources/"
cp "$repo/browser-extension/chromium/manifest.json" "$resources/manifest.json"
plutil -insert key -string "$(cat "$key")" "$resources/manifest.json"
plutil -replace name -string "$name (Review Links)" "$resources/manifest.json"
sed "s/com\\.dlm\\.browser/$host/" "$repo/browser-extension/shared/protocol.js" > "$resources/protocol.js"
firefox_resources="$bundle/Contents/Resources/DLMFirefoxExtension"
mkdir -p "$firefox_resources"
cp -R "$repo/browser-extension/shared/." "$firefox_resources/"
cp "$repo/browser-extension/firefox/manifest.json" "$firefox_resources/manifest.json"
plutil -replace name -string "$name (Review Links)" "$firefox_resources/manifest.json"
plutil -replace browser_specific_settings.gecko.id -string "$firefox" "$firefox_resources/manifest.json"
cp "$resources/protocol.js" "$firefox_resources/protocol.js"
host_target="${DLM_NATIVE_HOST_TARGET_DIR:-$project/build/chrome-rust}"
CARGO_TARGET_DIR="$host_target" cargo build --locked --release --manifest-path "$repo/Cargo.toml" \
  -p download_bridge --target "$rust_target" --features "$features" --bin dlm-browser-host
install -m 755 "$host_target/$rust_target/release/dlm-browser-host" "$helper"
lipo "$helper" -verify_arch "$architecture"
lipo "$bundle/Contents/MacOS/DLMSwiftExperiment" -verify_arch "$architecture"
codesign --force --sign - "$helper"
if [ "${DLM_NATIVE_SANDBOX:-0}" = 1 ]; then
    plutil -insert DLMScopedFolders -bool true "$bundle/Contents/Info.plist"
    codesign --force --sign "${DLM_NATIVE_SIGN_IDENTITY:--}" --entitlements Sandbox.entitlements "$bundle"
else
    codesign --force --sign - "$bundle"
fi
codesign --verify --deep --strict "$bundle"
printf '%s\n' "$bundle"
