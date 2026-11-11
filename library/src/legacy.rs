//! One-way import of the file-per-book format: `<uuid>/book.dat`,
//! `translation_*.dat`, `chapter_summaries.dat`, `state.json` and
//! `cards/<src>-<tgt>/<slug>.json`, with their sync-conflict siblings.
//!
//! Nothing here writes that format. Imported directories move out of the
//! synced folder into the device-local one, so other devices see them go and
//! receive the same rows through the store instead. Row ids are derived from
//! content, so two devices importing the same files agree without talking.
//!
//! Backups sit in `<local>/legacy/<YYYYMMDD>/` for a retention period, or in
//! `<local>/legacy/unreadable/` when part of the item could not be parsed.

use std::{
    collections::BTreeSet,
    io::{self, Cursor},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};

use uuid::Uuid;

use crate::{
    book::{
        book::Book, chapter_summaries::ChapterSummaries, serialization::Serializable,
        translation::Translation,
    },
    card::Card,
    library::library_book::{BookReadingState, BookUserState},
    store::{BookDelta, LibraryStore, NewVersion},
};

/// Where imported legacy directories are kept, inside the local directory.
pub const BACKUP_DIR: &str = "legacy";

/// Day buckets older than this are deleted; `unreadable` never is.
pub const BACKUP_RETENTION_DAYS: u64 = 30;

const UNREADABLE: &str = "unreadable";

/// What one pass added to the store.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    /// Book directories that added rows.
    pub books: usize,
    /// Whether any card row was added.
    pub cards: bool,
    /// Directories not, or not fully, imported.
    pub failed: usize,
}

impl Imported {
    /// True when rows were inserted, not merely when files were seen.
    pub fn any(&self) -> bool {
        self.books > 0 || self.cards
    }
}

struct LegacyCard {
    path: PathBuf,
    source: String,
    target: String,
    slug: String,
    card: Card,
}

/// True the first time `path` is asked about in this process.
fn first_sight(path: &Path) -> bool {
    static SEEN: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
    SEEN.lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(path.to_owned())
}

fn modified(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Files in `dir` named `<prefix>*<suffix>`, oldest first.
fn named(dir: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    // `~` marks a write that never finished.
                    .is_some_and(|name| {
                        name.starts_with(prefix) && name.ends_with(suffix) && !name.contains('~')
                    })
        })
        .collect();
    found.sort_by_key(|path| (modified(path), path.clone()));
    found
}

/// Whether Syncthing is still writing something under `dir`.
fn in_flight(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            (name.starts_with(".syncthing.") && name.ends_with(".tmp"))
                || (entry.path().is_dir() && in_flight(&entry.path()))
        })
}

fn read<T: Serializable>(path: &Path) -> Option<T> {
    let parsed = std::fs::read(path).and_then(|bytes| T::deserialize(&mut Cursor::new(bytes)));
    match parsed {
        Ok(value) => Some(value),
        Err(err) => {
            log::warn!("skipping unreadable legacy file {}: {err}", path.display());
            None
        }
    }
}

/// The fields one `state*.json` names; an outer `None` is a field it is
/// silent about.
#[derive(Debug, Default, PartialEq, Eq)]
struct StateFile {
    reading_state: Option<Option<BookReadingState>>,
    folder_path: Option<Vec<String>>,
}

/// Parses a `state*.json`: empty file, `{readingState, folderPath}` object, or
/// a bare [`BookReadingState`].
fn parse_user_state(contents: &str) -> Option<StateFile> {
    if contents.trim().is_empty() {
        return Some(StateFile::default());
    }
    let value: serde_json::Value = serde_json::from_str(contents).ok()?;
    let (reading, folder) = (value.get("readingState"), value.get("folderPath"));
    if reading.is_some() || folder.is_some() {
        return Some(StateFile {
            reading_state: match reading {
                Some(reading) => Some(serde_json::from_value(reading.clone()).ok()?),
                None => None,
            },
            folder_path: match folder {
                Some(folder) => Some(
                    serde_json::from_value::<Option<Vec<String>>>(folder.clone())
                        .ok()?
                        .unwrap_or_default(),
                ),
                None => None,
            },
        });
    }
    Some(StateFile {
        reading_state: Some(Some(serde_json::from_value(value).ok()?)),
        folder_path: None,
    })
}

