use std::{
    error::Error,
    fmt::Display,
    path::{Path, PathBuf},
};

use isolang::Language;
use log::trace;
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use crate::{
    book::book::Book,
    card::{Card, CardWordFields, card_update_from_word},
    epub_importer::EpubBook,
    library::{
        file_watcher::LibraryFileChange, library_book::Writes, library_card::LibraryCardStore,
    },
    store::LibraryStore,
};

pub use library_book::{BookHandle, BookRef};

pub mod file_watcher;
pub mod library_book;
pub mod library_card;
mod session;

#[derive(Debug)]
pub enum LibraryError {
    DuplicateTitle(String),
}

impl Display for LibraryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LibraryError::DuplicateTitle(title) => {
                write!(f, "Failed to create book: duplicate title ({title})")
            }
        }
    }
}

impl Error for LibraryError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardCreationOutcome {
    Created,
    AlreadyExists,
}

pub struct LibraryTranslationMetadata {
    pub id: Uuid,
    pub source_langugage: String,
    pub target_language: String,
    pub translated_paragraphs_count: usize,
}

pub struct LibraryBookMetadata {
    pub id: Uuid,
    pub title: String,
    pub chapters_count: usize,
    pub paragraphs_count: usize,
    pub translations_metadata: Vec<LibraryTranslationMetadata>,
    pub folder_path: Vec<String>,
}

/// What adopting other devices' writes changed here.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PeerChanges {
    /// The shelf may differ: books, their folders or their translation counts.
    pub library: bool,
    /// Books whose summaries, position or folder changed, or whose changed
    /// paragraphs are not known one by one.
    pub books: Vec<Uuid>,
    /// Paragraphs that gained a translation.
    pub paragraphs: Vec<(Uuid, usize)>,
    /// Books deleted on another device.
    pub deleted: Vec<Uuid>,
    pub cards: bool,
}

impl PeerChanges {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Housekeeping waits until startup is well over, then recurs rarely: it
/// rewrites whole files and nothing reads better for it.
const MAINTAIN_AFTER: std::time::Duration = std::time::Duration::from_secs(120);
const MAINTAIN_EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

type Closed = watch::Receiver<bool>;
type Retired = std::sync::Arc<std::sync::Mutex<Vec<u64>>>;

/// True once the library has closed, or was dropped.
async fn closed_within(closed: &mut Closed, wait: std::time::Duration) -> bool {
    if *closed.borrow() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(wait) => false,
        changed = closed.changed() => changed.is_err() || *closed.borrow(),
    }
}

async fn maintain_periodically(store: LibraryStore, retired: Retired, mut closed: Closed) {
    let mut wait = MAINTAIN_AFTER;
    while !closed_within(&mut closed, wait).await {
        wait = MAINTAIN_EVERY;
        let retired = retired.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match store.run(move |db| db.maintain(&retired)).await {
            Ok(done) => log::debug!("library store maintained: {done:?}"),
            Err(err) => log::warn!("library store maintenance skipped: {err:#}"),
        }
    }
}

/// Compaction is looked into this often, and done only once nothing was
/// written for a while: writers wait for it while it runs.
const COMPACT_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
const COMPACT_IDLE: std::time::Duration = std::time::Duration::from_secs(30);

async fn compact_when_idle(store: LibraryStore, mut closed: Closed) {
    while !closed_within(&mut closed, COMPACT_EVERY).await {
        let compacting = store.clone();
        match tokio::task::spawn_blocking(move || compacting.compact_if_idle(COMPACT_IDLE)).await {
            Ok(Err(err)) => log::debug!("library store compaction skipped: {err:#}"),
            Ok(Ok(_)) | Err(_) => {}
        }
    }
}

