# Release operations

The workflow is local preparation until publication is explicitly authorized.
All scripts/release/policy.json flags remain false. The repository approval variable
DLM_PUBLICATION_APPROVED=true and reviewed policy flags are required before builds.
Source visibility without an open-source grant is an approved choice; dependency
license compliance must still be cleared independently before release.

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
publication preflight. There is no skip-preflight flag. Policy values and the existing
release workflow remain unchanged. Do not use a new upload action to turn inspection
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
checks succeeded. Later production privacy remapping requires renewed Intel CI
verification; the owner-reported earlier Intel result predates that correction.

## First 0.1.0 release approval sequence

See PUBLIC_RELEASE_READINESS.md for the latest scoped local findings. Before any
release workflow is enabled, the owner must approve binary distribution (including
potentially public Actions artifacts), source-availability/recipient terms, third-party
redistribution and final privacy/history/artwork review. This preparation changes no
approval values. The four committed policy flags and repository approval variables
remain separate controls, not findings that can be silently set by a build.

Review and commit the corrections, rerun both production and DMG verification
architectures on that exact revision, and complete manual unsigned-app installation
acceptance. Then explicitly authorize the approval edits, configure public-release
reviewers/branch restrictions, and confirm Private Vulnerability Reporting. Only
then create the version-matching tag and run the intended release workflow. Artifact
uploads happen in the build jobs before draft approval; they are distribution, even
when create_draft is false. Verify both checksum sidecars against their downloaded
DMGs, review draft notes/assets, and manually approve final public publication.

The release job creates a draft only; it contains no automatic public-publish command.
SOURCE_AVAILABILITY.md is not a general open-source grant. The owner should confirm
intended binary recipient permissions without implying third-party terms are DLM's
project license. Do not add a project license on the owner's behalf.
