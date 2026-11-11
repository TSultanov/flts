use super::{BookReadingState, BookUserState, LibraryBook};
use crate::book::{
    book::Book,
    chapter_summaries::{ChapterSummaries, ChapterSummary},
    translation::Translation,
    translation_import::ParagraphTranslation,
};
use isolang::Language;
use std::{sync::Arc, time::Duration};
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookStatus {
    Active,
    Retiring,
    Deleting,
    Closed,
}

pub struct BookSnapshot {
    pub book: Arc<Book>,
    pub(super) translations: Vec<Arc<Translation>>,
    pub summaries: Arc<ChapterSummaries>,
    pub user_state: BookUserState,
    /// The store generation the annotations were read at.
    pub store_generation: u64,
    pub status: BookStatus,
}
impl BookSnapshot {
    pub fn translation(&self, language: &Language) -> Option<&Translation> {
        self.translations.iter().map(Arc::as_ref).find(|t| {
            t.source_language == self.book.language && t.target_language == language.to_639_3()
        })
    }
}

type ResultReply<T> = oneshot::Sender<anyhow::Result<T>>;
pub(crate) enum BookCommand {
    ApplyTranslation {
        paragraph: usize,
        target: Language,
        translation: ParagraphTranslation,
        model: String,
        reply: ResultReply<()>,
    },
    ApplySummary {
        chapter: usize,
        expected_prior: Vec<ChapterSummary>,
        summary: ChapterSummary,
        reply: ResultReply<()>,
    },
    Flush {
        reply: ResultReply<()>,
    },
    Reload {
        reply: ResultReply<bool>,
    },
    UpdateReadingState {
        state: BookReadingState,
        reply: ResultReply<()>,
    },
    UpdateFolderPath {
        path: Vec<String>,
        reply: ResultReply<()>,
    },
    #[cfg(test)]
    Modify(Box<dyn FnOnce(&mut LibraryBook) + Send>),
    #[cfg(test)]
    Pause {
        entered: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    },
}

pub struct BookSubscription {
    receiver: watch::Receiver<Arc<BookSnapshot>>,
}
impl BookSubscription {
    pub fn snapshot(&mut self) -> Arc<BookSnapshot> {
        self.receiver.borrow_and_update().clone()
    }
    pub async fn changed(&mut self) -> anyhow::Result<()> {
        self.receiver.changed().await.map_err(Into::into)
    }
}

pub struct BookRef {
    pub(super) commands: mpsc::Sender<BookCommand>,
    snapshot: watch::Receiver<Arc<BookSnapshot>>,
    /// Admission gate; a watch so waiters observe every status change.
    pub(crate) admission: Arc<watch::Sender<BookStatus>>,
    /// Tells this owner from an earlier one of the same book.
    pub(crate) incarnation: u64,
}
pub type BookHandle = Arc<BookRef>;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopMode {
    Running,
    Retire,
    Delete,
    Close,
}