/// How often a commit that failed is tried again on its own.
const REPUBLISH_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// Commits when a book asks for it, and lands what a failed commit left
/// staged, for as long as the library is open.
async fn commit_when_due(writes: Arc<Writes>, mut closed: Closed) {
    let mut asked = writes.commit_due.subscribe();
    loop {
        let due = *asked.borrow_and_update();
        let wait = match due {
            Some(at) => at.saturating_duration_since(tokio::time::Instant::now()),
            None => REPUBLISH_EVERY,
        };
        tokio::select! {
            closed = closed_within(&mut closed, wait) => if closed { break },
            _ = asked.changed() => continue,
        }
        if due.is_some() {
            // Whoever asked for an earlier commit meanwhile keeps its turn.
            writes.commit_due.send_if_modified(|held| {
                let ours = *held == due;
                if ours {
                    *held = None;
                }
                ours
            });
        } else if !writes
            .store
            .read(|db| Ok(db.has_unpublished()))
            .await
            .unwrap_or(false)
        {
            continue;
        }
        if let Err(err) = writes.land().await {
            log::debug!("library store cannot commit yet: {err:#}");
        }
    }
}

type CloseReply = oneshot::Sender<Result<(), String>>;

/// Holds the lease for as long as the library is open. Closes it when asked
/// to, or when the last `Library` is dropped.
async fn hold_open(
    lease: session::LibraryLease,
    writes: Arc<Writes>,
    mut closes: mpsc::Receiver<CloseReply>,
    closed: watch::Sender<bool>,
) {
    loop {
        let reply = closes.recv().await;
        if let Err(err) = writes.land().await {
            log::warn!("library closing with unsaved changes: {err:#}");
            if let Some(reply) = reply {
                // Stays open, so the caller may try to close again.
                let _ = reply.send(Err(format!("flush failed: {err:#}")));
                continue;
            }
        }
        writes.store.close().await;
        drop(lease);
        closed.send_replace(true);
        if let Some(reply) = reply {
            let _ = reply.send(Ok(()));
        }
        break;
    }
}

use std::sync::Arc;

#[derive(Clone)]
pub struct Library {
    library_root: PathBuf,
    store: LibraryStore,
    card_store: LibraryCardStore,
    writes: Arc<Writes>,
    closes: mpsc::Sender<CloseReply>,
    closed: Closed,
    retired: Retired,
}

impl Library {
    pub async fn open(library_root: PathBuf) -> anyhow::Result<Self> {
        Self::open_inner(library_root, None).await
    }

    /// `open`, telling `progress` how far the import of old-format files is.
    /// On a large library that import is most of the first start.
    pub async fn open_reporting(
        library_root: PathBuf,
        progress: crate::legacy::Progress,
    ) -> anyhow::Result<Self> {
        Self::open_inner(library_root, Some(progress)).await
    }

    async fn open_inner(
        library_root: PathBuf,
        progress: Option<crate::legacy::Progress>,
    ) -> anyhow::Result<Self> {
        if !tokio::fs::try_exists(&library_root).await? {
            tokio::fs::create_dir_all(&library_root).await?;
        }

        let lease = session::LibraryLease::acquire(&library_root)?;
        let library_root = lease.root.clone();
        let store = {
            let root = library_root.clone();
            tokio::task::spawn_blocking(move || LibraryStore::open(&root)).await??
        };
        import_legacy(&library_root, &store, progress).await;
        store.run(|db| Ok(db.forget_imported())).await?;
        let card_store = LibraryCardStore::new(store.clone());
        let writes = Writes::new(store.clone());
        let (closes, close_requests) = mpsc::channel(4);
        let (closing, closed) = watch::channel(false);
        tokio::spawn(hold_open(lease, writes.clone(), close_requests, closing));
        tokio::spawn(commit_when_due(writes.clone(), closed.clone()));
        tokio::spawn(compact_when_idle(store.clone(), closed.clone()));
        let retired = Retired::default();
        tokio::spawn(maintain_periodically(
            store.clone(),
            retired.clone(),
            closed.clone(),
        ));

        Ok(Library {
            library_root,
            store,
            card_store,
            writes,
            closes,
            closed,
            retired,
        })
    }

