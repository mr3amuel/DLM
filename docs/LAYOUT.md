# Native source boundary

- crates/download_core: shared engine, SQLite, queue, recovery and deterministic tests.
- crates/download_bridge: browser inbox/protocol/host/setup only; binary-only crate.
- prototypes/swiftui/Sources: complete native application and lifecycle, without the
  experimental fixture entry point. In-memory timing API remains for shell tests.
- prototypes/swiftui/bridge and include: versioned native C ABI adapter and ownership.
- prototypes/swiftui/Resources and scripts: original native logo/icon, localization,
  native-only accent generation. No other UI application's assets are required.
- prototypes/swiftui/Tests: native shell/FFI/workflow checks.
- browser-extension: authenticated extension/setup sources, synthetic tests and public keys.
- scripts/release and .github/workflows: explicitly gated builds, DMGs and integrity checks.
- third-party: dependency license text/metadata when available, never runtime binaries.
- docs: public native build, security and release guidance only.

SOURCE_MANIFEST.json enumerates every managed file and purpose. Snapshot refresh is
performed locally from the authoritative repository using the separate export control
directory. No development history, credentials, caches, logs or application databases
belong in this snapshot. Public history uses only the independently configured approved public name and
verified noreply identity; private development history is never imported.
