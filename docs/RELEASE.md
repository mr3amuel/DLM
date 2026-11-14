# Release operations

All four local scripts/release/policy.json flags are true following explicit owner
approval, including potentially public Actions artifact distribution and acceptance
of the corrected ring notices and provenance uncertainty. See PUBLIC_RELEASE_READINESS.md.
No GitHub setting was changed. DLM_PUBLICATION_APPROVED=true must still be manually
configured before release workflow builds. Draft creation remains separately gated;
no draft/public release exists as a result of this local preparation.

Manual workflow dispatch or intentional v<version> tags build separate macos-15
arm64 and macos-15-intel artifacts. Version/build are in prototypes/swiftui's release
text files. Native Rust targets match Swift and the browser helper. CI uses SHA-pinned
actions, read-only builds, no workspace/log uploads, exact DMG/checksum artifact paths.
The only write job creates a draft after manual dispatch, create_draft=true and the
separate DLM_DRAFT_RELEASE_APPROVED variable. Configure the public-release environment
with required reviewers and branch restrictions. Its matching version tag must already
point at the dispatched commit. Public release publication remains a human action.

Each headless DMG contains DLM.app, an Applications shortcut and scripted Finder
presentation using original DLM branding. Bundle identity/version/architecture,
resources/helpers, linked libraries, signatures and read-only mounted DMG contents
are verified. SHA-256 sidecars cover finalized bytes; digests alone do not authenticate
the publisher. Both-architecture CI packaging success has been reported by the owner. Finder and
installation acceptance remain separate; release-candidate changes require fresh CI checks.

Apps are ad-hoc signed, not Developer ID signed or notarized. Gatekeeper may block
first launch; follow Apple's per-application guidance at https://support.apple.com/102445
only after choosing to trust the source. No Gatekeeper or quarantine bypass exists.
Choose the matching architecture and quit normally before replacing an installed app.
Folder access/bookmarks may require renewed consent after ad-hoc rebuilds. This phase
has no automatic installer, updater, repository creation, upload or native cutover.

## Runner-local DMG inspection (no distribution)

`macOS DMG Packaging Verification` is a separate manual-only workflow for arm64 and
x86_64 production Release builds. It installs the pinned packaging tools, builds
without launching the app, and invokes:

```sh
python scripts/release/package.py inspect-runner-local arm64
```

This command deliberately refuses execution outside a manually dispatched,
GitHub-hosted run with an existing RUNNER_TEMP outside the checkout. Its only
output directory is a new marked temporary workspace below RUNNER_TEMP. It uses
the same bundle, architecture, signature, dependency, DMG-content and checksum
checks as publication packaging, then deletes the image and sidecar. It never writes
GITHUB_OUTPUT or the source dist directory. No app launch, installation, registration,
migration, artifact upload or release command is included. Environment guards prevent
accidental misuse; they are not a security boundary against someone editing code or
spoofing their own environment. No licensing or public distribution approval is implied.

The ordinary `package.py arm64` / `package.py x86_64` entry still calls the unchanged
publication preflight. There is no skip-preflight flag. Inspection does not change policy values or the existing
release workflow. Do not use a new upload action to turn inspection
into unapproved public distribution.

Inspection detaches its unique read-only mount, retries with force if needed, and
refuses recursive cleanup while that mount remains. An always-run workflow step
retries cleanup of marked workspaces. Cancellation, runner loss or an unrecoverable
busy mount can prevent cleanup; leftovers are then confined to the ephemeral runner
and discarded at teardown. Logs also stay runner-local and are never uploaded.

The owner reports successful GitHub-runner packaging on both architectures. This
does not verify later source changes or Finder/installation behavior. Local arm64
real-image evidence is recorded below; mocked tests alone do not establish packaging success.

## Explicit local packaging diagnostics

For local macOS testing, `python scripts/release/package.py inspect-local arm64`
uses the same disposable image creation and all validation checks as runner-local
inspection. Install requirements in an isolated virtual environment and build the
production Release app first. The local command rejects GitHub Actions and does
not set or spoof any GitHub environment variables. It never calls publication
preflight, writes GITHUB_OUTPUT, retains a distributable image, or changes approvals.
Ordinary publication packaging still requires its unchanged preflight.

