//! The library's persistence: one dbsync store whose per-writer shards live in
//! the synced folder, with the cache and lock kept device-local.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use anyhow::Context;
use dbsync::{
    Id, WriterId,
    schema::{Entity, Layout, RowStatus, Store, Table},
};
use uuid::Uuid;

use crate::{
    book::{
        book::Book,
        chapter_summaries::{ChapterSummaries, ChapterSummary},
        translation_import,
    },
    card::{AnkiData, AnkiState, Card, Example},
    library::library_book::{BookReadingState, BookUserState},
};

mod reader;
pub mod schema;

pub use reader::BookReader;

use schema::{AnkiStateCode, LibraryDb, ROOT, book_id, derived, from_db};

/// Directory of the shards inside the library root; the only part that syncs.
pub const SHARED_DIR: &str = "db";

pub struct StoredTranslation {
    pub id: Uuid,
    pub source_language: String,
    pub target_language: String,
    pub translated_paragraphs_count: usize,
}

pub struct StoredBook {
    pub id: Uuid,
    pub title: String,
    pub chapters_count: usize,
    pub paragraphs_count: usize,
    pub translations: Vec<StoredTranslation>,
    pub folder_path: Vec<String>,
}

/// A book's row: all of it that is held while the book is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookInfo {
    pub id: Uuid,
    pub title: String,
    /// ISO 639-3.
    pub language: String,
    pub(crate) chapters: usize,
    pub(crate) paragraphs: usize,
}

impl BookInfo {
    pub fn chapter_count(&self) -> usize {
        self.chapters
    }

    pub fn paragraphs_count(&self) -> usize {
        self.paragraphs
    }
}

pub struct ChapterOutline {
    pub title: Option<String>,
    /// Flat paragraph numbers, in reading order.
    pub paragraphs: Vec<usize>,
}

/// Which paragraphs make up which chapter; no text.
pub struct Outline {
    pub chapters: Vec<ChapterOutline>,
    chapter_of: Vec<u32>,
}

impl Outline {
    pub fn chapter_for_paragraph(&self, paragraph: usize) -> Option<usize> {
        self.chapter_of.get(paragraph).map(|c| *c as usize)
    }
}

pub struct ParagraphText {
    pub text: String,
    pub html: Option<String>,
}

impl ParagraphText {
    /// The markup if the book has any, else the plain text.
    pub fn original(&self) -> &str {
        self.html.as_deref().unwrap_or(&self.text)
    }
}

/// One whole version of a paragraph's translation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredVersion {
    pub translation: translation_import::ParagraphTranslation,
    pub model: String,
}

/// The newest version of each paragraph of one translation.
#[derive(Default)]
struct Latest {
    by_paragraph: HashMap<u32, Newest>,
    translated: usize,
}

#[derive(Clone, Copy)]
struct Newest {
    timestamp: u64,
    id: Id<schema::ParagraphTranslation>,
    empty: bool,
}

impl Latest {
    fn note(&mut self, paragraph: u32, version: Newest) {
        match self.by_paragraph.get_mut(&paragraph) {
            Some(held) if (held.timestamp, held.id) > (version.timestamp, version.id) => {}
            Some(held) => {
                self.translated -= usize::from(!held.empty);
                self.translated += usize::from(!version.empty);
                *held = version;
            }
            None => {
                self.translated += usize::from(!version.empty);
                self.by_paragraph.insert(paragraph, version);
            }
        }
    }
}

/// What a refresh changed in one book.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BookChange {
    pub paragraphs: BTreeSet<usize>,
    pub summaries: bool,
    pub user_state: bool,
}

/// What adopting other devices' rows changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Adopted {
    /// The list of books, their folders or their translation counts.
    pub shelf: bool,
    pub books: HashMap<Uuid, BookChange>,
    pub purged: Vec<Uuid>,
    pub cards: bool,
}

impl Adopted {
    pub fn any(&self) -> bool {
        self.shelf || self.cards || !self.books.is_empty() || !self.purged.is_empty()
    }
}

/// Shard files this device cannot use. None of it is fatal; all of it means
/// some other device's data is not showing here.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StoreHealth {
    /// Unreadable for many refreshes in a row, so not merely mid-transfer.
    pub unreadable: Vec<PathBuf>,
    /// Written by a newer version of the app.
    pub incompatible: Vec<PathBuf>,
    /// Not shards at all and not going away, such as a sync conflict copy.
    pub strangers: Vec<PathBuf>,
    /// Rows of another device that cannot be applied, refresh after refresh:
    /// most likely written by a newer version of the app.
    pub stalled: bool,
    /// Another installation wrote under this one's identity, as a copy
    /// restored from this device's backup does. The store takes a new
    /// writer by itself; the sync identity they also share is the user's
    /// to settle, and this stays set until they have.
    pub shared_identity: bool,
}

/// Rows `Inner::maintain` dropped.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Maintained {
    pub pruned: usize,
    pub scrubbed: usize,
}

/// Refreshes a file may stay unusable before it is reported: one being
/// delivered is unreadable, or oddly named, for a while.
const ODD_GRACE: u32 = 6;

#[derive(Clone)]
pub struct NewVersion {
    pub source_language: String,
    pub target_language: String,
    pub paragraph: usize,
    pub translation: translation_import::ParagraphTranslation,
    pub model: String,
}

/// Everything one book wants written, applied in a single commit. Every part
/// is idempotent, so a delta that failed halfway is simply applied again.
#[derive(Default)]
pub struct BookDelta {
    pub book: Option<Arc<Book>>,
    pub versions: Vec<NewVersion>,
    pub summaries: Vec<(usize, ChapterSummary)>,
    pub reading_state: Option<BookReadingState>,
    pub folder_path: Option<Vec<String>>,
}

impl BookDelta {
    pub fn is_empty(&self) -> bool {
        self.book.is_none()
            && self.versions.is_empty()
            && self.summaries.is_empty()
            && self.reading_state.is_none()
            && self.folder_path.is_none()
    }
}

#[derive(Clone)]
pub struct LibraryStore {
    /// `None` once closed: every clone refuses work from then on.
    inner: Arc<RwLock<Option<Inner>>>,
    /// Held shared by whoever may change the store, and alone by a
    /// compaction, which works on a copy while readers go on with this one.
    writers: Arc<RwLock<()>>,
    shared: PathBuf,
    local: PathBuf,
    /// This device's writer now; a store told to start over has a new one.
    writer: Arc<std::sync::atomic::AtomicU64>,
}