    pub fn card_store(&self) -> &LibraryCardStore {
        &self.card_store
    }

    /// Create the card for one word in a translated paragraph, or report that
    /// it already exists.
    pub async fn create_card_for_word(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
        sentence_id: usize,
        word_id: usize,
        target_language: Language,
    ) -> anyhow::Result<CardCreationOutcome> {
        let book = self.get_book(&book_id).await?;
        let language = book.info().language.clone();
        let source_language = Language::from_639_3(&language).ok_or_else(|| {
            anyhow::anyhow!("unknown source language code on book {book_id}: {language}")
        })?;
        let (chapter_index, translation) = book
            .read(move |book| {
                Ok((
                    book.outline()?.chapter_for_paragraph(paragraph_id),
                    book.translation(&target_language, paragraph_id)?,
                ))
            })
            .await?;
        let chapter_index = chapter_index.ok_or_else(|| {
            anyhow::anyhow!("paragraph {paragraph_id} is not attached to any chapter")
        })?;
        let paragraph = translation
            .ok_or_else(|| anyhow::anyhow!("paragraph {paragraph_id} is not translated"))?
            .translation;
        // The frontend can race a re-translation that shrinks the paragraph.
        let sentence = paragraph
            .sentences
            .get(sentence_id)
            .ok_or_else(|| anyhow::anyhow!("sentence {sentence_id} is out of range"))?;
        let word = sentence
            .words
            .get(word_id)
            .ok_or_else(|| anyhow::anyhow!("word {word_id} is out of range"))?;
        let update = card_update_from_word(
            CardWordFields {
                is_punctuation: word.is_punctuation,
                original_initial_form: &word.grammar.original_initial_form,
                target_initial_form: &word.grammar.target_initial_form,
                part_of_speech: &word.grammar.part_of_speech,
            },
            &crate::card::render_example_source(sentence.words.iter().map(|w| w.original.as_str())),
            &sentence.full_translation,
            source_language,
            target_language,
            book_id,
            chapter_index,
            paragraph_id,
        )
        .map_err(|reason| anyhow::anyhow!("cannot add word to Anki: {reason}"))?;

        let created = self
            .card_store
            .create_if_absent(
                &Card::new_from_update(&update),
                &update.key.source_language,
                &update.key.target_language,
            )
            .await?;
        Ok(if created {
            CardCreationOutcome::Created
        } else {
            CardCreationOutcome::AlreadyExists
        })
    }

    pub async fn list_books(&self) -> anyhow::Result<Vec<LibraryBookMetadata>> {
        let books = self.store.read(|db| db.list_books()).await?;
        Ok(books
            .into_iter()
            .map(|book| LibraryBookMetadata {
                id: book.id,
                title: book.title,
                chapters_count: book.chapters_count,
                paragraphs_count: book.paragraphs_count,
                translations_metadata: book
                    .translations
                    .into_iter()
                    .map(|t| LibraryTranslationMetadata {
                        id: t.id,
                        source_langugage: t.source_language,
                        target_language: t.target_language,
                        translated_paragraphs_count: t.translated_paragraphs_count,
                    })
                    .collect(),
                folder_path: book.folder_path,
            })
            .collect())
    }

    pub fn store(&self) -> &LibraryStore {
        &self.store
    }