impl BookRef {
    pub fn snapshot(&self) -> Arc<BookSnapshot> {
        self.snapshot.borrow().clone()
    }
    pub fn subscribe(&self) -> BookSubscription {
        BookSubscription {
            receiver: self.snapshot.clone(),
        }
    }
    pub fn is_alive(&self) -> bool {
        !self.commands.is_closed()
    }
    async fn request<T>(
        &self,
        build: impl FnOnce(ResultReply<T>) -> BookCommand,
    ) -> anyhow::Result<T> {
        let queued_at = tokio::time::Instant::now();
        let (reply, response) = oneshot::channel();
        let permit = self
            .commands
            .reserve()
            .await
            .map_err(|_| anyhow::anyhow!("book deleted or session closed"))?;
        {
            // The borrow blocks a status change until the command is queued.
            let gate = self.admission.borrow();
            anyhow::ensure!(*gate == BookStatus::Active, "book is {:?}", *gate);
            permit.send(build(reply));
        }
        log::trace!("book mailbox admission waited {:?}", queued_at.elapsed());
        response
            .await
            .map_err(|_| anyhow::anyhow!("book deleted or session closed"))?
    }
    /// In memory at once; `flush` says whether it reached the disk.
    pub async fn apply_translation(
        &self,
        paragraph: usize,
        target: Language,
        translation: ParagraphTranslation,
        model: String,
    ) -> anyhow::Result<()> {
        self.request(|reply| BookCommand::ApplyTranslation {
            paragraph,
            target,
            translation,
            model,
            reply,
        })
        .await
    }
    /// Refused when a summary before `chapter` changed since `snapshot`:
    /// each one is written from those before it.
    pub async fn apply_summary(
        &self,
        snapshot: &BookSnapshot,
        chapter: usize,
        summary: ChapterSummary,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            chapter < snapshot.summaries.entries.len(),
            "summary chapter out of range"
        );
        let expected_prior = snapshot.summaries.entries[..chapter].to_vec();
        self.request(|reply| BookCommand::ApplySummary {
            chapter,
            expected_prior,
            summary,
            reply,
        })
        .await
    }
    /// Ok once everything applied so far is on disk.
    pub async fn flush(&self) -> anyhow::Result<()> {
        self.request(|reply| BookCommand::Flush { reply }).await
    }
    /// Adopts what other devices wrote; true when the book changed.
    pub async fn reload(&self) -> anyhow::Result<bool> {
        self.request(|reply| BookCommand::Reload { reply }).await
    }
    /// Reads bypass the mailbox so they never queue behind a save. The
    /// snapshot is ahead of the store only by this device's own updates.
    fn user_state(&self) -> anyhow::Result<BookUserState> {
        let gate = self.admission.borrow();
        anyhow::ensure!(*gate == BookStatus::Active, "book is {:?}", *gate);
        Ok(self.snapshot().user_state.clone())
    }
    pub async fn reading_state(&self) -> anyhow::Result<Option<BookReadingState>> {
        Ok(self.user_state()?.reading_state)
    }
    pub async fn update_reading_state(&self, state: BookReadingState) -> anyhow::Result<()> {
        self.request(|reply| BookCommand::UpdateReadingState { state, reply })
            .await
    }
    pub async fn folder_path(&self) -> anyhow::Result<Vec<String>> {
        Ok(self.user_state()?.folder_path)
    }
    pub async fn update_folder_path(&self, path: Vec<String>) -> anyhow::Result<()> {
        self.request(|reply| BookCommand::UpdateFolderPath { path, reply })
            .await
    }
    #[cfg(test)]
    pub async fn modify<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut LibraryBook) -> R + Send + 'static,
    ) -> anyhow::Result<R> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(BookCommand::Modify(Box::new(move |book| {
                let _ = tx.send(f(book));
            })))
            .await
            .map_err(|_| anyhow::anyhow!("book closed"))?;
        rx.await.map_err(Into::into)
    }
}

pub(crate) fn owner(
    book: &mut LibraryBook,
    incarnation: u64,
) -> (
    BookHandle,
    mpsc::Receiver<BookCommand>,
    watch::Sender<Arc<BookSnapshot>>,
    watch::Sender<StopMode>,
    watch::Receiver<StopMode>,
) {
    let (commands, receiver) = mpsc::channel(256);
    let (publish, snapshot) = watch::channel(book.snapshot());
    let (stop, stopped) = watch::channel(StopMode::Running);
    let reference = Arc::new(BookRef {
        commands,
        snapshot,
        admission: Arc::new(watch::channel(BookStatus::Active).0),
        incarnation,
    });
    (reference, receiver, publish, stop, stopped)
}

fn publish(book: &mut LibraryBook, tx: &watch::Sender<Arc<BookSnapshot>>, status: BookStatus) {
    book.status = status;
    tx.send_replace(book.snapshot());
}

/// Commands taken in one go, so a burst of them costs one commit.
const BATCH: usize = 32;

/// How long a reading position waits for company before it is committed
/// alone. Flipping pages would otherwise be a commit, and a file for every
/// other device to fetch, each.
const POSITION_DELAY: Duration = Duration::from_secs(2);

