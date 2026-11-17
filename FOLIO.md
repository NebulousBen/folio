# Folio development

Folio builds on PDFCraft v0.21.1. The `folio` branch is the base for Folio changes. The official repository belongs in the `upstream` remote and the public fork in `origin`.

## Local Windows build

Install Rust 1.95.0 with rustfmt and Clippy, plus Microsoft C++ Build Tools and the Windows SDK. From the fork, build with `cargo +1.95.0 build --locked -p pdfcraft -p pdfcraft-cli`. The Cargo executable is `target/debug/pdfcraft.exe`. Set `FOLIO_NATIVE_DATA_DIR` to a disposable absolute directory when testing the app. The release repository is a compile-time `FOLIO_RELEASE_REPO` value containing the fork's GitHub owner and repository.

The development port can preview and import legacy reading data into a separate profile. Windows smoke tests verified bookmark autosave, checkpoint restore, virtual workspace persistence, the offline Canadian-English checker, and creation of a one-page AI handoff with a verified page map and checksum. The source PDF's SHA-256 stayed the same. The CLI exposes `folio_reading_info` and `folio_extract_bookmarks` for PDFs with embedded Folio data.

The branch also contains reading mark and category editors, filtered note exports, annotated PDF copies, and folder workspaces. The two-pass AI reading resumed after an app restart in a test with a local Codex stub. The bundled Codex runtime started a thread under the restricted Folio permission profile. Live model analysis still needs an end-to-end test. Set `FOLIO_CODEX_BINARY` to the runtime's absolute executable path when running a development build.

The native Rotate Pages and Save commands preserved reading marks and bookmarks in the new revision. Folio carries anchors through incremental saves when it can verify the page content and bounds. Reading data for changed pages and full rewrites stays attached to the prior revision.

Production cutover still requires a complete migration and release test. The existing Folio release remains the production reader.

## Upstream releases

Enable Actions on the fork and allow GitHub Actions to create pull requests in its repository settings. Set `folio` as the default branch. The **Check upstream PDFCraft** workflow looks for a new official stable release daily and can also run on demand. It uses Git, GitHub's release API, and standard runners without AI model calls. A new release opens an update PR on the fork and validates the candidate merge. Resolve conflicts on the update branch, re-run the checks, then merge with a merge commit to retain upstream ancestry. If GitHub pauses the schedule after inactivity, re-enable it in the fork's Actions tab.
