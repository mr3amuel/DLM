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
