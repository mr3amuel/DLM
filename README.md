# DLM for macOS

Native SwiftUI/AppKit download manager with a shared Rust engine. macOS 13+, Apple
Silicon or Intel. Downloads run only while DLM is running. Includes safe recovery,
priority queues, categories, schedules, browser link review and Menu Bar controls.

No public release or installation acceptance is implied by this source snapshot.
See SOURCE_AVAILABILITY.md, SECURITY.md, THIRD_PARTY_NOTICES.md and docs/RELEASE.md.

## Build

Install Xcode command-line tools, Python 3 and the pinned Rust toolchain, then:

```sh
DLM_NATIVE_CONFIGURATION=production DLM_NATIVE_SANDBOX=0 ./prototypes/swiftui/build.sh release
```

The result is prototypes/swiftui/build/DLM.app. Build on matching arm64 or x86_64
macOS hardware. Development builds use DLM_NATIVE_CONFIGURATION=development and
retain separate identities. This repository intentionally retains the native source
paths to keep C ABI source includes and engine dependencies stable.

## Tests

Use ./prototypes/swiftui/test_lifecycle.sh for focused native checks without launching
the application, python3 scripts/release/test_release.py for release safeguards,
and node --test browser-extension/tests/*.test.cjs for extension unit checks.
Rust core tests are in crates/download_core/tests; the native C ABI adapter has its
own tests under prototypes/swiftui/bridge. Full builds/GUI testing are separate.

## Local storage

Production uses com.dlm.native for the app/preferences identity and
~/Library/Application Support/com.dlm.native for its Engine database, recovery state,
bookmarks and authenticated browser inbox. Replacing a closed DLM.app does not reset
these. Permission errors are surfaced; there is no alternate empty database fallback.
Development uses com.dlm.native-development and a separate data root. No history
import, automatic updater or background daemon is included.

Browser setup is optional and explicit, using com.dlm.native.chrome. Chrome/Edge
require user installation and pairing consent; no cookies or authorization headers
are forwarded by default. Firefox permanent extension installation requires Mozilla
signing. Browser registration is never performed by a build.