pub struct Inner {
    db: LibraryDb,
    writer: WriterId,
    unpublished: bool,
    /// Read caches, dropped whenever rows arrive or leave by another way
    /// than `insert_version`.
    latest: Mutex<HashMap<dbsync::Uuid, Latest>>,
    outlines: Mutex<HashMap<Uuid, Arc<Outline>>>,
    /// Books whose whole content is here; content never changes after that.
    complete: Mutex<HashSet<Uuid>>,
    /// Books an import added to since the last refresh reported.
    imported_into: BTreeSet<Uuid>,
    odd_for: HashMap<PathBuf, u32>,
    waiting_for: u32,
    health: StoreHealth,
    local: PathBuf,
    last_write: Instant,
    /// A new writer is due at the next opening.
    rotating: bool,
    shared_identity: bool,
    #[cfg(test)]
    pub(crate) fail_commits: usize,
}

fn fnv(bytes: &[u8]) -> u64 {
    use std::hash::Hasher;
    let mut hasher = fnv::FnvHasher::default();
    hasher.write(bytes);
    hasher.finish()
}

/// This installation's writer: random, minted once, never synced.
pub(crate) fn writer_id(local: &Path) -> anyhow::Result<WriterId> {
    let path = local.join(WRITER_FILE);
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(raw) = u64::from_str_radix(text.trim(), 16)
        && raw != 0
    {
        return Ok(WriterId::from_raw(raw));
    }
    let raw = loop {
        let raw: u64 = rand::random();
        if raw != 0 {
            break raw;
        }
    };
    let temp = local.join("writer-id.tmp");
    std::fs::write(&temp, format!("{raw:016x}"))?;
    std::fs::rename(&temp, &path)?;
    Ok(WriterId::from_raw(raw))
}

const WRITER_FILE: &str = "writer-id";

/// Left where the writer id is by a session that found its identity shared;
/// the next opening starts over as a new writer.
const ROTATE_FILE: &str = "rotate-writer";

/// Stays, across the restart that takes a new writer, until the user has
/// seen to the sync identity the two installations also share.
const SHARED_FILE: &str = "identity-shared";

/// Forgets this installation's writer, and the caches built for it. What it
/// wrote stays in its shards, now another writer's as far as this one cares.
fn rotate_writer(local: &Path, cache: &Path) -> anyhow::Result<()> {
    log::warn!("another installation shares this one's identity; taking a new one");
    let gone = |result: std::io::Result<()>| match result {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    };
    gone(std::fs::remove_file(cache))?;
    gone(std::fs::remove_dir_all(
        local.join(crate::sync::roster::LOCAL_DIR),
    ))?;
    gone(std::fs::remove_file(local.join(WRITER_FILE)))?;
    gone(std::fs::remove_file(local.join(ROTATE_FILE)))?;
    Ok(())
}

/// Where device-local state goes when the root's parent is writable.
pub fn sibling_local_dir(root: &Path) -> Option<PathBuf> {
    let mut local = std::ffi::OsString::from(".");
    local.push(root.file_name()?);
    local.push(".flts-local");
    Some(root.parent()?.join(local))
}

/// Device-local state lives beside the root, so it can never sync; inside it
/// only when the parent is unwritable. Whichever already names this device's
/// writer is kept, so a passing failure cannot mint a second identity.
pub fn local_dir(root: &Path) -> PathBuf {
    let (sibling, inside) = (sibling_local_dir(root), root.join(".flts-local"));
    let named = |dir: &Path| dir.join(WRITER_FILE).is_file();
    match sibling {
        Some(sibling) if named(&sibling) => sibling,
        _ if named(&inside) => inside,
        Some(sibling) if std::fs::create_dir_all(&sibling).is_ok() => sibling,
        _ => inside,
    }
}

impl LibraryStore {
    pub fn open(root: &Path) -> anyhow::Result<Self> {
        let shared = root.join(SHARED_DIR);
        let local = local_dir(root);
        std::fs::create_dir_all(&shared)?;
        std::fs::create_dir_all(&local)?;
        let layout = Layout::split(&shared, &local);
        let mut rotated = false;
        let (db, writer) = loop {
            if local.join(ROTATE_FILE).exists() {
                rotate_writer(&local, &layout.cache(LibraryDb::SCHEMA_VERSION))?;
            }
            let writer = writer_id(&local)?;
            let mut db = LibraryDb::open_or_create(layout.clone(), writer)?;
            // A commit that crosses the threshold would take as long as the
            // rewrite; `LibraryStore::compact_if_idle` picks the moment.
            db.set_compaction_policy(dbsync::CompactionPolicy::Disabled)?;
            // The shards may have been replaced while this was not running.
            if db.check_own_shards(writer)?.forked && !rotated {
                drop(db);
                std::fs::write(local.join(ROTATE_FILE), b"")?;
                std::fs::write(local.join(SHARED_FILE), b"")?;
                rotated = true;
                continue;
            }
            break (db, writer);
        };
        Ok(Self {
            inner: Arc::new(RwLock::new(Some(Inner {
                db,
                writer,
                unpublished: false,
                latest: Mutex::default(),
                outlines: Mutex::default(),
                complete: Mutex::default(),
                imported_into: BTreeSet::new(),
                odd_for: HashMap::new(),
                waiting_for: 0,
                health: StoreHealth::default(),
                local: local.clone(),
                last_write: Instant::now(),
                rotating: false,
                shared_identity: local.join(SHARED_FILE).exists(),
                #[cfg(test)]
                fail_commits: 0,
            }))),
            writers: Arc::default(),
            shared,
            local,
            writer: Arc::new(std::sync::atomic::AtomicU64::new(writer.raw())),
        })
    }

