# Third-party notices

DLM itself has no general open-source license grant; see SOURCE_AVAILABILITY.md.
Dependencies retain their own grants and obligations. RUST_DEPENDENCIES.json lists
149 resolved package versions, upstream SPDX expressions, selected permissive
alternatives and preserved root/nested notices. The inventory is a conservative
superset of the runtime, helper, test and build graphs for both macOS architectures.

All preserved legal texts and public attributions are retained under third-party/rust
and copied into DLM.app/Contents/Resources/ThirdPartyNotices at build time. Packaging
checks their presence before distribution. Do not strip that resource directory.
See docs/LICENSING_REVIEW.md for selections, distribution duties and review scope.

Official DLM 0.1.0 binaries are intended for free download and use under the owner's
confirmed decision. The proprietary source is source-available, not open source.
Neither that decision nor the owner's rights in DLM establishes satisfaction of
all third-party redistribution conditions. These restrictions do not diminish
third-party license permissions. Final binary compliance sign-off remains separate.