pub(crate) async fn serve_book(
    mut book: LibraryBook,
    mut commands: mpsc::Receiver<BookCommand>,
    tx: watch::Sender<Arc<BookSnapshot>>,
    mut stop: watch::Receiver<StopMode>,
    admission: Arc<watch::Sender<BookStatus>>,
    incarnation: u64,
) -> (Uuid, u64) {
    let mut flushes = Flushes::new();
    let mut position_due = None;
    loop {
        tokio::select! {
            biased;
            changed = stop.changed() => {
                let mode = if changed.is_err() { StopMode::Close } else { *stop.borrow_and_update() };
                if mode == StopMode::Running { continue; }
                if mode != StopMode::Delete {
                    while let Ok(command) = commands.try_recv() { run(&mut book, command, &tx, &mut flushes).await; }
                    // What a failed commit leaves staged, the store lands later.
                    settle(&mut book, &mut flushes, true).await;
                }
                commands.close();
                // Every queued reply is resolved by closing its sender.
                while commands.recv().await.is_some() {}
                publish(&mut book, &tx, BookStatus::Closed);
                admission.send_replace(BookStatus::Closed);
                break;
            }
            command = commands.recv() => {
                let Some(command) = command else { break; };
                run(&mut book, command, &tx, &mut flushes).await;
                for _ in 1..BATCH {
                    match commands.try_recv() { Ok(command) => run(&mut book, command, &tx, &mut flushes).await, Err(_) => break }
                }
                settle(&mut book, &mut flushes, false).await;
                position_due = match (book.has_unsaved_changes(), position_due) {
                    (false, _) => None,
                    (true, None) => Some(tokio::time::Instant::now() + POSITION_DELAY),
                    (true, due) => due,
                };
            }
            _ = async { tokio::time::sleep_until(position_due.expect("guarded")).await }, if position_due.is_some() => {
                position_due = None;
                settle(&mut book, &mut flushes, true).await;
            }
        }
    }
    log::debug!("book {} owner {incarnation} exited", book.book.id);
    (book.book.id, incarnation)
}

/// Flush replies parked until the batch's single commit.
type Flushes = Vec<ResultReply<()>>;

/// Commits what the batch changed and answers its flushes with the outcome.
/// A reading position alone waits, unless `now` or a flush says otherwise.
async fn settle(book: &mut LibraryBook, flushes: &mut Flushes, now: bool) {
    let result = match flushes.is_empty() {
        true if !now && book.only_position_unsaved() => return,
        true => book.persist().await,
        false => book.flush().await,
    };
    if let Err(err) = &result {
        log::warn!("book {} save failed: {err:#}", book.book.id);
    }
    for reply in flushes.drain(..) {
        let _ = reply.send(match &result {
            Ok(()) => Ok(()),
            Err(err) => Err(anyhow::anyhow!("{err:#}")),
        });
    }
}

