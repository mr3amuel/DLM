# DLM 0.1.0 final metadata and binary-license investigation — 2026-11-14

## Scope and evidence

This follow-up resolves the previously opaque provenance finding and checks binary
redistribution terms. It preserves the earlier source/history review in
PRIVACY_REVIEW.md rather than repeating it. Only this independent public repository
and its actual build outputs/dependency cache were inspected. Existing owner rights,
free official binary use and acceptance of ad-hoc signing are not re-requested.
No policy flag is changed by this investigation. No app/helper was launched or installed.

The existing production arm64 app was inspected first. A real disposable DMG was
created and mounted through the existing inspect-local machinery. After finding and
correcting missing ring notices, the production arm64 Release app was rebuilt and
real DMG inspection repeated. The owner reports both architectures passed prior
production and packaging CI at f9bf207; that evidence predates the supplemental notice.

## Metadata results

| Category | Actual evidence | Finding and uncertainty |
| --- | --- | --- |
| Extended attributes | Native listxattr/getxattr APIs enumerated every bundle/mounted entry; xattr was also consulted. Rebuilt bundle: 472 entries; mounted image: 476 entries, 475 carrying provenance. | Only com.apple.provenance was present. No quarantine, resource-fork, FinderInfo, MACL or WhereFroms attribute was observed. Attribute values were not printed in reports. |
| Provenance | All present values are identical 11-byte records: three-byte prefix 010200 and eight nonzero identifier bytes. Values were structurally inspected and scanned for paths, identity strings, email/key/token patterns and known local identity markers. | No direct username, home/temp path, hostname, email or credential was found. The identifier is a local provenance-tracking key, not evidence of a hardware/machine ID. It can support correlation with a system's private provenance database; it is not treated as anonymous merely because it is binary. |
| AppleDouble | Mounted tree and bundle checked for ._ files. | None found. |
| Finder presentation | Six actual .DS_Store records decoded using ds-store, including icon positions and window/icon-view property lists; binary alias fields inspected where present. | No matched private path, identity or credential. Finder presentation metadata is present; a com.apple.FinderInfo xattr is not. |
| Quarantine | Extended attributes enumerated without modification. | Absent on this locally built artifact. Public downloads may gain quarantine on the recipient Mac. Nothing was cleared or bypassed. |
| Signing | Real codesign --verify --deep --strict and signature detail inspection. | Ad-hoc production identity com.dlm.native; no signing team/certificate identity or trusted notarization timestamp. The resource seal remains intact. No signing/security metadata was removed. |
| Build metadata | Actual Info.plist, executable architecture/dynamic links and existing bundle privacy guards checked by the common packager. | arm64, production 0.1.0/build 1; system/Swift dynamic libraries only. Neutral /dlm source remaps remain. Normal binary/build identifiers and filesystem timestamps are not claimed to identify a developer. |
| Spotlight | mdls queried the actual app. | No Spotlight record was available for the uninstalled local build; absence of index results is not proof about future indexing. |