/// Each field from the newest file that names it, an empty value included.
/// False when a file could not be parsed.
fn user_state(dir: &Path) -> (BookUserState, bool) {
    let (mut reading, mut folder, mut clean) = (None, None, true);
    for path in named(dir, "state", ".json").iter().rev() {
        let Some(state) = std::fs::read_to_string(path)
            .ok()
            .and_then(|contents| parse_user_state(&contents))
        else {
            log::warn!("skipping unreadable legacy state {}", path.display());
            clean = false;
            continue;
        };
        reading = reading.or(state.reading_state);
        folder = folder.or(state.folder_path);
    }
    let state = BookUserState {
        reading_state: reading.flatten(),
        folder_path: folder.unwrap_or_default(),
    };
    (state, clean)
}

/// Every version as its own file stamped it: merging files first would move
/// timestamps, and with them the ids another device derives.
fn push_versions(translation: &Translation, versions: &mut Vec<NewVersion>) {
    for paragraph in 0..translation.paragraph_slots() {
        for version in translation.versions(paragraph) {
            versions.push(NewVersion {
                source_language: translation.source_language.clone(),
                target_language: translation.target_language.clone(),
                paragraph,
                translation: version.to_import(),
                model: version.model.clone(),
            });
        }
    }
}

/// The cards under `cards`, and whether everything there could be read.
fn read_cards(cards: &Path) -> (Vec<LegacyCard>, bool) {
    let (mut found, mut clean) = (Vec::new(), true);
    for deck in std::fs::read_dir(cards).into_iter().flatten().flatten() {
        if !deck.path().is_dir() {
            continue;
        }
        let name = deck.file_name();
        let Some((source, target)) = name.to_str().and_then(|name| name.split_once('-')) else {
            log::warn!("skipping legacy deck {}", deck.path().display());
            clean = false;
            continue;
        };
        let mut files = named(&deck.path(), "", ".json");
        // The canonical file before its conflict siblings: it carries the
        // Anki state the siblings' copies must not replace.
        files.sort_by_key(|path| path.as_os_str().len());
        for path in files {
            let slug = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Card>(&bytes).ok())
                .map(|card| {
                    let slug = crate::card::lemma_slug(
                        &crate::card::canonicalize_lemma_display(&card.lemma).to_lowercase(),
                    );
                    (card, slug)
                })
                .filter(|(_, slug)| !slug.is_empty());
            let Some((card, slug)) = slug else {
                log::warn!("skipping unreadable legacy card {}", path.display());
                clean = false;
                continue;
            };
            found.push(LegacyCard {
                path,
                source: source.to_owned(),
                target: target.to_owned(),
                slug,
                card,
            });
        }
    }
    (found, clean)
}

fn scan(root: &Path) -> (Vec<(PathBuf, Uuid)>, Option<PathBuf>) {
    let mut books = Vec::new();
    let mut cards = None;
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name == "cards" {
            cards = Some(path);
        } else if let Ok(id) = Uuid::parse_str(name) {
            books.push((path, id));
        }
    }
    books.sort();
    (books, cards)
}

fn today() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| (since.as_secs() / 86_400) as i64)
}

/// `YYYYMMDD` of a day counted from 1970-01-01, proleptic Gregorian.
fn bucket_name(day: i64) -> String {
    let shifted = day + 719_468;
    let (era, of_era) = (shifted.div_euclid(146_097), shifted.rem_euclid(146_097));
    let year_of_era = (of_era - of_era / 1_460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * of_year + 2) / 153;
    let day_of_month = of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day_of_month:02}")
}