    /// Adopts what other devices' shards (or their leftover legacy files) now
    /// hold, and brings every loaded book that is behind up to date.
    pub async fn refresh_from_peers(&self) -> anyhow::Result<PeerChanges> {
        let imported = import_legacy(&self.library_root, &self.store, None).await;
        let adopted = match self.store.run(|db| db.refresh()).await {
            Ok(adopted) => adopted,
            Err(err) => {
                // Whatever got in before the failure may include cards.
                self.card_store.invalidate_all();
                return Err(err);
            }
        };
        let mut changes = PeerChanges {
            library: imported.books > 0 || adopted.shelf,
            cards: imported.cards || adopted.cards,
            deleted: adopted.purged.clone(),
            ..PeerChanges::default()
        };
        if changes.cards {
            self.card_store.invalidate_all();
        }
        if !adopted.purged.is_empty() {
            // The set-aside old-format copy holds the same text.
            let (local, purged) = (self.store.local_dir().to_owned(), adopted.purged.clone());
            let swept = tokio::task::spawn_blocking(move || {
                purged
                    .into_iter()
                    .try_for_each(|id| crate::legacy::remove_backups_of(&local, id))
            });
            if let Ok(Err(err)) = swept.await {
                log::warn!("could not remove the old copy of a deleted book: {err}");
            }
        }
        for (id, change) in &adopted.books {
            // Without a summary or state change, the paragraphs say it all.
            if change.summaries || change.user_state || change.paragraphs.is_empty() {
                changes.books.push(*id);
            }
            changes
                .paragraphs
                .extend(change.paragraphs.iter().map(|paragraph| (*id, *paragraph)));
        }
        changes.books.sort_unstable();
        changes.paragraphs.sort_unstable();
        if !adopted.books.is_empty() {
            self.writes.note_summaries_changed();
        }
        for id in &changes.deleted {
            self.writes.forget(*id);
        }
        Ok(changes)
    }

    /// Names the store writers of devices that left for good, so that
    /// housekeeping stops waiting for them.
    pub fn set_retired_writers(&self, retired: Vec<u64>) {
        *self.retired.lock().unwrap_or_else(|e| e.into_inner()) = retired;
    }

    /// What `maintain_periodically` does on its own schedule.
    pub async fn maintain(&self) -> anyhow::Result<crate::store::Maintained> {
        let retired = self
            .retired
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        self.store.run(move |db| db.maintain(&retired)).await
    }

    /// See [`crate::store::Inner::note_shared_identity`].
    pub async fn note_shared_identity(&self) -> anyhow::Result<()> {
        self.store.run(|db| db.note_shared_identity()).await
    }

    /// The user keeps this installation's sync identity as it is.
    pub async fn settle_shared_identity(&self) -> anyhow::Result<()> {
        self.store.run(|db| db.settle_shared_identity()).await
    }

    /// The user gives this installation an identity of its own: a new store
    /// writer here, and a new sync identity by the caller, who must have
    /// stopped the sync engine first.
    pub async fn take_new_identity(&self) -> anyhow::Result<()> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.start_over_as_new_writer()).await??;
        self.card_store.invalidate_all();
        self.settle_shared_identity().await
    }

    pub async fn store_health(&self) -> anyhow::Result<crate::store::StoreHealth> {
        self.store.read(|db| Ok(db.health())).await
    }

    pub async fn create_book_plain(
        &self,
        title: &str,
        text: &str,
        language: &Language,
    ) -> anyhow::Result<Uuid> {
        let mut book = Book::create(Uuid::new_v4(), title, language);
        let chapter_index = book.push_chapter(None);
        for paragraph in split_paragraphs(text) {
            book.push_paragraph(chapter_index, paragraph, None);
        }

        Ok(self.create_book(book).await?.id())
    }

    pub async fn create_book_epub(
        &self,
        epub: &EpubBook,
        language: &Language,
    ) -> anyhow::Result<Uuid> {
        let mut book = Book::create(Uuid::new_v4(), &epub.title, language);
        for ch in &epub.chapters {
            let ch_idx = book.push_chapter(Some(&ch.title));
            for p in &ch.paragraphs {
                book.push_paragraph(ch_idx, &p.text, Some(&p.html));
            }
        }

        Ok(self.create_book(book).await?.id())
    }

    /// Ok once nothing of this device's is waiting for a commit.
    pub async fn flush(&self) -> anyhow::Result<()> {
        self.writes.land().await
    }

    /// A close that could not commit fails and leaves the library open.
    pub async fn close(&self) -> anyhow::Result<()> {
        if *self.closed.borrow() {
            return Ok(());
        }
        let (reply, done) = oneshot::channel();
        if self.closes.send(reply).await.is_err() {
            return Ok(());
        }
        match done.await {
            Ok(result) => result.map_err(anyhow::Error::msg),
            Err(_) => Ok(()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.library_root
    }

    pub async fn handle_file_change_event(
        &self,
        event: &LibraryFileChange,
    ) -> anyhow::Result<PeerChanges> {
        trace!("Starting file change event handling: {:?}...", event);
        let LibraryFileChange::StoreChanged { paths } = event;
        // Our own commits echo back through the watcher.
        if paths.iter().all(|path| self.store.is_own_file(path)) {
            return Ok(PeerChanges::default());
        }
        let result = self.refresh_from_peers().await;
        trace!("Finish file change event handling");
        result
    }
}

/// Imports what old-format files are in the root. Never fatal: the store
/// works without them, and they are looked at again on the next refresh.
async fn import_legacy(
    root: &Path,
    store: &LibraryStore,
    progress: Option<crate::legacy::Progress>,
) -> crate::legacy::Imported {
    crate::legacy::migrate_reporting(root, store, progress)
        .await
        .unwrap_or_else(|err| {
            log::warn!("legacy import skipped: {err:#}");
            crate::legacy::Imported::default()
        })
}

fn split_paragraphs(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|p| !p.is_empty())
}

