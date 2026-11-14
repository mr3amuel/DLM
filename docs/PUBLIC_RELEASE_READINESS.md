# DLM 0.1.0 final release preparation — 2026-11-14

**HOLD: not authorized for public release.** Local arm64 build/package and scoped
privacy checks pass after the corrections below. Owner approval and fresh
post-correction CI verification remain mandatory. No approval flags were changed.

## Evidence and corrections

A real production Release build and real DMG were created locally, mounted read-only,
verified and removed through the existing local inspection path. Neither DLM nor its
browser helper was launched or installed. No user application data was accessed.

The first binary scan found embedded development home paths in both executables.
Production Rust builds now remap home, registry and repository paths to neutral
/dlm locations. Encoded flags preserve spaces and caller-supplied flags. The actual
rebuilt and mounted executables contain no matched personal-home/temp-path,
known local account/hostname, private-key or provider-token signatures. Packaging
now rejects high-confidence home/temp/key/token signatures in production executables.
This is a heuristic gate, not a guarantee that every possible secret is absent.

The existing workflow referenced a missing release-notes file. That file is now
provided, and the initial release gate requires it to be nonempty before costly
builds. No authorization condition was removed or altered.

The built and mounted app each contain exactly 296 expected files and two Mach-O
executables. Source resources and all 269 legal texts match by bytes. No symlinks,
extra executables, bundled dynamic libraries or DWARF sections were found. System
and Swift dynamic links are checked. Browser manifests use the production extension
key, Firefox ID and host identity. Public extension keys are not pairing secrets;
pairing data is runtime-generated and not part of the image.

DMG metadata/background passed the private path/credential-signature checks. macOS
com.apple.provenance extended attributes (11-byte opaque system records) remain on
the files. No personal-text signature was found in them; they were not independently
decoded or stripped. The owner should include this finding in final metadata review.
No quarantine/Gatekeeper/security attributes were automatically removed.

## Rights and distribution

The source-only licensing review is not binary publication approval. Actual resolved
arm64 dependency versions and license expressions match the reviewed inventory;
all notices accompany the app. The three installed packaging tools are MIT and
build-only, not bundled. No new DLM license was selected. The owner must confirm
binary distribution and intended recipient permissions under SOURCE_AVAILABILITY.md,
and accept the scoped third-party/asset/metadata review before enabling uploads.

## Workflow behavior and remaining approvals

- All four policy booleans remain false: publication_approved,
  source_availability_reviewed, third_party_licenses_reviewed,
  history_and_assets_reviewed. No ownership/approval field was edited.
- Release build jobs require the repository approval variable and committed preflight.
  Version tags must match 0.1.0. Production identity remains com.dlm.native, build 1.
- Build jobs upload the exact DMGs/checksums before draft-release approval. Actions
  artifacts in a public repository are potentially publicly downloadable; owner
  authorization must cover them, not only the final GitHub Release.
- Draft creation needs manual dispatch from main, create_draft, a separate repository
  variable, configured environment rules and the exact version tag at the same commit.
  Exact downloaded filenames/checksums are checked before gh release create --draft.
- Required reviewers/branch rules and Private Vulnerability Reporting are GitHub
  settings and were not inspected or configured locally. An environment name alone
  does not establish reviewer protection. Final Publish is a human action.

The owner reports successful development, production and DMG CI runs on both
architectures before these corrections. This review verified only arm64 locally;
rerun production/package verification on both architectures after committing the
privacy changes. No real x86_64 artifact was inspected in this local review.

Ad-hoc signatures are not Developer ID or notarization. Downloaded-app Gatekeeper,
macOS 13 runtime, Finder presentation, actual installation, folder permissions and
browser handoff need manual acceptance or explicit acceptance of known limitations.
No automatic updater or separate background download daemon exists. Closing a
window can leave the Menu Bar process active; Quit stops and checkpoints downloads.

No tag, commit, push, release, upload or owner-approval change was performed by
this preparation. The final decision is HOLD until the explicit approvals and
post-correction evidence are complete.
