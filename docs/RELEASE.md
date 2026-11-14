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
the publisher. CI/Intel packaging and visual acceptance must run after authorization.

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

Real packaging, Finder presentation and both-architecture packaging success remain
unverified until this workflow completes. Mocked tests do not create real disk images.
