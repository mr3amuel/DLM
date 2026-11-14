# DLM 0.1.0 final release preparation — 2026-11-14

**OWNER APPROVALS RECORDED — candidate verification remains required.** All four
local policy flags are true. The owner explicitly accepted third-party redistribution
including the corrected ring notices, source-history/assets/privacy including the
provenance uncertainty, and potentially public Actions binary artifacts before any
draft/published release. No GitHub variable or setting was changed, and no release
has been created or published. This records decisions, not new runtime acceptance.

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

The earlier built and mounted app contained 296 expected files and two Mach-O
executables, including 269 matching legal texts. The subsequent ring correction adds
one legal resource; the corrected arm64 app and DMG contain 270 matching legal texts. No symlinks,
extra executables, bundled dynamic libraries or DWARF sections were found. System
and Swift dynamic links are checked. Browser manifests use the production extension
key, Firefox ID and host identity. Public extension keys are not pairing secrets;
pairing data is runtime-generated and not part of the image.

DMG metadata/background passed the private path/credential-signature checks. macOS
com.apple.provenance extended attributes (11-byte opaque system records) remain on
the files. The follow-up below characterizes the eight-byte local tracking identifier;
the three-byte prefix remains undocumented. The owner explicitly accepts that residual
uncertainty and possible local correlation. The attributes were not stripped.
No quarantine/Gatekeeper/security attributes were automatically removed.

## Rights and distribution

The owner confirms proprietary code/name/artwork distribution rights, free download
and use of official 0.1.0 binaries, proprietary source-available terms and acceptance
of ad-hoc signing/non-notarization with disclosures. SOURCE_AVAILABILITY.md records
these decisions without granting proprietary-source redistribution rights. Third-party
license permissions remain unaffected. No OSI license is introduced.

Actual resolved arm64 dependency versions and selected license expressions match
the reviewed inventory; notices accompany the app. The three packaging tools are
MIT and build-only. No specific third-party violation was found, but technical
checks do not establish complete legal compliance or constitute reviewer sign-off.

## Local policy: explicit owner decisions — 2026-11-14

| Flag | Before this approval → after | Recorded authorization |
| --- | --- | --- |
| publication_approved | false → true | Owner explicitly authorizes DLM 0.1.0 installation binaries as potentially public GitHub Actions artifacts before a draft or published GitHub Release exists, as well as the intended public release channel. |
| source_availability_reviewed | true → true | Previously approved free official binary use and proprietary source-available terms remain unchanged; third-party rights are preserved. |
| third_party_licenses_reviewed | false → true | Owner explicitly accepts the documented binary redistribution review, including the corrected ring 0.17.14 file-level attribution notices. The technical assessment is not a legal warranty. |
| history_and_assets_reviewed | false → true | Owner explicitly accepts the documented source-history, assets and privacy review, including residual com.apple.provenance format/correlation uncertainty. |

These explicit decisions satisfy the local policy approvals. They do not establish
Apple notarization, installation/runtime acceptance, a new Intel packaging result,
or separate authorization/configuration for workflow draft creation. No new project
license or proprietary-source redistribution grant is introduced.

## Exact workflow protections

- The release gate needs repository variable DLM_PUBLICATION_APPROVED=true and
  committed preflight: all four flags must be the JSON boolean true, the availability
  document must exist, and a tag run must exactly match the committed release version.
  Nonempty release notes are checked. Build jobs depend on this gate and repeat preflight.
- Identity remains com.dlm.native, version 0.1.0, build 1. Production Release jobs use
  macos-15 arm64 and macos-15-intel x86_64. Each packages and uploads only its DMG and
  SHA-256 sidecar, retained seven days. These are potentially public distribution
  BEFORE draft-environment approval, even when create_draft is false.
- Draft creation needs successful builds, manual dispatch from main, create_draft=true,
  DLM_DRAFT_RELEASE_APPROVED=true and the public-release environment. Configure actual
  required reviewers and main branch restrictions; the environment name alone is not protection.
- The draft job validates the exact four filenames and checksums, requires v0.1.0 to
  resolve to that run's commit, then uses --verify-tag --draft and the committed
  docs/release/RELEASE_NOTES.md. It cannot automatically publish the draft.
- Read-only jobs use contents: read; only the draft job requests contents: write and
  actions: read. Repository/organization token policy must permit those job permissions.
  No external signing credentials are required for the existing ad-hoc builds.
- GitHub variables, reviewer/branch protections, token policy and Private Vulnerability
  Reporting were not inspected or configured. Confirm them manually. See RELEASE.md
  for the controlled sequence; none of these settings is changed by local preparation.

The owner reports successful production and DMG CI on arm64 and x86_64 at f9bf207,
after the privacy corrections. This is supplied CI evidence, not a new local run or
independent Intel artifact inspection. The corrected ring candidate passed a later
real arm64 build/DMG audit; its updated Intel package still requires CI verification.
This approval step changes only documentation, policy and focused tests, so no new
local rebuild or DMG creation is required.
The eventual release run must validate the exact tagged revision and its updated
bundled notice/availability documents. No claim is made about unexecuted final assets.

Ad-hoc signatures are not Developer ID or notarization. Downloaded-app Gatekeeper,
macOS 13 runtime, Finder presentation, actual installation, folder permissions and
browser handoff need manual acceptance or explicit acceptance of known limitations.
No automatic updater or separate background download daemon exists. Closing a
window can leave the Menu Bar process active; Quit stops and checkpoints downloads.

No tag, commit, push, release or upload was performed by this preparation. All four
local approvals are recorded. GitHub safeguards and corrected-candidate CI remain
operational prerequisites before the controlled release sequence.

## Local preparation checks

All 20 focused release-policy/packaging regression tests pass, including independent
rejection of each missing, false or non-boolean approval and rejection of absent
availability terms. Packaging subprocesses are mocked in these tests; these results
are not a new real build or DMG validation. Workflow YAML/matrix/SHA pins, embedded
shell syntax, Python syntax, notes presence and git diff whitespace/privacy-pattern
checks pass. Protected build, identity, version, packaging and workflow files are
byte-for-byte unchanged. No GitHub configuration or remote run was inspected.

## Final metadata and license follow-up

FINAL_METADATA_AND_LICENSE_AUDIT.md supersedes the earlier opaque-provenance and
license-inventory findings above. Provenance's eight-byte local tracking identifier
is characterized; the three-byte prefix remains undocumented, with residual local
correlation uncertainty disclosed. No concrete sensitive metadata was found and no
security attributes were removed. A ring file-header attribution gap was corrected;
the updated production arm64 build and actual DMG pass inspection with 270 legal
texts. Twenty focused tests pass. The owner has now explicitly accepted the review
and its limitations, as recorded in the approval matrix above. Corrected-candidate
Intel CI and separately protected draft creation remain outstanding operational steps.

## Approval-step local validation

Release-policy preflight passes with all four booleans true. All 20 focused regression
tests pass, including independent fail-closed checks for each missing/false/non-boolean
approval and the required ring supplement. git diff --check passes. Build, packaging,
workflow and ring resource/inventory bytes are unchanged during this approval step.
No new build, DMG operation, remote run, variable change or publication was performed.