/// Inverse of [`bucket_name`]; `None` for anything it would not produce.
fn bucket_day(name: &str) -> Option<i64> {
    if name.len() != 8 || !name.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let part = |range: std::ops::Range<usize>| name[range].parse::<i64>().ok();
    let (year, month, day_of_month) = (part(0..4)?, part(4..6)?, part(6..8)?);
    let year = year - i64::from(month <= 2);
    let (era, year_of_era) = (year.div_euclid(400), year.rem_euclid(400));
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let of_year = (153 * shifted_month + 2) / 5 + day_of_month - 1;
    let of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + of_year;
    let day = era * 146_097 + of_era - 719_468;
    (bucket_name(day) == name).then_some(day)
}

fn copy_all(from: &Path, to: &Path) -> io::Result<()> {
    if !from.is_dir() {
        return std::fs::copy(from, to).map(drop);
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        copy_all(&entry.path(), &to.join(entry.file_name()))?;
    }
    Ok(())
}

fn remove(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Moves `from` to `to`, copying when `rename` refuses, as it does across
/// filesystems.
fn relocate(
    from: &Path,
    to: &Path,
    rename: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    if rename(from, to).is_ok() {
        return Ok(());
    }
    if let Err(err) = copy_all(from, to) {
        let _ = remove(to);
        return Err(err);
    }
    remove(from)
}

/// Where something set aside now goes: today's bucket, or the one that is
/// never swept.
pub(crate) fn bucket(backup: &Path, unreadable: bool) -> PathBuf {
    if unreadable {
        backup.join(UNREADABLE)
    } else {
        backup.join(bucket_name(today()))
    }
}

/// Out of the synced folder, but kept: the import is not trusted with the
/// only copy. `unreadable` keeps it past the retention period.
pub(crate) fn set_aside(path: &Path, backup: &Path, unreadable: bool) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("nothing to set aside"))?;
    let bucket = bucket(backup, unreadable);
    std::fs::create_dir_all(&bucket)?;
    let mut target = bucket.join(name);
    let mut copy = 1;
    while target.exists() {
        target = bucket.join(format!("{}.{copy}", name.to_string_lossy()));
        copy += 1;
    }
    relocate(path, &target, |from, to| std::fs::rename(from, to))
}

/// Backups named `<uuid>[.N]`, in a bucket or loose in `backup` itself.
fn book_backups(backup: &Path) -> Vec<(PathBuf, Uuid)> {
    let owner = |path: &Path| {
        let name = path.file_name()?.to_str()?;
        Uuid::parse_str(name.split('.').next()?).ok()
    };
    let mut found = Vec::new();
    for entry in std::fs::read_dir(backup).into_iter().flatten().flatten() {
        let path = entry.path();
        if let Some(id) = owner(&path) {
            found.push((path, id));
            continue;
        }
        for inner in std::fs::read_dir(&path).into_iter().flatten().flatten() {
            if let Some(id) = owner(&inner.path()) {
                found.push((inner.path(), id));
            }
        }
    }
    found
}

/// Deletes every set-aside copy of book `id`. Takes no store lock.
pub(crate) fn remove_backups_of(local: &Path, id: Uuid) -> io::Result<()> {
    for (path, owner) in book_backups(&local.join(BACKUP_DIR)) {
        if owner == id {
            remove(&path)?;
        }
    }
    Ok(())
}

fn sweep(store: &LibraryStore, today: i64) -> anyhow::Result<()> {
    let backup = store.local_dir().join(BACKUP_DIR);
    for entry in std::fs::read_dir(&backup).into_iter().flatten().flatten() {
        if let Some(day) = entry.file_name().to_str().and_then(bucket_day)
            && today - day > BACKUP_RETENTION_DAYS as i64
        {
            remove(&entry.path())?;
        }
    }
    let held = book_backups(&backup);
    if held.is_empty() {
        return Ok(());
    }
    let deleted = store.run_blocking(|db| {
        let mut deleted = Vec::new();
        for (path, id) in held {
            if db.knows_book(id)? && !db.has_book(id)? {
                deleted.push(path);
            }
        }
        Ok(deleted)
    })?;
    for path in deleted {
        remove(&path)?;
    }
    Ok(())
}