Research basis: [Koh Nakagawa's original Black Hat paper, Appendix B](https://i.blackhat.com/BH-USA-25/Presentations/USA-25-Koh-XUnprotect-Reverse-Engineering-macOS-XProtect-Remediator-wp.pdf)
describes an eight-byte random ExecPolicy provenance-table key and an undocumented
three-byte portion. This is reverse-engineering evidence, not an Apple stable-format
specification. The observed shape agrees with that account; this audit did not read
any private system provenance database or prove a mapping for this particular key.
The prefix's complete semantics remain unresolved. No attribute is stripped simply
to make the scan clean. No concrete metadata correction or release blocker was found;
the owner should accept the limited correlation/undocumented-field uncertainty.
A negative string scan alone would not justify this conclusion.

## Actual production dependency inventory

Locked, offline Cargo metadata was resolved separately for the adapter and production
browser helper on aarch64-apple-darwin and x86_64-apple-darwin. Traversal starts at the
actual production roots, excludes dev-only edges, separates build/proc-macro closure
from normal runtime dependencies, and checks declared expressions against the cached
package manifests and committed inventory.

| Graph scope | arm64 | x86_64 |
| --- | ---: | ---: |
| Potential normal runtime/helper packages | 118 | 119 |
| Additional build/proc-macro packages | 25 | 25 |
| Total production normal/build closure | 143 | 144 |

Intel additionally includes cpufeatures 0.3.1 (MIT). These are graph-based potential
linkage counts, not a claim that all symbols survive optimization. The reviewed
149-package superset additionally retains test-only notices. No package/version or
declared expression is missing/mismatched. No license metadata is unknown. No
selected GPL/LGPL/AGPL/MPL/EPL/CDDL/SSPL or other reciprocal source-disclosure license
was found. Static linkage of the selected permissive components imposes notice/data
conditions, not a requirement to publish DLM's proprietary implementation.

The full inventory selects MIT for 122 packages, Unicode-3.0 for 18, ISC for two,
Apache-2.0 for two, and one each of BSD-3-Clause, MIT AND BSD-3-Clause,
Apache-2.0 AND ISC, MIT AND Unicode-3.0 and CDLA-Permissive-2.0. Alternatives are
explicit selections, not cumulative obligations; AND expressions remain cumulative.

## Cases requiring closer examination

| Package/version | Production/distribution and terms | Finding/correction |
| --- | --- | --- |
| ring 0.17.14 | Crypto dependency statically incorporated through the native engine; Apache-2.0 AND ISC plus preserved nested once_cell/fiat terms. | Concrete gap: top-level legal files omitted some file-level permissive copyrights, including Trent Clarke, David Judd, Simon Sapin and Intel. Added SOURCE-NOTICES.txt: 42 distinct complete legal headers from 166 upstream source files, with relative source references; no implementation copied. Inventory now requires this resource. Existing BoringSSL/once_cell/fiat legal files remain. |
| webpki-roots 1.0.10 | Runtime certificate-root data, CDLA-Permissive-2.0. | Agreement accompanies the data in app notices; no DLM-wide source obligation. |
| ICU/Unicode packages (exact 18 versions in RUST_DEPENDENCIES.json) | Unicode-3.0 code/data; runtime or proc-macro according to graph. | Copyright, permission, disclaimer texts retained byte-for-byte. |
| unicode-ident 1.0.27 | Proc-macro/build closure; (MIT OR Apache-2.0) AND Unicode-3.0, selecting MIT AND Unicode-3.0. | Both cumulative texts preserved, even though not counted as a runtime library. |
| chrono-tz 0.10.4 | Runtime MIT implementation and generated timezone data. | Own legal text and tz/LICENSE retained; timezone public-domain statement and BSD-derived exceptions are not replaced by DLM terms. |
| libsqlite3-sys 0.30.1 / rusqlite 0.32.1 | Runtime static bundled SQLite; MIT bindings and SQLite public-domain amalgamation. | bundled/bundled_bindings features observed; no SQLCipher feature. SQLCipher notice is retained conservatively without claiming that implementation ships. |
| ryu 1.0.23 / sync_wrapper 1.0.2 | Runtime Apache-2.0; ryu offers Apache OR BSL and Apache is selected. | License/available upstream attribution retained. No separately supplied NOTICE file was omitted. |
| rustls-webpki 0.103.16 / untrusted 0.9.0 / subtle 2.6.1 | Runtime static ISC / ISC / BSD-3-Clause. | Copyright, conditions, disclaimers and non-endorsement terms retained. |
| matchit 0.8.4 | Dev/test-only MIT AND BSD-3-Clause in this production graph. | Both texts retained; not treated as evidence of a shipped production web server. |

All recognizable published LICENSE/LICENCE/COPYING/COPYRIGHT/NOTICE/AUTHORS files
in the production normal/build packages were compared with preserved files, with
no missing or modified legal file found. The file-header investigation caught a gap
that filename-based checks alone missed; top-level license metadata is insufficient.
The new ring supplement was checked against exact cached upstream headers.
No third-party implementation patch or Cargo patch override is introduced.

The corrected built and mounted app each contain the inventory and 270 legal texts,
all matching source bytes. The full 149-package conservative inventory is retained.
System frameworks, system fonts and Swift runtime libraries are dynamically resolved
from macOS, not separately copied as bundled frameworks/dylibs. No SwiftPM/CocoaPods
or downloaded font/icon pack is used by this native build. DLM artwork and browser
extension sources are project components covered by the owner's existing decision.
The packaging renderer uses that artwork. MIT packaging Python tools are build-only,
not inside the image. Public browser keys/allowlists are not pairing secrets.

Apache obligations were assessed against [the upstream license](https://apache.org/licenses/LICENSE-2.0.html),
and certificate-data sharing against [CDLA-Permissive-2.0](https://cdla.dev/permissive-2.0/).
No applicable upstream NOTICE text is missing. Keep all legal resources with future
binaries, preserve third-party permissions over proprietary DLM restrictions, avoid
endorsement/trademark claims, and identify future third-party modifications as required.
Source-available DLM terms expressly preserve third-party rights; free download/use
is not offered as an open-source grant for proprietary source.

**Binary redistribution classification: PASS — no unresolved material redistribution
issue identified after the ring correction**, for the reviewed inventory and contents.
This is an evidence-based technical license assessment, not a legal warranty. Intel's
updated final artifact still needs normal CI validation; its dependency terms and
architecture-independent notice inclusion were reviewed locally.

## Validation and recommendation before subsequent owner acceptance

- Production Release arm64 rebuild: passed using the existing build.sh, production,
  non-sandbox configuration and temporary Cargo targets. No algorithm/identity change.
- Actual DMG creation, integrity verification, read-only mount, contents/background,
  production identity/architecture, strict signatures, resources/legal files, SHA-256
  generation/validation, detach and disposable cleanup: passed after correction.
- 20 focused release regression tests pass, including required ring supplement
  attributions and all existing approval/failure/cleanup checks. Mocked unit results
  are separate from the real DMG evidence above.
- Existing app/helper were never run. No installed app, browser registration or
  application data was modified. Temporary diagnostics were not uploaded.
- Policy flags, production build script, packaging implementation, release workflow
  and source-availability terms are unchanged by this follow-up.

The evidence supports the owner approving third_party_licenses_reviewed after
reviewing this correction and validating the updated candidate on both CI runners.
It also supports history_and_assets_reviewed in conjunction with the existing
PRIVACY_REVIEW.md history/source/artwork evidence, provided the owner accepts the
explicit provenance-format/correlation limitation. No new ownership confirmation is
needed. This is not a new exhaustive history audit or a claim of Intel execution.
Neither flag is set here. Publication/Actions-upload authorization and GitHub controls
remain separate and unchanged; these findings do not authorize distribution.

## Subsequent explicit owner acceptance

The owner has accepted this third-party review including the corrected ring notices,
and the source-history/assets/privacy review including the residual provenance
uncertainty. The owner also authorizes potentially public Actions binary artifacts
before a draft/published release. The four local policy flags are now true; the
earlier audit's no-policy-change statements describe that investigation, not this
subsequent approval step. No legal warranty, notarization or installation/runtime
acceptance is asserted. Corrected-candidate Intel packaging still requires CI. No
GitHub variable or environment setting was changed and no release was published.
