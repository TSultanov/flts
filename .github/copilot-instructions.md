# Copilot instructions (FLTS)

## Repo layout (service boundaries)

- Rust workspace members are declared in `Cargo.toml`: `library/` (core), `cli/` (batch/ops), `site/src-tauri/` (Tauri backend).
- `site/` is the Svelte 5 + Vite UI; it talks to the Tauri backend via `invoke` + emitted events.
- `dbsync/` is the vendored columnar CRDT store the library persists to; `syncthing-sys/` + `syncthing-core/` embed the Syncthing engine that transports the files.

## End-to-end data flow (UI & Tauri & library)

- Frontend calls commands by string name via `@tauri-apps/api/core` (see `site/src/lib/data/library.ts`).
- Tauri command registration lives in `site/src-tauri/src/lib.rs` (`tauri::generate_handler![...]`), with implementations under `site/src-tauri/src/app/` (e.g. `site/src-tauri/src/app/library_view.rs`).
- After writes/imports, the backend emits events like `library_updated` and `book_updated` which the UI turns into Svelte stores via `site/src/lib/data/tauri.ts`.

## Library persistence conventions (Rust `library` crate)

- A “library” is a folder on disk holding one `dbsync` store, wrapped by `LibraryStore` (`library/src/store/mod.rs`; tables in `library/src/store/schema.rs`).
- Each device appends rows to its own shard files (`*.dbs`) under `<library_root>/db/`; that directory is the only part that syncs. Row ids are derived from content, so devices writing the same thing name the same row.
- The cache, lock and writer id are device-local, in `<parent>/.<root_name>.flts-local/` (inside the root only when the parent is unwritable).
- The device roster is a separate dbsync store under `<library_root>/.flts/roster/` (`library/src/sync/roster.rs`).
- The watcher (`library/src/library/file_watcher.rs`) reports changed shards; the library ignores its own and adopts other devices' rows.
- The older file-per-book format (`<uuid>/book.dat`, `translation_*.dat`, …) is only read, by the one-way importer in `library/src/legacy.rs`; nothing writes it.
- Settings stay in `config.json`; caches are plain files under the cache dir.
- Language IDs are ISO 639-3 (3-letter) strings; Rust code uses `isolang::Language`.

## Translation prompt / model IDs

- Translation models and the LLM prompt template live in `library/src/translator.rs`.
- The model is passed over the JS/Rust boundary as a string ID; the catalog lives in `library/src/translator/catalog.rs`.
- Translation caching is a disk cache (`DiskCache`, `library/src/cache/`) keyed by `${prompt_schema_version}\n${src}\n${tgt}\n${paragraph}` (see `library/src/cache/mod.rs`).

## Developer workflows (commands that actually exist here)

- Rust (from repo root): `cargo check -q`, `cargo test -q`.
- Tauri desktop (from `site/`): `pnpm install`, then `cargo tauri dev`.
- Site tests (from `site/`): `pnpm test`, `pnpm test:coverage`, `pnpm test:e2e`.
- CLI expects `--library-path` and subcommands (see `cli/src/main.rs`); example:
  - `cargo run -p cli -- --library-path /path/to/library list`

## Repo-specific change rules (avoid footguns)

- Adding/renaming a Tauri command requires updating BOTH:
  1. the handler list in `site/src-tauri/src/lib.rs`, and
  2. the frontend wrapper(s) that call it (typically `site/src/lib/data/library.ts`).
- Prefer emitting an existing event (or add a new one consistently) so the UI refreshes; event names are stringly-typed.
- UI styling: reuse CSS variables and button variant classes defined in `site/src/app.css` (e.g. `button.secondary`, `button.danger`, `button.compact`).