    fn writer(&self) -> u64 {
        self.writer.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Starts over as a new writer, at once: for when the user has this
    /// installation give up an identity it shares with another. Nothing else
    /// may hold the local state open, the roster store included.
    pub fn start_over_as_new_writer(&self) -> anyhow::Result<()> {
        let layout = Layout::split(&self.shared, &self.local);
        let writer = self.run_blocking(|db| {
            db.publish()?;
            // Closes the files before they are removed.
            db.db = LibraryDb::default();
            rotate_writer(&db.local, &layout.cache(LibraryDb::SCHEMA_VERSION))?;
            let writer = writer_id(&db.local)?;
            let mut opened = LibraryDb::open_or_create(layout.clone(), writer)?;
            opened.set_compaction_policy(dbsync::CompactionPolicy::Disabled)?;
            db.db = opened;
            db.writer = writer;
            db.rotating = false;
            db.forget_read_caches();
            Ok(writer)
        })?;
        self.writer
            .store(writer.raw(), std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    pub fn shared_dir(&self) -> &Path {
        &self.shared
    }

    pub fn local_dir(&self) -> &Path {
        &self.local
    }

    /// Whether `path` is one of this device's own shards, whose changes are
    /// echoes of our own commits.
    pub fn is_own_file(&self, path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(&format!("{:016x}.", self.writer())))
    }

    /// Deletes the book's data for real, the set-aside legacy copy included.
    pub async fn delete_book(&self, id: Uuid) -> anyhow::Result<()> {
        let local = self.local.clone();
        self.run(move |db| {
            db.delete_book(id)?;
            Ok(crate::legacy::remove_backups_of(&local, id)?)
        })
        .await
    }

    /// Runs `f` on the blocking pool: every store call may touch the disk.
    pub async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Inner) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.run_blocking(f)).await?
    }

    /// Like `run`, for reads: these do not wait for each other.
    pub async fn read<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Inner) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            match &*inner.read().unwrap_or_else(|e| e.into_inner()) {
                Some(db) => f(db),
                None => Err(closed()),
            }
        })
        .await?
    }

    pub fn run_blocking<T>(
        &self,
        f: impl FnOnce(&mut Inner) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _writing = self.writers.read().unwrap_or_else(|e| e.into_inner());
        match &mut *self.inner.write().unwrap_or_else(|e| e.into_inner()) {
            Some(db) => f(db),
            None => Err(closed()),
        }
    }

    /// Compacts the files that have enough to reclaim, unless something was
    /// written in the last `idle`. Blocking, and long when it does compact:
    /// writers wait for it, readers do not. True when it looked.
    pub fn compact_if_idle(&self, idle: Duration) -> anyhow::Result<bool> {
        let _alone = self.writers.write().unwrap_or_else(|e| e.into_inner());
        let mut copy = match &*self.inner.read().unwrap_or_else(|e| e.into_inner()) {
            Some(db) if !db.unpublished && db.last_write.elapsed() >= idle => db.db.snapshot(),
            _ => return Ok(false),
        };
        copy.compact_by(
            WriterId::from_raw(self.writer()),
            dbsync::CompactionPolicy::default(),
        )?;
        if let Some(db) = &mut *self.inner.write().unwrap_or_else(|e| e.into_inner()) {
            db.db = copy;
        }
        Ok(true)
    }

    /// Lets go of the store for every clone. Whoever holds the library's
    /// lease calls this before giving it up, so no straggler writes after
    /// another session may have opened the same files.
    pub async fn close(&self) {
        let inner = self.inner.clone();
        let _ = tokio::task::spawn_blocking(move || {
            inner.write().unwrap_or_else(|e| e.into_inner()).take()
        })
        .await;
    }
}

fn closed() -> anyhow::Error {
    anyhow::anyhow!("library session closed")
}

fn u32_of(value: usize) -> anyhow::Result<u32> {
    u32::try_from(value).context("index does not fit the store")
}

pub fn translation_uuid(book: Uuid, source: &str, target: &str) -> Uuid {
    from_db(
        derived::<schema::Translation>(
            schema::to_db(book),
            &format!("translation/{source}/{target}"),
        )
        .uuid(),
    )
}

fn card_row_id(card_id: &str) -> Id<schema::Card> {
    derived(ROOT, &format!("card/{card_id}"))
}

fn state_code(state: AnkiState) -> AnkiStateCode {
    match state {
        AnkiState::Active => AnkiStateCode::Active,
        AnkiState::Suspended => AnkiStateCode::Suspended,
        AnkiState::Deleted => AnkiStateCode::Deleted,
    }
}

impl Inner {
    fn commit(&mut self) -> anyhow::Result<()> {
        #[cfg(test)]
        if self.fail_commits > 0 {
            self.fail_commits -= 1;
            anyhow::bail!("injected commit failure");
        }
        self.last_write = Instant::now();
        self.db.commit(self.writer)?;
        self.unpublished = false;
        Ok(())
    }

    /// Adopts whatever other devices' shards now hold.
    pub fn refresh(&mut self) -> anyhow::Result<Adopted> {
        if self.unpublished {
            self.commit()?;
        }
        // Before reading: a replaced shard of ours may lack what we wrote.
        let own = self.db.check_own_shards(self.writer)?;
        if own.restored > 0 {
            log::info!("{} rows restored to this device's shards", own.restored);
        }
        let report = self.db.refresh_peers()?;
        if (own.forked || report.forked.contains(&self.writer)) && !self.rotating {
            log::warn!("another installation writes under this one's identity");
            // Rows keep going out under the shared identity until the restart.
            self.rotating = true;
            self.shared_identity = true;
            for file in [ROTATE_FILE, SHARED_FILE] {
                if let Err(err) = std::fs::write(self.local.join(file), b"") {
                    log::warn!("could not note the shared identity: {err}");
                }
            }
        }
        self.note_health(&report);
        let mut adopted = Adopted::default();
        let rows = report.adopted.iter().copied().chain(
            report
                .visibility
                .iter()
                .map(|change| (change.entity, change.id)),
        );
        for (entity, id) in rows {
            self.classify(entity, id, &mut adopted);
        }
        for change in &report.visibility {
            if change.entity == schema::Book::NAME && change.after == RowStatus::Deleted {
                adopted.purged.push(from_db(change.id));
            }
        }
        // A purge learned from a peer: our copies of those rows go now.
        self.db.reclaim_purged(self.writer)?;
        adopted.purged.sort_unstable();
        adopted.purged.dedup();
        for id in &adopted.purged {
            adopted.books.remove(id);
        }
        if adopted.any() || own.restored > 0 {
            self.forget_read_caches();
        }
        for id in std::mem::take(&mut self.imported_into) {
            if !adopted.purged.contains(&id) {
                adopted.books.entry(id).or_default();
            }
        }
        Ok(adopted)
    }