#[cfg(test)]
mod library_tests {
    use super::*;
    use crate::book::translation_import;
    use crate::test_utils::TempDir;

    fn full_word(
        original: &str,
        lemma_src: &str,
        lemma_tgt: &str,
        part_of_speech: &str,
        translations: &[&str],
        is_punctuation: bool,
    ) -> translation_import::Word {
        translation_import::Word {
            original: original.into(),
            contextual_translations: translations.iter().map(|s| (*s).into()).collect(),
            note: None,
            is_punctuation,
            grammar: translation_import::Grammar {
                original_initial_form: lemma_src.into(),
                target_initial_form: lemma_tgt.into(),
                part_of_speech: part_of_speech.into(),
                plurality: None,
                person: None,
                tense: None,
                case: None,
                other: None,
            },
        }
    }

    fn paragraph_with(
        full_translation: &str,
        words: Vec<translation_import::Word>,
    ) -> translation_import::ParagraphTranslation {
        translation_import::ParagraphTranslation {
            timestamp: 0,
            total_tokens: None,
            sentences: vec![translation_import::Sentence {
                full_translation: full_translation.into(),
                words,
            }],
        }
    }

    async fn library_with_one_paragraph_book(
        library_path: PathBuf,
        paragraph_text: &str,
    ) -> (Library, Uuid) {
        let library = Library::open(library_path).await.unwrap();
        let book_id = create_saved_book(&library, "Test Book", "spa", &[paragraph_text]).await;
        (library, book_id)
    }

    async fn create_saved_book(
        library: &Library,
        title: &str,
        language: &str,
        paragraphs: &[&str],
    ) -> Uuid {
        let mut book = Book::create(
            Uuid::new_v4(),
            title,
            &Language::from_639_3(language).unwrap(),
        );
        if !paragraphs.is_empty() {
            let chapter = book.push_chapter(Some("Intro"));
            for paragraph in paragraphs {
                book.push_paragraph(chapter, paragraph, None);
            }
        }
        let id = book.id;
        library.create_book(book).await.unwrap();
        id
    }

    #[tokio::test]
    async fn library_open_newdirectory() {
        let temp_dir = TempDir::new("flts_test");
        let library_path = temp_dir.path.join("test");
        _ = Library::open(library_path.clone()).await.unwrap();

        assert!(library_path.exists());
        assert!(library_path.is_dir());
    }

    #[tokio::test]
    async fn list_books_empty_library() {
        let temp_dir = TempDir::new("flts_test");
        let library_path = temp_dir.path.join("lib");
        let library = Library::open(library_path).await.unwrap();

        let books = library.list_books().await.unwrap();
        assert!(books.is_empty(), "Expected no books, got {:?}", books.len());
    }

