# Initial source snapshot licensing review — 2026-11-14

**PASS for the reviewed source snapshot and local initial commit.** This is a
technical license/provenance review, not a warranty of legal compliance or approval
to publish a binary release. Release gates remain false and were not loosened.

## Scope and consistency

Offline locked Cargo metadata for both architectures and both workspaces matches
149 third-party package names/versions and their cached package manifests. Lockfile
checksums identify the registry packages. The engine's own source is unchanged;
third-party implementation sources/binaries are not vendored. Available root and nested legal
texts plus packaged AUTHORS/COPYRIGHT attributions are preserved. References to
author lists not present in the published crate are retained verbatim, not invented. Required DLM
branding is project-supplied original artwork, not newly sourced proprietary art;
no contrary embedded ownership claim was found. This review relies on the owner's
approval to use existing project assets, not an independent trademark/title search.

## Selected rights and obligations

RUST_DEPENDENCIES.json records a selected expression for every package. MIT is
selected where offered; the legacy Cargo MIT/Apache-2.0 notation is treated as the
upstream offered alternative and both texts remain. Apache-2.0 is selected for the
Apache/BSL alternative. AND terms remain cumulative, including ring's Apache-2.0
and ISC and other mixed MIT/BSD/Unicode packages. No selected expression is GPL,
LGPL, AGPL, MPL or another reciprocal source-disclosure license. No obligation to
license DLM's own source as open source was identified under these selections.

- MIT/ISC/BSD/Zlib: retain applicable copyright, license, conditions and disclaimers;
  binary distributions must carry the required notices/documentation. No endorsement
  or trademark permission is inferred. Mark changes if future third-party code is
  modified where required. Upstream notices are preserved, not replaced by summaries.
- Apache-2.0: preserve license and applicable attribution/NOTICE, identify modified
  third-party files if any, and respect patent/trademark provisions. No dependency
  implementation has been modified in this snapshot.
- Unicode-3.0: preserve the data/software license, notices and disclaimers with
  distributed copies. No advertising/endorsement permission is inferred.
- webpki-roots CDLA-Permissive-2.0: the certificate-root data's agreement accompanies
  the data through the app's bundled notice resources. The agreement requires this
  when sharing Data; it is not a DLM-wide source-disclosure condition.
- chrono-tz: retain its nested tz LICENSE. It declares timezone data public-domain
  except named BSD-derived code; the package's own permissive terms also remain.
- SQLite: libsqlite3-sys/rusqlite MIT binding notices remain. The bundled default
  SQLite source identifies itself as a source amalgamation with embedded original
  notices/public-domain material. SQLCipher is not enabled in the inspected features;
  its separately packaged license is nevertheless retained as a conservative notice.
- ring: top-level summaries are insufficient by themselves. Its BoringSSL, ISC,
  once_cell polyfill and fiat-crypto nested license/attribution files are retained.

These are notice/attribution and conditional modification duties, not an offer to
redistribute DLM under those licenses. SOURCE_AVAILABILITY.md remains the owner's
chosen no-general-grant policy. No root MIT/Apache/GPL project license is added.

## What is shipped and what is not

The inventory also records graph roles from Cargo normal/build/dev edges and proc-macro
targets across both architectures/workspaces: 119 packages are in the potential runtime/helper
closure and 30 are build/proc-macro or test-only. These roles describe the dependency
graph, not a claim that every symbol survives final linking. The notice resource
intentionally retains the full superset, so uncertain dead-code elimination never
removes a required attribution.


Rust libraries and bundled SQLite are statically incorporated into native executables;
the standalone browser helper and the WebExtension sources are DLM components.
The build now includes the conservative complete Rust notice inventory in the app,
and packaging verifies listed notice files. No runtime-dependent legal text may be
removed merely because it is not executable. System Apple frameworks/fonts are
referenced through the SDK/system and not copied into the source or DMG.

CI-only dmgbuild, ds-store and mac-alias execute during packaging but are not vendored
or shipped inside DLM.app/DMG. No redistribution of their implementation is authorized
by this snapshot. Their runner installation terms must be checked before activating
CI, separately from the reviewed source snapshot. SHA-pinned CI actions likewise
execute on runners but their implementation is not vendored or bundled. These future
execution/artifact checks are not a missing license for code copied into this commit.

## Future release checks

After the first authorized builds, confirm actual link/features and bundled notices,
including platform-conditional code and final archive contents. Repeat this review
for dependency/feature changes. Enabling CI/publication is a separate approval, not
implied by this initial source commit. Native distribution and installation acceptance
remain pending. Private Vulnerability Reporting will be enabled manually after
repository creation; documentation does not claim it is already enabled.