    /// Files `(entity, id)` under what a reader of it has to look at again.
    /// A row that cannot be read yet says nothing: it is not showing either.
    fn classify(&self, entity: &str, id: dbsync::Uuid, adopted: &mut Adopted) {
        let db = &self.db;
        let book_of = |book: Option<Id<schema::Book>>| book.map(|book| from_db(book.uuid()));
        if entity == schema::Book::NAME
            || entity == schema::Chapter::NAME
            || entity == schema::Paragraph::NAME
        {
            adopted.shelf = true;
        } else if entity == schema::Translation::NAME {
            adopted.shelf = true;
            if let Some(book) = book_of(db.translations.book(Id::from_uuid(id)).ok()) {
                adopted.books.entry(book).or_default();
            }
        } else if entity == schema::ParagraphTranslation::NAME {
            adopted.shelf = true;
            let version = Id::from_uuid(id);
            let table = &db.paragraph_translations;
            let book = table
                .translation(version)
                .ok()
                .and_then(|translation| db.translations.book(translation).ok());
            if let (Some(book), Ok(paragraph)) = (book_of(book), table.paragraph(version)) {
                let change = adopted.books.entry(book).or_default();
                change.paragraphs.insert(paragraph as usize);
            }
        } else if entity == schema::ChapterSummary::NAME {
            if let Some(book) = book_of(db.chapter_summaries.book(Id::from_uuid(id)).ok()) {
                adopted.books.entry(book).or_default().summaries = true;
            }
        } else if entity == schema::ReadingPosition::NAME
            || entity == schema::ImportedPosition::NAME
        {
            let book = match entity == schema::ReadingPosition::NAME {
                true => db.reading_positions.book(Id::from_uuid(id)).ok(),
                false => db.imported_positions.book(Id::from_uuid(id)).ok(),
            };
            if let Some(book) = book_of(book) {
                adopted.books.entry(book).or_default().user_state = true;
            }
        } else if entity == schema::BookFolder::NAME || entity == schema::ImportedFolder::NAME {
            adopted.shelf = true;
            let book = match entity == schema::BookFolder::NAME {
                true => db.book_folders.book(Id::from_uuid(id)).ok(),
                false => db.imported_folders.book(Id::from_uuid(id)).ok(),
            };
            if let Some(book) = book_of(book) {
                adopted.books.entry(book).or_default().user_state = true;
            }
        } else {
            adopted.cards = true;
        }
    }