    #[tokio::test]
    async fn list_books_multiple_empty_books() {
        let temp_dir = TempDir::new("flts_test");
        let library_path = temp_dir.path.join("lib");
        let library = Library::open(library_path.clone()).await.unwrap();

        create_saved_book(&library, "First Book", "eng", &[]).await;
        create_saved_book(&library, "Second Book", "eng", &[]).await;

        let mut books = library.list_books().await.unwrap();
        assert_eq!(books.len(), 2);
        books.sort_by(|a, b| a.title.cmp(&b.title));
        assert_eq!(books[0].title, "First Book");
        assert_eq!(books[0].paragraphs_count, 0);
        assert!(books[0].translations_metadata.is_empty());
        assert_eq!(books[1].title, "Second Book");
        assert_eq!(books[1].paragraphs_count, 0);
        assert!(books[1].translations_metadata.is_empty());
    }

    #[tokio::test]
    async fn list_books_includes_folder_path() {
        let temp_dir = TempDir::new("flts_test");
        let library_path = temp_dir.path.join("lib");
        let library = Library::open(library_path.clone()).await.unwrap();

        let id = create_saved_book(&library, "Categorized", "eng", &[]).await;
        library
            .get_book(&id)
            .await
            .unwrap()
            .update_folder_path(vec!["Shelf".into(), "Modern".into()])
            .await
            .unwrap();

        let books = library.list_books().await.unwrap();
        assert_eq!(books.len(), 1);
        assert_eq!(
            books[0].folder_path,
            vec!["Shelf".to_string(), "Modern".to_string()]
        );
    }

    #[test]
    fn split_paragraphs_js_equivalence_basic() {
        let input = "Hello\n\n  world  \r\n\nNext line\n";
        let result: Vec<_> = split_paragraphs(input).collect();
        assert_eq!(result, vec!["Hello", "world", "Next line"]);
    }

    #[test]
    fn split_paragraphs_whitespace_only() {
        let input = "  \n\n\t\n\r\n";
        let result: Vec<_> = split_paragraphs(input).collect();
        assert!(result.is_empty());
    }

    async fn make_saved_book(library: &Library, title: &str) -> Uuid {
        create_saved_book(library, title, "eng", &[]).await
    }

    #[tokio::test]
    async fn background_tasks_exit_when_the_library_drops() {
        let temp_dir = TempDir::new("flts_test");
        let baseline = tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks();
        let library = Library::open(temp_dir.path.join("lib")).await.unwrap();
        let id = make_saved_book(&library, "Short-lived").await;
        library.get_book(&id).await.unwrap();

        drop(library);

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while tokio::runtime::Handle::current()
                .metrics()
                .num_alive_tasks()
                > baseline
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("every background task must exit once the library drops");
    }