/// Deletes the backups of books deleted in the store, and day buckets past
/// [`BACKUP_RETENTION_DAYS`]. Blocking, and locks the store: not for use
/// inside `LibraryStore::run`.
pub fn sweep_backups(store: &LibraryStore) -> anyhow::Result<()> {
    sweep(store, today())
}

fn delete_stale(dir: &Path) {
    // Deleted here since: a copy another device still had is deleted too.
    if let Err(err) = std::fs::remove_dir_all(dir) {
        log::warn!("could not delete {}: {err}", dir.display());
    }
}

fn import_book(dir: &Path, id: Uuid, store: &LibraryStore, report: &mut Imported) {
    if in_flight(dir) {
        return;
    }
    let books = named(dir, "book", ".dat");
    let translations = named(dir, "translation_", ".dat");
    let summary_files = named(dir, "chapter_summaries", ".dat");
    if books.is_empty()
        && translations.is_empty()
        && summary_files.is_empty()
        && named(dir, "state", ".json").is_empty()
    {
        return;
    }
    let warn = |what: &str, err: &dyn std::fmt::Display| {
        log::warn!("legacy book at {} {what}: {err:#}", dir.display());
    };

    let known = store.run_blocking(|db| {
        let (knows, has) = (db.knows_book(id)?, db.has_book(id)?);
        let chapters = db
            .list_books()?
            .into_iter()
            .find(|book| book.id == id)
            .map(|book| book.chapters_count);
        Ok((knows && !has, chapters))
    });
    let (deleted, stored_chapters) = match known {
        Ok(known) => known,
        Err(err) => {
            report.failed += 1;
            return warn("could not be looked up", &err);
        }
    };
    if deleted {
        return delete_stale(dir);
    }

    let mut clean = true;
    // Newest readable copy wins, as the old loader chose between conflicts.
    let mut book = None;
    for path in books.iter().rev() {
        match read::<Book>(path) {
            Some(read) if read.id == id => {
                book = Some(read);
                break;
            }
            Some(_) => {
                log::warn!("skipping misfiled legacy book {}", path.display());
                clean = false;
            }
            None => clean = false,
        }
    }
    let Some(chapters) = book.as_ref().map(Book::chapter_count).or(stored_chapters) else {
        // Left for the book to arrive, as a file or through the store.
        if books.is_empty() {
            if first_sight(dir) {
                log::info!("legacy files at {} wait for their book", dir.display());
            }
        } else {
            if first_sight(dir) {
                log::warn!(
                    "legacy book at {} is unreadable; left in place",
                    dir.display()
                );
            }
            report.failed += 1;
        }
        return;
    };

    let mut versions = Vec::new();
    for path in &translations {
        match read::<Translation>(path) {
            Some(translation) => push_versions(&translation, &mut versions),
            None => clean = false,
        }
    }
    let mut summaries = Vec::new();
    for path in &summary_files {
        match read::<ChapterSummaries>(path) {
            Some(loaded) if loaded.book_id == id && loaded.entries.len() == chapters => {
                summaries.extend(
                    loaded
                        .entries
                        .into_iter()
                        .enumerate()
                        .filter(|(_, summary)| summary.generated),
                );
            }
            Some(_) => {
                log::warn!(
                    "skipping legacy summaries of another shape {}",
                    path.display()
                );
                clean = false;
            }
            None => clean = false,
        }
    }
    let (state, state_clean) = user_state(dir);
    clean &= state_clean;
    let delta = BookDelta {
        book: book.map(Arc::new),
        versions,
        summaries,
        reading_state: state.reading_state,
        folder_path: (!state.folder_path.is_empty()).then_some(state.folder_path),
    };

    let outcome = store.run_blocking(|db| {
        if db.knows_book(id)? && !db.has_book(id)? {
            return Ok(None);
        }
        // Staged rows show to readers even when the commit then fails.
        let inserted = db.stage(id, &delta, true)?;
        Ok(Some((inserted, db.publish())))
    });
    drop(delta);
    let stored = match outcome {
        Ok(Some((inserted, stored))) => {
            report.books += usize::from(inserted);
            stored
        }
        Ok(None) => return delete_stale(dir),
        Err(err) => Err(err),
    };
    if let Err(err) = stored {
        report.failed += 1;
        return warn("was not imported; left in place", &err);
    }
    let aside = set_aside(dir, &store.local_dir().join(BACKUP_DIR), !clean);
    if let Err(err) = &aside {
        warn("was imported but could not be set aside", err);
    }
    report.failed += usize::from(aside.is_err() || !clean);
}

