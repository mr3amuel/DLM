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