    #[tokio::test]
    async fn get_book_fails_for_every_caller_asking_for_a_missing_book() {
        let temp_dir = TempDir::new("flts_test");
        let library = Library::open(temp_dir.path.join("lib")).await.unwrap();
        let missing = Uuid::new_v4();

        let gets: Vec<_> = (0..4)
            .map(|_| {
                let library = library.clone();
                tokio::spawn(async move { library.get_book(&missing).await })
            })
            .collect();
        for get in gets {
            assert!(get.await.unwrap().is_err());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_get_book_serves_every_caller() {
        let temp_dir = TempDir::new("flts_test");
        let library = Library::open(temp_dir.path.join("lib")).await.unwrap();
        let id = make_saved_book(&library, "Raced").await;

        let gets: Vec<_> = (0..8)
            .map(|_| {
                let library = library.clone();
                tokio::spawn(async move { library.get_book(&id).await })
            })
            .collect();
        for get in gets {
            let book = get
                .await
                .unwrap()
                .expect("concurrent get_book must not fail");
            assert_eq!(book.id(), id);
            assert_eq!(book.info().title, "Raced");
        }
    }

    fn spanish_paragraph() -> translation_import::ParagraphTranslation {
        paragraph_with(
            "Я могу войти в дом.",
            vec![
                full_word("Puedo", "poder", "мочь", "verb", &["могу"], false),
                full_word("entrar", "entrar", "входить", "verb", &["войти"], false),
                full_word("en", "en", "в", "prep", &["в"], false),
                full_word("casa", "casa", "дом", "noun", &["дом"], false),
                full_word(".", ".", ".", "punct", &[], true),
            ],
        )
    }

    #[tokio::test]
    async fn create_card_for_word_writes_one_card() {
        let tmp = TempDir::new("flts_card_manual");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "Puedo entrar en casa.").await;
        let tgt = Language::from_639_3("rus").unwrap();
        let paragraph = spanish_paragraph();
        seed_translation(&library, book_id, 0, &paragraph, tgt).await;

        // Clicking the inflected "Puedo" creates the citation-form card.
        let outcome = library
            .create_card_for_word(book_id, 0, 0, 0, tgt)
            .await
            .unwrap();
        assert_eq!(outcome, CardCreationOutcome::Created);

        let card = stored_card(&library, "poder").await.unwrap();
        assert_eq!(card.lemma, "poder");
        assert_eq!(card.translations_flat(), vec!["мочь"]);
        let example = &card.examples[0];
        assert_eq!(example.source, "Puedo entrar en casa.");
        assert_eq!(example.translation, "Я могу войти в дом.");
        assert_eq!(example.book_id, book_id);
        assert_eq!(example.chapter, 0);
        assert_eq!(example.paragraph, 0);
        assert!(card.anki_data.is_none(), "new cards wait for the sync pass");

        // A second click is a no-op, not a second example.
        let outcome = library
            .create_card_for_word(book_id, 0, 0, 0, tgt)
            .await
            .unwrap();
        assert_eq!(outcome, CardCreationOutcome::AlreadyExists);
        let card = stored_card(&library, "poder").await.unwrap();
        assert_eq!(card.examples.len(), 1);

        // The other words were not touched by this one click.
        assert_eq!(deck(&library).await, vec!["poder".to_string()]);
    }

    async fn stored_card(library: &Library, slug: &str) -> Option<Card> {
        library.card_store().load("spa", "rus", slug).await.unwrap()
    }

    async fn deck(library: &Library) -> Vec<String> {
        library
            .card_store()
            .list_cards_in_pair("spa", "rus")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn create_card_for_word_rejects_ineligible_word() {
        let tmp = TempDir::new("flts_card_manual_skip");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "Puedo entrar en casa.").await;
        let tgt = Language::from_639_3("rus").unwrap();
        let paragraph = spanish_paragraph();
        seed_translation(&library, book_id, 0, &paragraph, tgt).await;

        let err = library
            .create_card_for_word(book_id, 0, 0, 4, tgt)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("punctuation"),
            "unexpected error: {err}"
        );
        assert!(
            deck(&library).await.is_empty(),
            "ineligible words must not write a card"
        );
    }

    #[tokio::test]
    async fn create_card_for_word_requires_a_translation() {
        let tmp = TempDir::new("flts_card_manual_notrans");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "Puedo entrar.").await;