fn import_cards(dir: &Path, store: &LibraryStore, report: &mut Imported) {
    if in_flight(dir) {
        return;
    }
    let (cards, clean) = read_cards(dir);
    if cards.is_empty() && clean {
        return;
    }
    let outcome = store.run_blocking(|db| {
        let (mut inserted, mut refused) = (false, false);
        for card in &cards {
            match db.stage_card(&card.source, &card.target, &card.slug, &card.card, None) {
                Ok(changed) => inserted |= changed,
                Err(err) => {
                    log::warn!(
                        "legacy card {} was not imported: {err:#}",
                        card.path.display()
                    );
                    refused = true;
                }
            }
        }
        let published = db.publish();
        Ok((inserted, refused, published))
    });
    let stored = match outcome {
        Ok((inserted, refused, published)) => {
            report.cards |= inserted;
            published.map(|()| !refused)
        }
        Err(err) => Err(err),
    };
    let backup = store.local_dir().join(BACKUP_DIR);
    let done = match stored {
        // A refused card stays in the synced folder to be tried again.
        Ok(false) => false,
        Ok(true) => match set_aside(dir, &backup, !clean) {
            Ok(()) => clean,
            Err(err) => {
                log::warn!(
                    "legacy cards at {} were imported but could not be set aside: {err}",
                    dir.display()
                );
                false
            }
        },
        Err(err) => {
            log::warn!(
                "legacy cards at {} were not imported; left in place: {err:#}",
                dir.display()
            );
            false
        }
    };
    report.failed += usize::from(!done);
}

/// Called with (directories done, directories in all) as an import goes.
pub type Progress = std::sync::Arc<dyn Fn(usize, usize) + Send + Sync>;

fn import(root: &Path, store: &LibraryStore, progress: Option<&Progress>) -> Imported {
    let (book_dirs, cards_dir) = scan(root);
    let mut report = Imported::default();
    let total = book_dirs.len() + usize::from(cards_dir.is_some());
    let tell = |done: usize| {
        if let Some(progress) = progress {
            progress(done, total);
        }
    };
    for (done, (dir, id)) in book_dirs.iter().enumerate() {
        tell(done);
        import_book(dir, *id, store, &mut report);
    }
    if let Some(cards_dir) = &cards_dir {
        tell(book_dirs.len());
        import_cards(cards_dir, store, &mut report);
    }
    if total > 0 {
        tell(total);
    }
    if let Err(err) = sweep_backups(store) {
        log::warn!("legacy backups were not swept: {err:#}");
    }
    if report.any() {
        log::info!(
            "imported legacy files into the store: {} books, cards: {}, {} directories failed",
            report.books,
            report.cards,
            report.failed
        );
    }
    report
}

/// Imports whatever legacy files sit in `root`, one directory at a time; a
/// directory that fails is counted and left for the next pass. Runs at every
/// open and refresh, because a device still on the old format keeps syncing
/// such files in.
pub async fn migrate(root: &Path, store: &LibraryStore) -> anyhow::Result<Imported> {
    migrate_reporting(root, store, None).await
}

/// `migrate`, telling `progress` how far along it is.
pub async fn migrate_reporting(
    root: &Path,
    store: &LibraryStore,
    progress: Option<Progress>,
) -> anyhow::Result<Imported> {
    let (root, store) = (root.to_owned(), store.clone());
    Ok(tokio::task::spawn_blocking(move || import(&root, &store, progress.as_ref())).await?)
}

#[cfg(test)]
mod tests;
