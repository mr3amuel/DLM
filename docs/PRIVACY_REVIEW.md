# Initial source privacy review — 2026-11-14

**PASS for the exact reviewed snapshot and local initial commit.** This is not a
proof that every possible secret, steganographic payload or private detail is absent.
No remote creation, upload, public release or runtime acceptance is authorized by
this review. Release policy flags deliberately remain false.

The exact SOURCE_MANIFEST inventory and hashes were checked against all public
files. It contains only native sources, shared engine code, required browser/FFI
components, two project branding assets, tests, documentation, license notices and
release configuration. No previous Git directory/history, other UI application,
Dart source/generated bindings, personal runtime databases, browser profiles, logs,
private signing material or build outputs is included. Source dependencies resolve
with both locked native Cargo workspaces for both macOS architectures, offline.

The private export controls checked known private development identities, home
paths, sensitive file types and key/token signatures before writing. Manual triage
of remaining scanner candidates found only the following:

- Credential-bearing URL examples occur in synthetic negative validation tests;
  they are not usable credentials. Preserve those security tests.
- Email-shaped browser extension IDs, public add-on allowlists and third-party
  copyright/attribution contacts are required identifiers or legitimate notices.
- The explicitly approved public GitHub noreply identity is permitted for new Git
  author/committer metadata and is not treated as a private address disclosure.
- The only opaque artwork is DLMBrand.png and DLMIcon.icns. Both were visually
  inspected as the original DLM two-arrow branding. Chunk inspection found only
  PNG image/color chunks; no text, EXIF or time metadata remains. All 11 icon
  representations retain the original native artwork's compressed pixel payloads.

No unresolved personal-data or credential exposure was identified. This result is
scoped to the reviewed bytes, not arbitrary future files or revisions. Automated
triage intentionally still labels public attribution/fixture/image candidates;
those generic labels are not themselves a confirmed privacy incident or a reason
to strip license notices. No remote secret-scanning service received repository data.

Before the first actual upload, verify staged filenames and metadata again, retain
source availability/third-party rights documentation and enable Private Vulnerability
Reporting after authorized repository creation. Never copy the authoritative .git
or export-control directory. Build logs and workspace directories are not artifacts.