        let err = library
            .create_card_for_word(book_id, 0, 0, 0, Language::from_639_3("rus").unwrap())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("is not translated"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn create_card_for_word_leaves_an_existing_card_unchanged() {
        use crate::card::Example;
        let tmp = TempDir::new("flts_card_existing");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "No puedo más.").await;

        let other_book = Uuid::new_v4();
        let existing = Card {
            version: 2,
            id: "flts_spa_rus_poder".into(),
            lemma: "poder".into(),
            translations: std::collections::BTreeMap::from([(
                "verb".to_string(),
                vec!["мочь".to_string(), "уметь".to_string()],
            )]),
            examples: vec![Example {
                source: "puedo".into(),
                translation: "могу".into(),
                book_id: other_book,
                chapter: 0,
                paragraph: 0,
            }],
            anki_data: None,
        };
        library
            .card_store()
            .create_if_absent(&existing, "spa", "rus")
            .await
            .unwrap();

        let paragraph = paragraph_with(
            "Я больше не могу.",
            vec![full_word(
                "puedo",
                "poder",
                "мочь",
                "verb",
                &["могу_новое"],
                false,
            )],
        );
        let tgt = Language::from_639_3("rus").unwrap();
        seed_translation(&library, book_id, 0, &paragraph, tgt).await;

        let outcome = library
            .create_card_for_word(book_id, 0, 0, 0, tgt)
            .await
            .unwrap();
        assert_eq!(outcome, CardCreationOutcome::AlreadyExists);

        assert_eq!(deck(&library).await, vec!["poder".to_string()]);
        assert_eq!(
            stored_card(&library, "poder").await,
            Some(existing),
            "the existing card is not given a new example"
        );
    }

    #[tokio::test]
    async fn create_card_for_word_persists_noisy_pos() {
        let tmp = TempDir::new("flts_card_noisy_pos");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "good judge").await;
        let paragraph = paragraph_with(
            "хорошо судить",
            vec![
                full_word(
                    "good",
                    "good",
                    "хорошо",
                    "Существительное / Прилагательное",
                    &["хорошо"],
                    false,
                ),
                full_word(
                    "judge",
                    "judge",
                    "судить",
                    "глагол (герундий/причастие настоящего времени)",
                    &["судить"],
                    false,
                ),
            ],
        );

        let tgt = Language::from_639_3("rus").unwrap();
        seed_translation(&library, book_id, 0, &paragraph, tgt).await;
        library
            .create_card_for_word(book_id, 0, 0, 0, tgt)
            .await
            .unwrap();
        library
            .create_card_for_word(book_id, 0, 0, 1, tgt)
            .await
            .unwrap();

        assert_eq!(
            deck(&library).await,
            vec!["good".to_string(), "judge".to_string()],
            "cards are keyed by lemma alone"
        );

        // The noisy PoS strings survive as translations-map keys.
        let good = stored_card(&library, "good").await.unwrap();
        assert!(
            good.translations
                .keys()
                .any(|k| k.contains("существительное")),
            "expected noisy PoS as a key inside translations, got {:?}",
            good.translations.keys().collect::<Vec<_>>()
        );
        let judge = stored_card(&library, "judge").await.unwrap();
        assert!(
            judge.translations.keys().any(|k| k.contains("глагол")),
            "expected noisy PoS as a key inside translations, got {:?}",
            judge.translations.keys().collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn create_card_for_word_files_the_card_under_its_language_pair() {
        let tmp = TempDir::new("flts_card_pair");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "hola").await;
        let paragraph = paragraph_with(
            "привет",
            vec![full_word(
                "hola",
                "hola",
                "привет",
                "interj",
                &["привет"],
                false,
            )],
        );
        let tgt = Language::from_639_3("rus").unwrap();
        seed_translation(&library, book_id, 0, &paragraph, tgt).await;
        library
            .create_card_for_word(book_id, 0, 0, 0, tgt)
            .await
            .unwrap();
        assert_eq!(
            library.card_store().list_pairs().await.unwrap(),
            [("spa".to_string(), "rus".to_string())]
        );
    }

    async fn seed_translation(
        library: &Library,
        book_id: Uuid,
        paragraph_id: usize,
        paragraph: &translation_import::ParagraphTranslation,
        target_language: Language,
    ) {
        let book = library.get_book(&book_id).await.unwrap();
        let paragraph = paragraph.clone();
        book.apply_translation(
            paragraph_id,
            target_language,
            paragraph,
            "models/gemini-2.5-flash".into(),
        )
        .await
        .unwrap();
        book.flush().await.unwrap();
    }
}
