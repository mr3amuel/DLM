# DLM 0.1.0

Free to download and use. Proprietary source is source-available, not open-source licensed.

Native SwiftUI/AppKit download manager for macOS with a Rust engine: resumable
transfers, queue priorities, scheduling, categories, browser link review and Menu Bar
controls. Browser integration is optional and requires explicit setup and consent.

## Choose your download

Requires macOS 13 or later.

- Apple Silicon (M-series): DLM-0.1.0-macOS-arm64.dmg
- Intel: DLM-0.1.0-macOS-x86_64.dmg

Compare the download against its SHA-256 sidecar. A matching checksum detects byte
changes; it does not independently establish publisher identity or safety.

## Installation and first launch

Quit any running DLM instance normally before replacing it. Open the DMG and drag
DLM.app to Applications only when ready to replace an existing app with that name.

This build is ad-hoc signed, not Developer ID signed or notarized by Apple. macOS
may block first launch because the developer cannot be verified. Inspect the source
and decide whether you trust the download before proceeding. Where macOS permits
it, attempt opening the app, then use System Settings > Privacy & Security > Open
Anyway for this app and confirm the next prompt. Availability depends on macOS and
organization policy; this is not a promise that every warning can be overridden.
Do not override malware/damaged-app warnings or disable Gatekeeper globally.
See Apple's current guidance: https://support.apple.com/102445 .

There is no automatic updater. Install later versions manually after quitting DLM.
Download state and recovery metadata remain outside the application bundle. Folder
permissions may need renewed consent after ad-hoc rebuilds.

Downloads run only while the DLM application process is running. Closing the main
window may keep its Menu Bar process active; Quit DLM to stop it and checkpoint
transfers. There is no separate background daemon continuing downloads after Quit.

## Rights and browser limitations

The owner confirms distribution rights for DLM's proprietary code, name and artwork.
Public source viewing grants no permission to copy, modify or redistribute that
proprietary source. See SOURCE_AVAILABILITY.md;
third-party licenses remain separate and are bundled under
DLM.app/Contents/Resources/ThirdPartyNotices. Do not infer redistribution rights from
the availability of these downloads. Chrome/Edge setup uses developer-mode loading;
permanent Firefox extension installation requires Mozilla signing. No browser
installation or pairing occurs automatically during app installation.