async fn run(
    book: &mut LibraryBook,
    command: BookCommand,
    tx: &watch::Sender<Arc<BookSnapshot>>,
    flushes: &mut Flushes,
) {
    match command {
        BookCommand::ApplyTranslation {
            paragraph,
            target,
            translation,
            model,
            reply,
        } => {
            let result = (|| {
                anyhow::ensure!(
                    paragraph < book.book.paragraphs_count(),
                    "paragraph {paragraph} is out of range"
                );
                book.get_or_create_translation(&target)
                    .add_paragraph_translation(paragraph, &translation, &model);
                Ok(())
            })();
            publish(book, tx, BookStatus::Active);
            let _ = reply.send(result);
        }
        BookCommand::ApplySummary {
            chapter,
            expected_prior,
            summary,
            reply,
        } => {
            let result = (|| {
                anyhow::ensure!(
                    chapter < book.summaries.entries.len()
                        && book.summaries.entries[..chapter] == expected_prior,
                    "summary context changed"
                );
                book.set_summary(chapter, summary);
                Ok(())
            })();
            publish(book, tx, BookStatus::Active);
            let _ = reply.send(result);
        }
        BookCommand::Flush { reply } => flushes.push(reply),
        BookCommand::Reload { reply } => {
            let result = book.reload().await;
            publish(book, tx, BookStatus::Active);
            let _ = reply.send(result);
        }
        BookCommand::UpdateReadingState { state, reply } => {
            book.set_reading_state(state);
            publish(book, tx, BookStatus::Active);
            let _ = reply.send(Ok(()));
        }
        BookCommand::UpdateFolderPath { path, reply } => {
            let result = book.update_folder_path(path).await;
            publish(book, tx, BookStatus::Active);
            let _ = reply.send(result);
        }
        #[cfg(test)]
        BookCommand::Pause { entered, release } => {
            let _ = entered.send(());
            let _ = release.await;
        }
        #[cfg(test)]
        BookCommand::Modify(f) => {
            f(book);
            publish(book, tx, BookStatus::Active);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Library;

    fn eng() -> Language {
        Language::from_639_3("eng").unwrap()
    }
    fn rus() -> Language {
        Language::from_639_3("rus").unwrap()
    }
    async fn fixture(capacity: usize) -> (tempfile::TempDir, Library, BookHandle) {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open_with_capacity(dir.path().join("library"), capacity)
            .await
            .unwrap();
        let id = library
            .create_book_plain("First", "hello", &eng())
            .await
            .unwrap();
        let book = library.get_book(&id).await.unwrap();
        (dir, library, book)
    }
    /// Makes the next `count` commits fail; zero heals the store.
    fn fail_commits(store: &crate::store::LibraryStore, count: usize) {
        store
            .run_blocking(|db| {
                db.fail_commits = count;
                Ok(())
            })
            .unwrap();
    }
    fn committed_reading_state(
        store: &crate::store::LibraryStore,
        id: Uuid,
    ) -> Option<BookReadingState> {
        let (_dir, peer) = crate::test_utils::peer_view(store);
        peer.run_blocking(|db| db.user_state(id))
            .unwrap()
            .reading_state
    }
    async fn pause(book: &BookHandle) -> oneshot::Sender<()> {
        let (entered, start) = oneshot::channel();
        let (release, blocked) = oneshot::channel();
        book.commands
            .send(BookCommand::Pause {
                entered,
                release: blocked,
            })
            .await
            .unwrap();
        start.await.unwrap();
        release
    }
    fn empty_translation() -> ParagraphTranslation {
        ParagraphTranslation {
            timestamp: 1,
            sentences: vec![],
            total_tokens: None,
        }
    }
    async fn translate(book: &BookHandle) -> anyhow::Result<()> {
        book.apply_translation(0, rus(), empty_translation(), "test".into())
            .await
    }
    fn apply(reply: ResultReply<()>) -> BookCommand {
        BookCommand::ApplyTranslation {
            paragraph: 0,
            target: rus(),
            translation: empty_translation(),
            model: "test".into(),
            reply,
        }
    }
    fn summary(text: &str) -> ChapterSummary {
        ChapterSummary {
            generated: true,
            model: "test".into(),
            timestamp: 1,
            text: text.into(),
        }
    }
    fn position(page_offset: usize) -> BookReadingState {
        BookReadingState {
            chapter_id: 0,
            paragraph_id: 0,
            page_offset,
        }
    }
    async fn commits(book: &BookHandle) -> usize {
        book.modify(|book| book.save_count).await.unwrap()
    }
    /// The book as another device would read it from the shards.
    async fn committed(store: &crate::store::LibraryStore, id: Uuid) -> LibraryBook {
        let (_dir, peer) = crate::test_utils::peer_view(store);
        LibraryBook::load(peer, id).await.unwrap()
    }
    async fn committed_versions(store: &crate::store::LibraryStore, id: Uuid) -> usize {
        committed(store, id)
            .await
            .get_translation(&rus())
            .map_or(0, |t| t.translation.version_count())
    }
    /// Waits for the sweep to retire a book nobody holds.
    async fn retired(library: &Library, id: Uuid) {
        for _ in 0..100 {
            if library.cached_book(id).await.is_none() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("book {id} was never retired");
    }

    #[tokio::test]
    async fn a_reading_position_whose_commit_fails_lands_on_close() {
        let (_dir, library, handle) = fixture(2).await;
        let id = handle.snapshot().book.id;
        let store = library.store().clone();
        fail_commits(&store, usize::MAX);
        let expected = position(3);
        handle.update_reading_state(expected.clone()).await.unwrap();
        assert_eq!(
            handle.reading_state().await.unwrap(),
            Some(expected.clone())
        );
        assert!(handle.flush().await.is_err());
        assert_eq!(
            handle.snapshot().user_state.reading_state,
            Some(expected.clone())
        );
        assert_eq!(committed_reading_state(&store, id), None);

        fail_commits(&store, 0);
        drop(handle);
        library.close().await.unwrap();
        assert_eq!(committed_reading_state(&store, id), Some(expected));
    }
    #[tokio::test(start_paused = true)]
    async fn page_turns_are_coalesced_into_one_commit() {
        let (_dir, library, handle) = fixture(2).await;
        let id = handle.snapshot().book.id;
        let store = library.store().clone();
        let before = commits(&handle).await;
        for page in 1..=3 {
            handle.update_reading_state(position(page)).await.unwrap();
        }
        assert_eq!(
            handle.snapshot().user_state.reading_state,
            Some(position(3))
        );
        assert_eq!(commits(&handle).await, before);
        assert_eq!(committed_reading_state(&store, id), None);

        tokio::time::sleep(POSITION_DELAY + Duration::from_millis(100)).await;
        assert_eq!(commits(&handle).await, before + 1);
        assert_eq!(committed_reading_state(&store, id), Some(position(3)));
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn a_reading_position_set_just_before_close_survives_a_reopen() {
        let (_dir, library, handle) = fixture(2).await;
        let id = handle.snapshot().book.id;
        let root = library.root().to_path_buf();
        handle.update_reading_state(position(4)).await.unwrap();
        drop(handle);
        library.close().await.unwrap();
        drop(library);

        let library = Library::open(root).await.unwrap();
        let handle = library.get_book(&id).await.unwrap();
        assert_eq!(handle.reading_state().await.unwrap(), Some(position(4)));
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn library_flush_lands_a_reading_position_still_waiting() {
        let (_dir, library, handle) = fixture(2).await;
        let id = handle.snapshot().book.id;
        let before = commits(&handle).await;
        handle.update_reading_state(position(5)).await.unwrap();
        assert_eq!(committed_reading_state(library.store(), id), None);

        library.flush().await.unwrap();
        assert_eq!(commits(&handle).await, before + 1);
        assert_eq!(
            committed_reading_state(library.store(), id),
            Some(position(5))
        );
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn summaries_share_snapshots_until_changed() {
        let (_dir, library, handle) = fixture(2).await;
        let before = handle.snapshot();
        handle
            .apply_summary(&before, 0, summary("summary"))
            .await
            .unwrap();
        let after = handle.snapshot();
        assert!(!before.summaries.entries[0].generated);
        assert!(after.summaries.entries[0].generated);
        handle.flush().await.unwrap();
        assert!(
            handle
                .apply_summary(&after, usize::MAX, after.summaries.entries[0].clone())
                .await
                .is_err()
        );
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn summaries_survive_a_reopen_and_the_newest_wins() {
        let (_dir, library, handle) = fixture(2).await;
        let id = handle.snapshot().book.id;
        let root = library.root().to_path_buf();
        handle
            .apply_summary(&handle.snapshot(), 0, summary("first"))
            .await
            .unwrap();
        let mut newer = summary("second");
        newer.timestamp = 2;
        handle
            .apply_summary(&handle.snapshot(), 0, newer.clone())
            .await
            .unwrap();
        handle.flush().await.unwrap();
        assert_eq!(handle.snapshot().summaries.entries[0], newer);
        drop(handle);
        library.close().await.unwrap();
        drop(library);

        let library = Library::open(root).await.unwrap();
        let handle = library.get_book(&id).await.unwrap();
        assert_eq!(handle.snapshot().summaries.entries, vec![newer]);
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn a_summary_stamped_no_newer_than_the_one_it_replaces_still_wins() {
        let (_dir, library, handle) = fixture(2).await;
        let id = handle.snapshot().book.id;
        let root = library.root().to_path_buf();
        let mut first = summary("first");
        first.timestamp = 5;
        handle
            .apply_summary(&handle.snapshot(), 0, first)
            .await
            .unwrap();
        handle.flush().await.unwrap();
        let mut second = summary("second");
        second.timestamp = 5;
        handle
            .apply_summary(&handle.snapshot(), 0, second.clone())
            .await
            .unwrap();
        let mut third = summary("third");
        third.timestamp = 2;
        handle
            .apply_summary(&handle.snapshot(), 0, third.clone())
            .await
            .unwrap();
        third.timestamp = 7;
        assert_eq!(handle.snapshot().summaries.entries[0], third);
        handle.flush().await.unwrap();
        drop(handle);
        library.close().await.unwrap();
        drop(library);

        let library = Library::open(root).await.unwrap();
        let handle = library.get_book(&id).await.unwrap();
        assert_eq!(handle.snapshot().summaries.entries, vec![third]);
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn a_summary_written_from_changed_context_is_refused() {
        let (_dir, library, _first) = fixture(2).await;
        let mut source = Book::create(Uuid::new_v4(), "Chapters", &eng());
        for text in ["one", "two"] {
            let chapter = source.push_chapter(None);
            source.push_paragraph(chapter, text, None);
        }
        let book = library.create_book(source).await.unwrap();
        let stale = book.snapshot();
        book.apply_summary(&stale, 0, summary("first"))
            .await
            .unwrap();

        let error = book
            .apply_summary(&stale, 1, summary("second"))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("summary context changed"),
            "{error:#}"
        );
        assert!(!book.snapshot().summaries.entries[1].generated);
        book.apply_summary(&book.snapshot(), 1, summary("second"))
            .await
            .unwrap();
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn snapshots_and_other_books_progress_while_owner_is_blocked() {
        let (_dir, library, first) = fixture(2).await;
        let release = pause(&first).await;
        let before = first.snapshot();
        let second = library
            .create_book_plain("Second", "other", &eng())
            .await
            .unwrap();
        library
            .get_book(&second)
            .await
            .unwrap()
            .flush()
            .await
            .unwrap();
        assert_eq!(before.book.title, "First");
        let pending = tokio::spawn({
            let first = first.clone();
            async move { first.update_folder_path(vec!["queued".into()]).await }
        });
        tokio::task::yield_now().await;
        assert!(!pending.is_finished());
        release.send(()).unwrap();
        pending.await.unwrap().unwrap();
        assert!(before.user_state.folder_path.is_empty());
        assert_eq!(first.snapshot().user_state.folder_path, vec!["queued"]);
        library.close().await.unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn a_retiring_owner_finishes_the_work_it_accepted() {
        let (_dir, library, first) = fixture(1).await;
        let owner = first.incarnation;
        let id = first.snapshot().book.id;
        library
            .create_book_plain("Second", "other", &eng())
            .await
            .unwrap();
        let release = pause(&first).await;
        let (reply, response) = oneshot::channel();
        first
            .commands
            .send(BookCommand::UpdateFolderPath {
                path: vec!["accepted".into()],
                reply,
            })
            .await
            .unwrap();
        drop(first);
        retired(&library, id).await;
        let get = tokio::spawn({
            let library = library;
            async move {
                let result = library.get_book(&id).await.unwrap();
                (library, result)
            }
        });
        tokio::task::yield_now().await;
        assert!(!get.is_finished());
        release.send(()).unwrap();
        response.await.unwrap().unwrap();
        let (library, replacement) = get.await.unwrap();
        assert!(replacement.incarnation > owner);
        assert_eq!(
            replacement.snapshot().user_state.folder_path,
            vec!["accepted"]
        );
        library.close().await.unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn dropping_the_library_lands_accepted_work_and_releases_the_lease() {
        let (dir, library, first) = fixture(2).await;
        let root = library.root().to_path_buf();
        let id = first.snapshot().book.id;
        first.update_reading_state(position(2)).await.unwrap();
        drop(first);
        drop(library);
        let mut reopened = None;
        for _ in 0..120 {
            if let Ok(library) = Library::open(root.clone()).await {
                reopened = Some(library);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let library = reopened.expect("lease released once the registry closed");
        let book = library.get_book(&id).await.unwrap();
        assert_eq!(book.reading_state().await.unwrap(), Some(position(2)));
        drop(book);
        library.close().await.unwrap();
        drop(dir);
    }
    #[tokio::test]
    async fn a_failed_close_reports_keeps_the_lease_and_can_be_retried() {
        let (_dir, library, first) = fixture(2).await;
        let root = library.root().to_path_buf();
        let id = first.snapshot().book.id;
        fail_commits(library.store(), usize::MAX);
        translate(&first).await.unwrap();

        let error = library.close().await.unwrap_err();
        assert!(error.to_string().contains("flush failed"), "{error:#}");
        assert!(
            Library::open(root.clone()).await.is_err(),
            "the lease stays with the session that still holds unsaved work"
        );
        assert!(first.snapshot().translation(&rus()).is_some());
        assert_eq!(committed_versions(library.store(), id).await, 0);
        assert!(library.close().await.is_err());

        fail_commits(library.store(), 0);
        library.close().await.unwrap();
        drop(first);
        drop(library);
        let library = Library::open(root).await.unwrap();
        let book = library.get_book(&id).await.unwrap();
        assert_eq!(
            book.snapshot().translation(&rus()).unwrap().version_count(),
            1
        );
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn a_library_whose_close_failed_still_serves_its_books() {
        let (_dir, library, first) = fixture(2).await;
        let id = first.snapshot().book.id;
        fail_commits(library.store(), usize::MAX);
        translate(&first).await.unwrap();
        assert!(library.close().await.is_err());
        assert_eq!(first.snapshot().status, BookStatus::Closed);

        let again = library.get_book(&id).await.unwrap();
        assert!(again.incarnation > first.incarnation);
        assert_eq!(
            again
                .snapshot()
                .translation(&rus())
                .unwrap()
                .version_count(),
            1,
            "staged rows read back"
        );
        fail_commits(library.store(), 0);
        again.flush().await.unwrap();
        assert_eq!(committed_versions(library.store(), id).await, 1);
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn flushing_a_clean_book_commits_nothing() {
        let (_dir, library, handle) = fixture(2).await;
        translate(&handle).await.unwrap();
        handle.flush().await.unwrap();
        let before = commits(&handle).await;
        // Any commit now would fail; a clean flush must not run one.
        fail_commits(library.store(), usize::MAX);
        handle.flush().await.unwrap();
        library.flush().await.unwrap();
        fail_commits(library.store(), 0);
        assert_eq!(commits(&handle).await, before);
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn queued_applies_and_flushes_share_one_commit() {
        let (_dir, library, book) = fixture(2).await;
        let id = book.snapshot().book.id;
        let before = commits(&book).await;
        let release = pause(&book).await;
        let mut applied = Vec::new();
        let mut flushed = Vec::new();
        for _ in 0..2 {
            let (reply, done) = oneshot::channel();
            book.commands.send(apply(reply)).await.unwrap();
            applied.push(done);
            let (reply, done) = oneshot::channel();
            book.commands
                .send(BookCommand::Flush { reply })
                .await
                .unwrap();
            flushed.push(done);
        }
        release.send(()).unwrap();
        for done in applied {
            done.await.unwrap().unwrap();
        }
        for done in flushed {
            done.await.unwrap().unwrap();
        }
        assert_eq!(commits(&book).await, before + 1, "one commit for the batch");
        assert_eq!(
            book.snapshot().translation(&rus()).unwrap().version_count(),
            2
        );
        assert_eq!(committed_versions(library.store(), id).await, 2);
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn a_flush_reports_the_failed_commit_and_a_later_flush_lands_it() {
        let (_dir, library, book) = fixture(2).await;
        let id = book.snapshot().book.id;
        let release = pause(&book).await;
        fail_commits(library.store(), usize::MAX);
        let (reply, applied) = oneshot::channel();
        book.commands.send(apply(reply)).await.unwrap();
        let (reply, flushed) = oneshot::channel();
        book.commands
            .send(BookCommand::Flush { reply })
            .await
            .unwrap();
        release.send(()).unwrap();
        applied.await.unwrap().unwrap();
        let error = flushed.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("injected"), "{error:#}");
        assert!(book.flush().await.is_err());
        assert!(book.snapshot().translation(&rus()).is_some());
        assert_eq!(committed_versions(library.store(), id).await, 0);

        fail_commits(library.store(), 0);
        book.flush().await.unwrap();
        assert_eq!(committed_versions(library.store(), id).await, 1);
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn library_flush_lands_what_a_failed_commit_left_staged() {
        let (_dir, library, book) = fixture(2).await;
        let id = book.snapshot().book.id;
        fail_commits(library.store(), usize::MAX);
        translate(&book).await.unwrap();
        book.apply_summary(&book.snapshot(), 0, summary("kept"))
            .await
            .unwrap();
        assert!(book.flush().await.is_err());
        assert!(library.flush().await.is_err());
        assert_eq!(committed_versions(library.store(), id).await, 0);

        fail_commits(library.store(), 0);
        library.flush().await.unwrap();
        let stored = committed(library.store(), id).await;
        assert_eq!(
            stored
                .get_translation(&rus())
                .unwrap()
                .translation
                .version_count(),
            1
        );
        assert_eq!(stored.summaries.entries[0], summary("kept"));
        library.close().await.unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn a_failed_commit_is_republished_in_the_background() {
        let (_dir, library, book) = fixture(2).await;
        let id = book.snapshot().book.id;
        let store = library.store().clone();
        let folder = |store: &crate::store::LibraryStore| {
            let (_dir, peer) = crate::test_utils::peer_view(store);
            peer.run_blocking(|db| db.user_state(id))
                .unwrap()
                .folder_path
        };
        fail_commits(&store, usize::MAX);
        let error = book
            .update_folder_path(vec!["shelf".into()])
            .await
            .unwrap_err();
        assert!(error.to_string().contains("injected"), "{error:#}");
        assert_eq!(book.snapshot().user_state.folder_path, ["shelf"]);
        tokio::time::sleep(crate::library::REPUBLISH_EVERY * 2).await;
        assert!(folder(&store).is_empty());

        fail_commits(&store, 0);
        tokio::time::sleep(crate::library::REPUBLISH_EVERY * 2).await;
        assert_eq!(folder(&store), ["shelf"]);
        assert_eq!(book.snapshot().user_state.folder_path, ["shelf"]);
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn closing_a_clean_book_does_not_save_it() {
        let (_dir, library, book) = fixture(2).await;
        book.flush().await.unwrap();
        // Any commit would fail and fail the close with it.
        fail_commits(library.store(), usize::MAX);
        drop(book);
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn user_state_reads_do_not_wait_for_the_owner() {
        let (_dir, library, book) = fixture(2).await;
        let state = position(2);
        book.update_reading_state(state.clone()).await.unwrap();
        book.update_folder_path(vec!["shelf".into()]).await.unwrap();
        let release = pause(&book).await;
        let read = tokio::time::timeout(Duration::from_secs(5), async {
            (
                book.reading_state().await.unwrap(),
                book.folder_path().await.unwrap(),
            )
        })
        .await
        .expect("reads must not queue behind the blocked owner");
        assert_eq!(read, (Some(state), vec!["shelf".to_string()]));
        release.send(()).unwrap();
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn user_state_survives_a_reopen() {
        let (_dir, library, book) = fixture(2).await;
        let id = book.snapshot().book.id;
        let root = library.root().to_path_buf();
        let state = position(2);
        book.update_reading_state(state.clone()).await.unwrap();
        book.update_folder_path(vec!["shelf".into()]).await.unwrap();
        drop(book);
        library.close().await.unwrap();
        drop(library);

        let library = Library::open(root).await.unwrap();
        let book = library.get_book(&id).await.unwrap();
        assert_eq!(book.reading_state().await.unwrap(), Some(state));
        assert_eq!(book.folder_path().await.unwrap(), ["shelf"]);
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn a_closed_book_rejects_reads_and_writes() {
        let (_dir, library, book) = fixture(2).await;
        library.close().await.unwrap();
        assert_eq!(book.snapshot().status, BookStatus::Closed);
        assert!(book.reading_state().await.is_err());
        assert!(translate(&book).await.is_err());
        assert!(book.flush().await.is_err());
        assert!(book.reload().await.is_err());
    }
    #[tokio::test]
    async fn a_translation_of_a_missing_paragraph_is_rejected() {
        let (_dir, library, book) = fixture(2).await;
        let error = book
            .apply_translation(1, rus(), empty_translation(), "test".into())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("out of range"), "{error:#}");
        assert!(book.snapshot().translation(&rus()).is_none());
        library.close().await.unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn retiring_a_book_whose_commit_fails_neither_pins_it_nor_loses_the_change() {
        let (_dir, library, first) = fixture(1).await;
        let id = first.snapshot().book.id;
        let owner = first.incarnation;
        // Capacity 1: the second book leaves the first alive only by its handle.
        let second = library
            .create_book_plain("Second", "other", &eng())
            .await
            .unwrap();
        let second = library.get_book(&second).await.unwrap();
        let release = pause(&first).await;
        fail_commits(library.store(), usize::MAX);
        let (reply, applied) = oneshot::channel();
        first.commands.send(apply(reply)).await.unwrap();
        drop(first);
        retired(&library, id).await;
        release.send(()).unwrap();
        applied.await.unwrap().unwrap();

        let reloaded = library.get_book(&id).await.unwrap();
        assert!(
            reloaded.incarnation > owner,
            "the failed commit pinned nothing"
        );
        assert!(reloaded.snapshot().translation(&rus()).is_some());
        assert_eq!(committed_versions(library.store(), id).await, 0);

        // Any later commit carries what was left staged.
        fail_commits(library.store(), 0);
        second
            .update_folder_path(vec!["shelf".into()])
            .await
            .unwrap();
        assert_eq!(committed_versions(library.store(), id).await, 1);
        assert_eq!(
            reloaded
                .snapshot()
                .translation(&rus())
                .unwrap()
                .version_count(),
            1
        );
        library.close().await.unwrap();
    }
}
