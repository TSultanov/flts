# Copilot instructions (FLTS)

## Repo layout (service boundaries)
- Rust workspace members are declared in `Cargo.toml`: `library/` (core), `cli/` (batch/ops), `site/src-tauri/` (Tauri backend).
- `site/` is the Svelte 5 + Vite UI; it talks to the Tauri backend via `invoke` + emitted events.
- `extension/` is a WXT + Svelte browser extension (separate runtime/build from `site/`).

## End-to-end data flow (UI & Tauri & library)
- Frontend calls commands by string name via `@tauri-apps/api/core` (see `site/src/lib/data/library.ts`).
- Tauri command registration lives in `site/src-tauri/src/lib.rs` (`tauri::generate_handler![...]`), with implementations under `site/src-tauri/src/app/` (e.g. `site/src-tauri/src/app/library_view.rs`).
- After writes/imports, the backend emits events like `library_updated` and `book_updated` which the UI turns into Svelte stores via `site/src/lib/data/tauri.ts`.

## Library persistence conventions (Rust `library` crate)
- A “library” is a folder on disk; each book is stored under `<library_root>/<uuid>/`.
- Key file formats:
  - `book.dat` (plus possible conflict siblings like `book*.dat`)
  - `translation_<src>_<tgt>.dat` and `dictionary_<src>_<tgt>.dat` (see event classification in `library/src/library/file_watcher.rs`).
- Language IDs are ISO 639-3 (3-letter) strings; Rust code uses `isolang::Language`.

## Translation prompt / model IDs
- Translation models and the LLM prompt template live in `library/src/translator.rs`.
- The model is passed over the JS/Rust boundary as a numeric ID (don’t reorder/renumber without updating the UI/config paths).
- Translation caching is a disk-backed `foyer` hybrid cache keyed by `${src}\n${tgt}\n${paragraph}` (see `library/src/cache.rs`).

## Developer workflows (commands that actually exist here)
- Rust (from repo root): `cargo check -q`, `cargo test -q`.
- Tauri desktop (from `site/`): `pnpm install`, then `cargo tauri dev`.
- Site tests (from `site/`): `pnpm test`, `pnpm test:coverage`, `pnpm test:e2e`.
- Browser extension (from `extension/`): `pnpm install`, then `pnpm dev` (or `pnpm dev:firefox`).
- CLI expects `--library-path` and subcommands (see `cli/src/main.rs`); example:
  - `cargo run -p cli -- --library-path /path/to/library list`

## Repo-specific change rules (avoid footguns)
- Adding/renaming a Tauri command requires updating BOTH:
  1) the handler list in `site/src-tauri/src/lib.rs`, and
  2) the frontend wrapper(s) that call it (typically `site/src/lib/data/library.ts`).
- Prefer emitting an existing event (or add a new one consistently) so the UI refreshes; event names are stringly-typed.
- UI styling: reuse CSS variables and button variant classes defined in `site/src/app.css` (e.g. `button.secondary`, `button.danger`, `button.compact`).
