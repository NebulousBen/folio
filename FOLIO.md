# Folio development

Folio builds on PDFCraft v0.21.1. The `folio` branch is the base for Folio changes. The official repository belongs in the `upstream` remote and the public fork in `origin`.

## Local Windows build

Install Rust 1.95.0 with rustfmt and Clippy, plus Microsoft C++ Build Tools and the Windows SDK. From the fork, build with `cargo +1.95.0 build --locked -p pdfcraft -p pdfcraft-cli`. The Cargo executable is `target/debug/pdfcraft.exe`. Set `FOLIO_NATIVE_DATA_DIR` to a disposable absolute directory when testing the app. The release repository is a compile-time `FOLIO_RELEASE_REPO` value containing the fork's GitHub owner and repository.

The development port can preview and import legacy reading data into a separate profile. The native bookmark editor has been tested with a synthetic PDF. The test verified that a new bookmark persisted alongside the existing range and that the original PDF's SHA-256 stayed the same. The CLI exposes `folio_reading_info` and `folio_extract_bookmarks` for PDFs with embedded Folio data.

The branch also contains the reading mark and category editors, filtered note exports, annotated PDF copies, folder and virtual workspaces, offline Canadian-English proofreading, and AI reading handoffs. Those features need end-to-end verification on the latest build. The connected AI flow uses the local Codex runtime from the existing Folio release. Set `FOLIO_CODEX_BINARY` to its absolute executable path when running a development build.

Production cutover still requires verification of existing data migration, reading continuity after native PDF edits, and AI job recovery. The existing Folio release remains the production reader.

## Upstream releases

Enable Actions on the fork and allow GitHub Actions to create pull requests in its repository settings. Set `folio` as the default branch. The **Check upstream PDFCraft** workflow looks for a new official stable release daily and can also run on demand. It uses Git, GitHub's release API, and standard runners without AI model calls. A new release opens an update PR on the fork and validates the candidate merge. Resolve conflicts on the update branch, re-run the checks, then merge with a merge commit to retain upstream ancestry. If GitHub pauses the schedule after inactivity, re-enable it in the fork's Actions tab.