    fn note_health(&mut self, report: &dbsync::schema::MergeReport) {
        // Files in transit have dotted temporary names and are gone by the
        // next look; only what stays put is worth a word.
        let lasting = |path: &&PathBuf| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| !name.starts_with('.'))
        };
        let strangers: Vec<PathBuf> = report.strangers.iter().filter(lasting).cloned().collect();
        let odd = || report.unreadable.iter().chain(&strangers);
        self.odd_for
            .retain(|path, _| odd().any(|held| held == path));
        for path in odd() {
            *self.odd_for.entry(path.clone()).or_default() += 1;
        }
        let stuck = |paths: &[PathBuf]| -> Vec<PathBuf> {
            paths
                .iter()
                .filter(|path| self.odd_for.get(*path).is_some_and(|n| *n >= ODD_GRACE))
                .cloned()
                .collect()
        };
        self.waiting_for = match report.waiting {
            0 => 0,
            _ => self.waiting_for.saturating_add(1),
        };
        let health = StoreHealth {
            unreadable: stuck(&report.unreadable),
            incompatible: report.incompatible.clone(),
            strangers: stuck(&strangers),
            stalled: self.waiting_for >= ODD_GRACE,
            shared_identity: self.shared_identity,
        };
        if health != self.health {
            if health.stalled && !self.health.stalled {
                log::warn!("rows of another device stay unapplied; it may run a newer version");
            }
            for writer in report.forked.iter().filter(|w| **w != self.writer) {
                log::warn!("two installations write as {:016x}", writer.raw());
            }
            for path in &health.unreadable {
                log::warn!("shard stays unreadable: {}", path.display());
            }
            for path in &health.incompatible {
                log::warn!("shard from a newer app version skipped: {}", path.display());
            }
            for path in &health.strangers {
                log::warn!("not a shard, ignored: {}", path.display());
            }
            self.health = health;
        }
    }

    pub fn health(&self) -> StoreHealth {
        StoreHealth {
            shared_identity: self.shared_identity,
            ..self.health.clone()
        }
    }

    /// Another installation is known to share this one's identity, though
    /// no row of its has been met: the user tried to pair the two.
    pub fn note_shared_identity(&mut self) -> anyhow::Result<()> {
        self.shared_identity = true;
        Ok(std::fs::write(self.local.join(SHARED_FILE), b"")?)
    }

    /// The user has dealt with the identity two installations shared.
    pub fn settle_shared_identity(&mut self) -> anyhow::Result<()> {
        self.shared_identity = false;
        match std::fs::remove_file(self.local.join(SHARED_FILE)) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        }
    }

    pub fn prune(&mut self, retired: &[u64]) -> anyhow::Result<usize> {
        self.publish()?;
        let others: Vec<WriterId> = self
            .db
            .peers()?
            .into_iter()
            .map(|peer| peer.writer)
            .filter(|writer| *writer != self.writer && !retired.contains(&writer.raw()))
            .collect();
        let report = self.db.prune(self.writer, &others)?;
        Ok(report.dropped)
    }

    pub fn writer(&self) -> u64 {
        self.writer.raw()
    }

    /// Housekeeping no reader waits for: drops superseded revisions, and
    /// scrubs deleted books out of the shards of `retired` writers, which
    /// will never do it themselves.
    pub fn maintain(&mut self, retired: &[u64]) -> anyhow::Result<Maintained> {
        let pruned = self.prune(retired)?;
        let mut scrubbed = 0;
        // Two devices rewriting one file would conflict in transit, so only
        // the lowest writer still in use scrubs.
        let live = self
            .db
            .peers()?
            .into_iter()
            .map(|peer| peer.writer.raw())
            .filter(|writer| !retired.contains(writer));
        if live.chain([self.writer.raw()]).min() == Some(self.writer.raw()) {
            for writer in retired {
                scrubbed += self
                    .db
                    .scrub_purged(self.writer, WriterId::from_raw(*writer))?;
            }
        }
        Ok(Maintained { pruned, scrubbed })
    }

    /// True once the book's row and all of its content are here. A row alone
    /// is a book still arriving, or one whose import was cut short.
    fn is_complete(&self, row: Id<schema::Book>) -> anyhow::Result<bool> {
        let id = from_db(row.uuid());
        let mut complete = self.complete.lock().unwrap_or_else(|e| e.into_inner());
        if complete.contains(&id) {
            return Ok(true);
        }
        let chapters = self.db.children_of::<schema::Book, schema::Chapter>(row)?;
        let mut paragraphs = 0;
        for chapter in &chapters {
            paragraphs += self
                .db
                .children_of::<schema::Chapter, schema::Paragraph>(*chapter)?
                .len();
        }
        let whole = chapters.len() == self.db.books.chapters_count(row)? as usize
            && paragraphs == self.db.books.paragraphs_count(row)? as usize;
        if whole {
            complete.insert(id);
        }
        Ok(whole)
    }

    pub fn is_purged(&self, id: Uuid) -> anyhow::Result<bool> {
        Ok(self.db.is_purged::<schema::Book>(book_id(id))?)
    }

    pub fn has_book(&self, id: Uuid) -> anyhow::Result<bool> {
        Ok(self.db.books.live_row_of(book_id(id))?.is_some())
    }

    /// True for a deleted book too: its id is spent.
    pub fn knows_book(&self, id: Uuid) -> anyhow::Result<bool> {
        Ok(!self.db.books.versions_of(book_id(id))?.is_empty())
    }

    pub fn has_title(&self, title: &str) -> anyhow::Result<bool> {
        for book in self.db.all::<schema::Book>() {
            if book.title()? == title && self.is_complete(book.id()?)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn imported_folder(&self, book: Id<schema::Book>) -> anyhow::Result<Vec<String>> {
        let id = derived::<schema::ImportedFolder>(book.uuid(), "imported-folder");
        if self.db.imported_folders.live_row_of(id)?.is_none() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_str(&self.db.imported_folders.path(id)?).unwrap_or_default())
    }

    /// The folder the app itself stored, else the one an import brought.
    fn folder_path(&self, book: Id<schema::Book>) -> anyhow::Result<Vec<String>> {
        let id = derived::<schema::BookFolder>(book.uuid(), "folder");
        if self.db.book_folders.live_row_of(id)?.is_none() {
            return self.imported_folder(book);
        }
        Ok(serde_json::from_str(&self.db.book_folders.path(id)?).unwrap_or_default())
    }

    fn imported_position(
        &self,
        book: Id<schema::Book>,
    ) -> anyhow::Result<Option<BookReadingState>> {
        let id = derived::<schema::ImportedPosition>(book.uuid(), "imported-reading");
        if self.db.imported_positions.live_row_of(id)?.is_none() {
            return Ok(None);
        }
        let positions = &self.db.imported_positions;
        Ok(Some(BookReadingState {
            chapter_id: positions.chapter(id)? as usize,
            paragraph_id: positions.paragraph(id)? as usize,
            page_offset: positions.page_offset(id)? as usize,
        }))
    }

    /// The place the app itself stored, else the one an import brought.
    fn reading_state(&self, book: Id<schema::Book>) -> anyhow::Result<Option<BookReadingState>> {
        let id = derived::<schema::ReadingPosition>(book.uuid(), "reading");
        if self.db.reading_positions.live_row_of(id)?.is_none() {
            return self.imported_position(book);
        }
        let positions = &self.db.reading_positions;
        Ok(Some(BookReadingState {
            chapter_id: positions.chapter(id)? as usize,
            paragraph_id: positions.paragraph(id)? as usize,
            page_offset: positions.page_offset(id)? as usize,
        }))
    }

    pub fn user_state(&self, id: Uuid) -> anyhow::Result<BookUserState> {
        let book = book_id(id);
        Ok(BookUserState {
            reading_state: self.reading_state(book)?,
            folder_path: self.folder_path(book)?,
        })
    }

    fn translated_count(&self, translation: Id<schema::Translation>) -> anyhow::Result<usize> {
        self.with_latest(translation, |latest| latest.translated)
    }

    pub fn list_books(&self) -> anyhow::Result<Vec<StoredBook>> {
        let mut ids = Vec::new();
        for book in self.db.all::<schema::Book>() {
            if self.is_complete(book.id()?)? {
                ids.push(book.id()?);
            }
        }
        let mut books = Vec::with_capacity(ids.len());
        for id in ids {
            let mut translations = Vec::new();
            for translation in self
                .db
                .children_of::<schema::Book, schema::Translation>(id)?
            {
                translations.push(StoredTranslation {
                    id: from_db(translation.uuid()),
                    source_language: self
                        .db
                        .translations
                        .source_language(translation)?
                        .into_owned(),
                    target_language: self
                        .db
                        .translations
                        .target_language(translation)?
                        .into_owned(),
                    translated_paragraphs_count: self.translated_count(translation)?,
                });
            }
            books.push(StoredBook {
                id: from_db(id.uuid()),
                title: self.db.books.title(id)?.into_owned(),
                chapters_count: self.db.books.chapters_count(id)? as usize,
                paragraphs_count: self.db.books.paragraphs_count(id)? as usize,
                translations,
                folder_path: self.folder_path(id)?,
            });
        }
        Ok(books)
    }

    fn load_summaries(&self, id: Uuid, chapters: usize) -> anyhow::Result<ChapterSummaries> {
        let mut summaries = ChapterSummaries::empty_for(id, chapters);
        let table = &self.db.chapter_summaries;
        let mut newest: HashMap<usize, (u64, Id<schema::ChapterSummary>)> = HashMap::new();
        for summary in self
            .db
            .children_of::<schema::Book, schema::ChapterSummary>(book_id(id))?
        {
            let chapter = table.chapter(summary)? as usize;
            let key = (table.timestamp(summary)?, summary);
            if chapter < chapters && newest.get(&chapter).is_none_or(|held| *held < key) {
                newest.insert(chapter, key);
            }
        }
        for (chapter, (timestamp, summary)) in newest {
            summaries.entries[chapter] = ChapterSummary {
                generated: true,
                model: table.model(summary)?.into_owned(),
                timestamp,
                text: table.text(summary)?.into_owned(),
            };
        }
        Ok(summaries)
    }

    /// Inserts whatever of the book is missing, so a book cut short by a
    /// failed commit is completed by writing it again.
    fn insert_book(&mut self, book: &Book) -> anyhow::Result<bool> {
        let row = book_id(book.id);
        if self.is_complete(row).unwrap_or(false) || self.db.is_purged::<schema::Book>(row)? {
            return Ok(false);
        }
        for chapter in book.chapter_views() {
            let chapter_id =
                derived::<schema::Chapter>(row.uuid(), &format!("chapter/{}", chapter.idx));
            for paragraph in chapter.paragraphs() {
                let id = derived::<schema::Paragraph>(
                    row.uuid(),
                    &format!("paragraph/{}", paragraph.id),
                );
                if self.db.paragraphs.versions_of(id)?.is_empty() {
                    self.db.paragraphs.insert(
                        id,
                        chapter_id,
                        u32_of(paragraph.id)?,
                        &paragraph.original_text,
                        paragraph.original_html.as_deref(),
                    )?;
                }
            }
            if self.db.chapters.versions_of(chapter_id)?.is_empty() {
                self.db.chapters.insert(
                    chapter_id,
                    row,
                    u32_of(chapter.idx)?,
                    chapter.title.as_deref(),
                )?;
            }
        }
        if self.db.books.versions_of(row)?.is_empty() {
            self.db.books.insert(
                row,
                &book.title,
                &book.language,
                u32_of(book.chapter_count())?,
                u32_of(book.paragraphs_count())?,
            )?;
        }
        Ok(true)
    }

    fn insert_version(&mut self, book: Uuid, version: &NewVersion) -> anyhow::Result<bool> {
        let mut inserted = false;
        let translation = Id::<schema::Translation>::from_uuid(schema::to_db(translation_uuid(
            book,
            &version.source_language,
            &version.target_language,
        )));
        if self.db.translations.versions_of(translation)?.is_empty() {
            self.db.translations.insert(
                translation,
                book_id(book),
                &version.source_language,
                &version.target_language,
            )?;
        }
        let (words, details) = crate::book::words::split(&version.translation)?;
        let id = derived::<schema::ParagraphTranslation>(
            translation.uuid(),
            &format!(
                "{}/{}/{:016x}",
                version.paragraph,
                version.translation.timestamp,
                fnv(&[words.as_slice(), details.as_slice()].concat())
            ),
        );
        if self.db.paragraph_translations.versions_of(id)?.is_empty() {
            inserted = true;
            let paragraph = u32_of(version.paragraph)?;
            let empty = version.translation.sentences.is_empty();
            self.db.paragraph_translations.insert(
                id,
                translation,
                paragraph,
                version.translation.timestamp,
                &version.model,
                version.translation.total_tokens,
                empty,
                &words,
                &details,
            )?;
            let latest = self.latest.get_mut().unwrap_or_else(|e| e.into_inner());
            if let Some(latest) = latest.get_mut(&translation.uuid()) {
                latest.note(
                    paragraph,
                    Newest {
                        timestamp: version.translation.timestamp,
                        id,
                        empty,
                    },
                );
            }
        }
        Ok(inserted)
    }

    /// A translation made on this device, staged. Stored versions order by
    /// stamp, so it is given one newer than what it replaces.
    pub fn add_version(&mut self, book: Uuid, mut version: NewVersion) -> anyhow::Result<()> {
        let info = self.book_info(book)?;
        anyhow::ensure!(
            version.paragraph < info.paragraphs,
            "paragraph {} is out of range",
            version.paragraph
        );
        let paragraph = u32_of(version.paragraph)?;
        if let Some(translation) = self.translation_row(book, &version.target_language)?
            && let Some(held) =
                self.with_latest(translation, |l| l.by_paragraph.get(&paragraph).copied())?
            && version.translation.timestamp <= held.timestamp
        {
            version.translation.timestamp = held.timestamp + 1;
        }
        self.unpublished = true;
        self.insert_version(book, &version)?;
        Ok(())
    }

    fn insert_summary(
        &mut self,
        book: Uuid,
        chapter: usize,
        summary: &ChapterSummary,
    ) -> anyhow::Result<bool> {
        if !summary.generated {
            return Ok(false);
        }
        let row = book_id(book);
        let digest = fnv(format!("{}\n{}", summary.model, summary.text).as_bytes());
        let id = derived::<schema::ChapterSummary>(
            row.uuid(),
            &format!("summary/{chapter}/{}/{digest:016x}", summary.timestamp),
        );
        if !self.db.chapter_summaries.versions_of(id)?.is_empty() {
            return Ok(false);
        }
        self.db.chapter_summaries.insert(
            id,
            row,
            u32_of(chapter)?,
            summary.timestamp,
            &summary.model,
            &summary.text,
        )?;
        Ok(true)
    }

    /// Stages `delta` without committing. `only_absent` is an import of
    /// older data: its reading position and folder go where anything the
    /// app wrote itself outranks them. True when it added anything.
    pub fn stage(
        &mut self,
        id: Uuid,
        delta: &BookDelta,
        only_absent: bool,
    ) -> anyhow::Result<bool> {
        self.unpublished = true;
        let mut added = false;
        if let Some(book) = &delta.book {
            added |= self.insert_book(book)?;
        }
        for version in &delta.versions {
            added |= self.insert_version(id, version)?;
        }
        for (chapter, summary) in &delta.summaries {
            added |= self.insert_summary(id, *chapter, summary)?;
        }
        let row = book_id(id);
        if let Some(state) = &delta.reading_state {
            let (chapter, paragraph, offset) = (
                u32_of(state.chapter_id)?,
                u32_of(state.paragraph_id)?,
                u32_of(state.page_offset)?,
            );
            if only_absent {
                if self.imported_position(row)?.as_ref() != Some(state) {
                    added = true;
                    self.db
                        .put_imported_position(row, chapter, paragraph, offset)?;
                }
            } else if self.reading_state(row)?.as_ref() != Some(state) {
                added = true;
                self.db
                    .put_reading_position(row, chapter, paragraph, offset)?;
            }
        }
        if let Some(path) = &delta.folder_path {
            let json = serde_json::to_string(path)?;
            if only_absent {
                if &self.imported_folder(row)? != path {
                    added = true;
                    self.db.put_imported_folder(row, &json)?;
                }
            } else if &self.folder_path(row)? != path {
                added = true;
                self.db.put_book_folder(row, &json)?;
            }
        }
        if added && only_absent {
            self.imported_into.insert(id);
        }
        Ok(added)
    }

    /// Nobody was reading while the library opened, so what its import
    /// added is news to no one.
    pub fn forget_imported(&mut self) {
        self.imported_into.clear();
    }

    /// A summary made on this device, staged. Refused when a summary before
    /// `chapter` is not what it was written from; given a stamp newer than
    /// the one it replaces, because the newest stamp is the current summary.
    pub fn set_summary(
        &mut self,
        book: Uuid,
        chapter: usize,
        written_from: &[ChapterSummary],
        mut summary: ChapterSummary,
    ) -> anyhow::Result<()> {
        let held = self.book(book)?.summaries()?;
        anyhow::ensure!(
            chapter < held.entries.len() && held.entries[..chapter] == *written_from,
            "summary context changed"
        );
        let replaced = &held.entries[chapter];
        if replaced.generated && summary.timestamp <= replaced.timestamp {
            summary.timestamp = replaced.timestamp + 1;
        }
        self.unpublished = true;
        self.insert_summary(book, chapter, &summary)?;
        Ok(())
    }

    /// The reader's place or the book's folder, staged; a book that is gone
    /// takes neither.
    pub fn set_user_state(
        &mut self,
        book: Uuid,
        reading_state: Option<BookReadingState>,
        folder_path: Option<Vec<String>>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(self.has_book(book)?, "book {book} not found");
        let delta = BookDelta {
            reading_state,
            folder_path,
            ..BookDelta::default()
        };
        self.stage(book, &delta, false)?;
        Ok(())
    }

    /// A book of the same title and shape whose content is not all here: an
    /// import of it that was cut short, or the same book still arriving from
    /// another device. Writing it again under that id completes it.
    fn unfinished_twin(&self, book: &Book) -> anyhow::Result<Option<Uuid>> {
        for held in self.db.all::<schema::Book>() {
            let row = held.id()?;
            if held.title()? == book.title
                && held.language()? == book.language
                && held.chapters_count()? as usize == book.chapter_count()
                && held.paragraphs_count()? as usize == book.paragraphs_count()
                && !self.is_complete(row)?
            {
                return Ok(Some(from_db(row.uuid())));
            }
        }
        Ok(None)
    }

    /// Adds a new book in one commit and returns the id it has, which is
    /// that of its unfinished twin if there is one. A failed commit leaves
    /// it staged, to be published by the next one.
    pub fn create_book(&mut self, book: Arc<Book>) -> anyhow::Result<Uuid> {
        if self.has_title(&book.title)? {
            return Err(crate::library::LibraryError::DuplicateTitle(book.title.clone()).into());
        }
        let book = match self.unfinished_twin(&book)? {
            Some(id) => {
                let mut same = (*book).clone();
                same.id = id;
                Arc::new(same)
            }
            None => book,
        };
        let id = book.id;
        self.persist(
            id,
            &BookDelta {
                book: Some(book),
                ..BookDelta::default()
            },
        )?;
        Ok(id)
    }

    pub fn has_unpublished(&self) -> bool {
        self.unpublished
    }

    pub fn publish(&mut self) -> anyhow::Result<()> {
        if self.unpublished {
            self.commit()?;
        }
        Ok(())
    }

    pub fn persist(&mut self, id: Uuid, delta: &BookDelta) -> anyhow::Result<()> {
        self.stage(id, delta, false)?;
        self.commit()
    }

    /// Purges the book and everything that hangs off it, then drops the
    /// rows this device wrote from its shards. Rows another device wrote
    /// leave its shards when it learns of the purge.
    pub fn delete_book(&mut self, id: Uuid) -> anyhow::Result<()> {
        if !self.db.is_purged::<schema::Book>(book_id(id))? && self.knows_book(id)? {
            self.unpublished = true;
            self.db.purge::<schema::Book>(book_id(id))?;
        }
        self.publish()?;
        self.db.reclaim_purged(self.writer)?;
        self.forget_read_caches();
        Ok(())
    }

    fn card_row(
        &self,
        source: &str,
        target: &str,
        slug: &str,
    ) -> anyhow::Result<Option<Id<schema::Card>>> {
        let id = card_row_id(&crate::card::card_id(source, target, slug));
        Ok(self.db.cards.live_row_of(id)?.map(|_| id))
    }

    pub fn card(&self, source: &str, target: &str, slug: &str) -> anyhow::Result<Option<Card>> {
        let Some(id) = self.card_row(source, target, slug)? else {
            return Ok(None);
        };
        self.card_at(id, crate::card::card_id(source, target, slug))
            .map(Some)
    }

    /// Every card of one pair with its slug, sorted by slug.
    pub fn cards_in_pair(&self, source: &str, target: &str) -> anyhow::Result<Vec<(String, Card)>> {
        let mut cards = Vec::new();
        for id in self.db.cards.find_by_pair(&format!("{source}-{target}"))? {
            let slug = self.db.cards.slug(id)?.into_owned();
            let card = self.card_at(id, crate::card::card_id(source, target, &slug))?;
            cards.push((slug, card));
        }
        cards.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(cards)
    }

    /// The card's retention data alone; the outer `None` is "no such card".
    pub fn card_anki_data(
        &self,
        source: &str,
        target: &str,
        slug: &str,
    ) -> anyhow::Result<Option<Option<AnkiData>>> {
        match self.card_row(source, target, slug)? {
            Some(id) => self.anki_data_at(id).map(Some),
            None => Ok(None),
        }
    }

    fn anki_data_at(&self, id: Id<schema::Card>) -> anyhow::Result<Option<AnkiData>> {
        let cards = &self.db.cards;
        Ok(match cards.anki_state(id)? {
            None => None,
            Some(state) => Some(AnkiData {
                state: match state {
                    AnkiStateCode::Active => AnkiState::Active,
                    AnkiStateCode::Suspended => AnkiState::Suspended,
                    AnkiStateCode::Deleted => AnkiState::Deleted,
                },
                interval_days: cards.interval_days(id)?,
                ease_factor: cards.ease_factor(id)?,
                fsrs_difficulty: cards.fsrs_difficulty(id)?,
                fsrs_stability: cards.fsrs_stability(id)?,
            }),
        })
    }

    /// Sets the retention data if it still is `expected`; `false` when it is
    /// not, or there is no such card.
    pub fn set_card_anki_data(
        &mut self,
        source: &str,
        target: &str,
        slug: &str,
        expected: &Option<AnkiData>,
        data: &AnkiData,
    ) -> anyhow::Result<bool> {
        let Some(id) = self.card_row(source, target, slug)? else {
            return Ok(false);
        };
        let stored = self.anki_data_at(id)?;
        if stored != *expected {
            return Ok(false);
        }
        if stored.as_ref() != Some(data) {
            let lemma = self.db.cards.lemma(id)?.into_owned();
            self.unpublished = true;
            self.db.put_card(
                id,
                &format!("{source}-{target}"),
                slug,
                &lemma,
                Some(state_code(data.state)),
                data.interval_days,
                data.ease_factor,
                data.fsrs_difficulty,
                data.fsrs_stability,
            )?;
        }
        Ok(true)
    }

    fn card_at(&self, id: Id<schema::Card>, card_id: String) -> anyhow::Result<Card> {
        let cards = &self.db.cards;
        let mut translations: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for row in self
            .db
            .children_of::<schema::Card, schema::CardTranslation>(id)?
        {
            let list = translations
                .entry(self.db.card_translations.part_of_speech(row)?.into_owned())
                .or_default();
            let translation = self.db.card_translations.translation(row)?.into_owned();
            if !list.contains(&translation) {
                list.push(translation);
            }
        }
        let mut examples = Vec::new();
        for row in self
            .db
            .children_of::<schema::Card, schema::CardExample>(id)?
        {
            let table = &self.db.card_examples;
            let Ok(book) = <[u8; 16]>::try_from(&*table.book_id(row)?) else {
                continue;
            };
            examples.push(Example {
                source: table.source(row)?.into_owned(),
                translation: table.translation(row)?.into_owned(),
                book_id: Uuid::from_bytes(book),
                chapter: table.chapter(row)? as usize,
                paragraph: table.paragraph(row)? as usize,
            });
        }
        let mut card = Card {
            version: 2,
            id: card_id,
            lemma: cards.lemma(id)?.into_owned(),
            translations,
            examples: Vec::new(),
            anki_data: self.anki_data_at(id)?,
        };
        // Merging applies the example cap in its deterministic order.
        card.merge(Card {
            version: 2,
            id: card.id.clone(),
            lemma: String::new(),
            translations: BTreeMap::new(),
            examples,
            anki_data: None,
        });
        Ok(card)
    }

    /// Unions `card` into the stored one; `true` when that staged anything.
    /// `anki` decides the retention data: `None` keeps the stored value, or
    /// takes `card`'s when there is none stored.
    pub fn stage_card(
        &mut self,
        source: &str,
        target: &str,
        slug: &str,
        card: &Card,
        anki: Option<&Option<AnkiData>>,
    ) -> anyhow::Result<bool> {
        let id = card_row_id(&crate::card::card_id(source, target, slug));
        let stored = self.card(source, target, slug)?;
        let data = match (anki, &stored) {
            (Some(data), _) => data.clone(),
            (None, Some(stored)) if stored.anki_data.is_some() => stored.anki_data.clone(),
            (None, _) => card.anki_data.clone(),
        };
        let mut changed = false;
        let lemma = stored
            .as_ref()
            .map_or(card.lemma.as_str(), |held| held.lemma.as_str());
        if stored.as_ref().is_none_or(|held| held.anki_data != data) {
            (self.unpublished, changed) = (true, true);
            self.db.put_card(
                id,
                &format!("{source}-{target}"),
                slug,
                lemma,
                data.as_ref().map(|d| state_code(d.state)),
                data.as_ref().and_then(|d| d.interval_days),
                data.as_ref().and_then(|d| d.ease_factor),
                data.as_ref().and_then(|d| d.fsrs_difficulty),
                data.as_ref().and_then(|d| d.fsrs_stability),
            )?;
        }
        for (part_of_speech, list) in &card.translations {
            let held = stored
                .as_ref()
                .and_then(|stored| stored.translations.get(part_of_speech))
                .map_or(0, Vec::len);
            let mut position = held;
            for translation in list {
                let row = derived::<schema::CardTranslation>(
                    id.uuid(),
                    &format!("translation/{part_of_speech}\n{translation}"),
                );
                if self.db.card_translations.versions_of(row)?.is_empty() {
                    (self.unpublished, changed) = (true, true);
                    self.db.card_translations.insert(
                        row,
                        id,
                        u32_of(position)?,
                        part_of_speech,
                        translation,
                    )?;
                    position += 1;
                }
            }
        }
        for example in &card.examples {
            let row = derived::<schema::CardExample>(
                id.uuid(),
                &format!(
                    "example/{}/{}/{}\n{}\n{}",
                    example.book_id,
                    example.chapter,
                    example.paragraph,
                    example.source,
                    example.translation
                ),
            );
            if self.db.card_examples.versions_of(row)?.is_empty() {
                (self.unpublished, changed) = (true, true);
                self.db.card_examples.insert(
                    row,
                    id,
                    &example.source,
                    &example.translation,
                    example.book_id.as_bytes(),
                    u32_of(example.chapter)?,
                    u32_of(example.paragraph)?,
                )?;
            }
        }
        Ok(changed)
    }

    pub fn card_pairs(&self) -> anyhow::Result<Vec<String>> {
        let mut pairs = Vec::new();
        for card in self.db.all::<schema::Card>() {
            pairs.push(card.pair()?.into_owned());
        }
        pairs.sort();
        pairs.dedup();
        Ok(pairs)
    }

    pub fn card_slugs(&self, source: &str, target: &str) -> anyhow::Result<Vec<String>> {
        let mut slugs = Vec::new();
        for id in self.db.cards.find_by_pair(&format!("{source}-{target}"))? {
            slugs.push(self.db.cards.slug(id)?.into_owned());
        }
        slugs.sort();
        Ok(slugs)
    }
}

#[cfg(test)]
mod tests;