Private temporary diagnostic logs now include command context, subprocess failures
and Python validation tracebacks. Logs and checksums contain no distribution grant;
never upload them without separate review. Successful images/checksums are removed.

Real arm64 testing identified two failures: the Swift interpreter could not resolve
CoreGraphics overlay symbols, and dmgbuild 1.6.7 stores the PNG as `.background.png`,
not a `.background` directory. Packaging now compiles/links a temporary headless
renderer for the selected architecture and validates the mounted PNG byte-for-byte
against its generated source. Unexpected DMG entries remain rejected. Local arm64
creation, integrity, mounting, bundle/signature/resource/notice, checksum and cleanup
checks succeeded. The owner reports subsequent production and DMG verification success on both
architectures at f9bf207, including the production privacy remapping correction.

## First 0.1.0 release approval sequence

See PUBLIC_RELEASE_READINESS.md for the approval matrix and evidence. Ownership,
free official binary use, proprietary source availability and acceptance of ad-hoc
signing/non-notarization are confirmed. Explicit owner approval now also covers
potentially public Actions artifacts, the corrected third-party review and the
history/assets/privacy review including provenance uncertainty. These decisions
are recorded independently of build success; no legal warranty is implied.

1. Review these local edits. The owner commits/pushes the reviewed changes when ready;
   this preparation does neither. Include the untracked ring SOURCE-NOTICES.txt and
   final audit report, the inventory, tests and all reviewed documentation/policy edits.
   Run both manual no-upload production/DMG verification workflows on that exact
   candidate, particularly Intel, before any release tag or distribution run.
2. Configure repository variables DLM_PUBLICATION_APPROVED=true only after all committed
   approvals, and DLM_DRAFT_RELEASE_APPROVED=true only for separately authorized draft
   creation. Configure public-release required reviewers and main branch restriction.
   Confirm Actions is enabled and repository/organization token policy permits the
   draft job's contents: write/actions: read. Confirm Private Vulnerability Reporting.
   Protect release tags where supported. Do not store credentials in repository variables.
3. Choose the exact reviewed main commit. Version/build must remain 0.1.0/1. Confirm
   v0.1.0 does not already exist locally/remotely; never force-move a release tag.
   Review successful verification evidence for the corrected candidate; f9bf207
   predates the ring attribution supplement and does not verify its Intel package. Final publication packaging
   revalidates the exact tagged revision, including updated notice documents.
4. Only after artifact distribution is authorized, the owner creates an annotated
   v0.1.0 tag at that candidate commit and pushes it. The tag event builds and uploads
   the two DMGs and sidecars as Actions artifacts; it does NOT create a draft. Inspect
   both downloaded images and verify each SHA-256 sidecar. Do not assume these artifacts
   are private. Do not use the no-upload inspection workflow as a distribution bypass.
5. Keep main at that same candidate commit and manually run Intentional macOS release
   candidates from main with create_draft=true. This deliberately rebuilds/repackages
   both architectures; after the protected environment review it verifies all four
   assets and the matching tag before creating a draft. If main moved, do not retag;
   resolve the candidate mismatch before proceeding. Do not mix assets from different runs.
6. Inspect that draft's notes, identity/version, both architecture-specific DMGs and
   both sidecars. Verify checksums again and complete user-led installation/first-launch
   acceptance on the supported systems, or explicitly record untested limitations.
   Confirm free-use/source-available terms and non-notarization/Gatekeeper disclosures
   are visible. No automatic updater exists; downloads require the application process.
7. Publish the reviewed draft only through a separate, explicit owner action. Neither
   tag push nor workflow draft creation performs final publication. Stop on any failed
   gate, changed candidate, unexpected artifact, privacy issue or unresolved obligation.

For installation choose arm64 for Apple Silicon or x86_64 for Intel, macOS 13 or later.
Quit DLM before replacement, open the DMG and copy DLM.app to Applications. The app is
ad-hoc signed, not notarized; macOS may block it. If the user independently trusts
this source, follow Apple's per-app approval guidance linked above where available.
Never globally disable Gatekeeper or remove security attributes as a standard setup
step. Closing a window may leave the Menu Bar process running; Quit checkpoints and
stops transfers. There is no persistent background download service.
