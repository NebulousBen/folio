# Folio development

Folio builds on PDFCraft v0.21.1. The `folio` branch is the base for Folio changes. The official repository belongs in the `upstream` remote and the public fork in `origin`.

## Local Windows build

Install Rust 1.95.0 with rustfmt and Clippy, plus Microsoft C++ Build Tools and the Windows SDK. From the fork, build with `cargo +1.95.0 build --locked -p pdfcraft -p pdfcraft-cli`. The Cargo executable is `target/debug/pdfcraft.exe`. Set `FOLIO_NATIVE_DATA_DIR` to a disposable absolute directory when testing the app. The release repository is a compile-time `FOLIO_RELEASE_REPO` value containing the fork's GitHub owner and repository.

Folio stores native reading data in `%APPDATA%\folio-reader\native` on Windows. Run `pdfcraft-cli folio-migrate "%APPDATA%\folio-reader"` from Command Prompt for a read-only preview, and add `--apply` to copy readings and workspace references. Repeat imports preserve newer native notes. The Home screen offers saved-tab restoration, previous AI readings, and JSON backup and restore. Open a source PDF and use **Reading notes → Restore Folio form answers** to apply compatible saved answers as an undoable edit. Save to commit those answers to the PDF. The CLI also exposes `folio_reading_info` and `folio_extract_bookmarks` for PDFs with embedded Folio data.

The branch also contains reading mark and category editors, filtered note exports, annotated PDF copies, and folder workspaces. The bundled Codex runtime completed both reading passes on a synthetic PDF under Folio's restricted permission profile. Folio saved both checkpoints and imported the reading layer with the source PDF unchanged. A separate restart test verified resuming from checkpoints. Set `FOLIO_CODEX_BINARY` to the runtime's absolute executable path when running a development build.

The native Rotate Pages and Save commands preserved reading marks and bookmarks in the new revision. Folio carries anchors through incremental saves when it can verify the page content and bounds. Reading data for changed pages and full rewrites stays attached to the prior revision.

Markdown source files remain in the original library. Folio continues to write Markdown note exports and AI handoff instructions.

## Upstream releases

Enable Actions on the fork and allow GitHub Actions to create pull requests in its repository settings. Set `folio` as the default branch. The **Check upstream PDFCraft** workflow looks for a new official stable release daily and can also run on demand. It uses Git, GitHub's release API, and standard runners without AI model calls. A new release opens an update PR on the fork and validates the candidate merge. Resolve conflicts on the update branch, re-run the checks, then merge with a merge commit to retain upstream ancestry. If GitHub pauses the schedule after inactivity, re-enable it in the fork's Actions tab.
