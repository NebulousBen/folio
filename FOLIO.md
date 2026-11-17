# Folio development

Folio builds on PDFCraft v0.21.1. The `folio` branch is the base for Folio changes. The official repository belongs in the `upstream` remote and the public fork in `origin`.

## Local Windows build

Install Rust 1.95.0 with rustfmt and Clippy, plus Microsoft C++ Build Tools and the Windows SDK. From the fork, build with `cargo +1.95.0 build --locked -p pdfcraft -p pdfcraft-cli`. The Cargo executable is `target/debug/pdfcraft.exe`. Set `FOLIO_NATIVE_DATA_DIR` to a disposable absolute directory when testing the app. The release repository is a compile-time `FOLIO_RELEASE_REPO` value containing the fork's GitHub owner and repository.

The development port can preview and import legacy reading data into a separate profile. On Windows, smoke tests with a synthetic PDF verified bookmark autosave, checkpoint restore, virtual workspace persistence, and creation of a one-page AI handoff with a verified page map and checksum. The source PDF's SHA-256 stayed the same. The CLI exposes `folio_reading_info` and `folio_extract_bookmarks` for PDFs with embedded Folio data.

The branch also contains reading mark and category editors, filtered note exports, annotated PDF copies, folder workspaces, offline Canadian-English proofreading, and connected AI readings. The connected AI flow uses the local Codex runtime from the existing Folio release. Set `FOLIO_CODEX_BINARY` to its absolute executable path when running a development build. Connected analysis and recovery still need an end-to-end test with that runtime.

Production cutover still requires verification of existing data migration, reading continuity after native PDF edits, and AI job recovery. The existing Folio release remains the production reader.

## Upstream releases

Enable Actions on the fork and allow GitHub Actions to create pull requests in its repository settings. Set `folio` as the default branch. The **Check upstream PDFCraft** workflow looks for a new official stable release daily and can also run on demand. It uses Git, GitHub's release API, and standard runners without AI model calls. A new release opens an update PR on the fork and validates the candidate merge. Resolve conflicts on the update branch, re-run the checks, then merge with a merge commit to retain upstream ancestry. If GitHub pauses the schedule after inactivity, re-enable it in the fork's Actions tab.
