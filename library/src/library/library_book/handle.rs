use super::{BookReadingState, BookUserState, LibraryBook};
use crate::book::{
    book::Book,
    chapter_summaries::{ChapterSummaries, ChapterSummary},
    translation::Translation,
    translation_import::ParagraphTranslation,
};
use isolang::Language;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BookToken {
    pub session: Uuid,
    pub incarnation: Uuid,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookStatus {
    Active,
    Retiring,
    Deleting,
    Closed,
}
#[derive(Clone, Copy, Debug)]
pub struct MutationReceipt {
    pub revision: u64,
}

pub struct BookSnapshot {
    pub path: PathBuf,
    pub book: Arc<Book>,
    pub(super) translations: Vec<Arc<Translation>>,
    pub summaries: Arc<ChapterSummaries>,
    pub user_state: BookUserState,
    pub revision: u64,
    pub persisted_revision: u64,
    pub status: BookStatus,
    pub token: BookToken,
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
        token: BookToken,
        paragraph: usize,
        expected_text: String,
        source: Language,
        target: Language,
        translation: ParagraphTranslation,
        model: String,
        reply: ResultReply<MutationReceipt>,
    },
    ApplySummary {
        token: BookToken,
        chapter: usize,
        expected_source: Arc<Book>,
        expected_prior: Vec<ChapterSummary>,
        summary: ChapterSummary,
        reply: ResultReply<MutationReceipt>,
    },
    Flush {
        revision: u64,
        reply: ResultReply<()>,
    },
    ReloadBook {
        modified: SystemTime,
        reply: ResultReply<bool>,
    },
    ReloadTranslations {
        modified: SystemTime,
        from: Language,
        to: Language,
        reply: ResultReply<bool>,
    },
    ReloadSummaries {
        reply: ResultReply<bool>,
    },
    ReadingState {
        reply: ResultReply<Option<BookReadingState>>,
    },
    UpdateReadingState {
        state: BookReadingState,
        reply: ResultReply<()>,
    },
    FolderPath {
        reply: ResultReply<Vec<String>>,
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
    pub(crate) admission: Arc<Mutex<BookStatus>>,
    pub token: BookToken,
}
pub type BookHandle = Arc<BookRef>;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopMode {
    Running,
    Retire,
    Delete,
    Close,
}
pub(crate) struct StopFailed {
    pub id: Uuid,
    pub token: BookToken,
    pub error: String,
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
            let gate = self.admission.lock().unwrap();
            anyhow::ensure!(*gate == BookStatus::Active, "book is {:?}", *gate);
            permit.send(build(reply));
        }
        log::trace!(
            "book {:?} mailbox admission waited {:?}",
            self.token,
            queued_at.elapsed()
        );
        response
            .await
            .map_err(|_| anyhow::anyhow!("book deleted or session closed"))?
    }
    pub async fn apply_translation(
        &self,
        token: BookToken,
        paragraph: usize,
        expected_text: String,
        source: Language,
        target: Language,
        translation: ParagraphTranslation,
        model: String,
    ) -> anyhow::Result<MutationReceipt> {
        self.request(|reply| BookCommand::ApplyTranslation {
            token,
            paragraph,
            expected_text,
            source,
            target,
            translation,
            model,
            reply,
        })
        .await
    }
    pub async fn apply_summary(
        &self,
        snapshot: &BookSnapshot,
        chapter: usize,
        summary: ChapterSummary,
    ) -> anyhow::Result<MutationReceipt> {
        anyhow::ensure!(
            chapter < snapshot.summaries.entries.len(),
            "summary chapter out of range"
        );
        let expected_prior = snapshot.summaries.entries[..chapter].to_vec();
        self.request(|reply| BookCommand::ApplySummary {
            token: snapshot.token,
            chapter,
            expected_source: snapshot.book.clone(),
            expected_prior,
            summary,
            reply,
        })
        .await
    }
    pub async fn flush_through(&self, revision: u64) -> anyhow::Result<()> {
        self.request(|reply| BookCommand::Flush { revision, reply })
            .await
    }
    pub async fn save(&self) -> anyhow::Result<()> {
        self.flush_through(0).await
    }
    pub async fn save_if_dirty(&self) -> anyhow::Result<()> {
        self.save().await
    }
    pub async fn reload_book(&self, modified: SystemTime) -> anyhow::Result<bool> {
        self.request(|reply| BookCommand::ReloadBook { modified, reply })
            .await
    }
    pub async fn reload_translations(
        &self,
        modified: SystemTime,
        from: Language,
        to: Language,
    ) -> anyhow::Result<bool> {
        self.request(|reply| BookCommand::ReloadTranslations {
            modified,
            from,
            to,
            reply,
        })
        .await
    }
    pub async fn reload_summaries(&self) -> anyhow::Result<bool> {
        self.request(|reply| BookCommand::ReloadSummaries { reply })
            .await
    }
    pub async fn reading_state(&self) -> anyhow::Result<Option<BookReadingState>> {
        self.request(|reply| BookCommand::ReadingState { reply })
            .await
    }
    pub async fn update_reading_state(&self, state: BookReadingState) -> anyhow::Result<()> {
        self.request(|reply| BookCommand::UpdateReadingState { state, reply })
            .await
    }
    pub async fn folder_path(&self) -> anyhow::Result<Vec<String>> {
        self.request(|reply| BookCommand::FolderPath { reply })
            .await
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
    session: Uuid,
) -> (
    BookHandle,
    mpsc::Receiver<BookCommand>,
    watch::Sender<Arc<BookSnapshot>>,
    watch::Sender<StopMode>,
    watch::Receiver<StopMode>,
) {
    book.token = BookToken {
        session,
        incarnation: Uuid::new_v4(),
    };
    let (commands, receiver) = mpsc::channel(256);
    let (publish, snapshot) = watch::channel(book.snapshot());
    let (stop, stopped) = watch::channel(StopMode::Running);
    let reference = Arc::new(BookRef {
        commands,
        snapshot,
        admission: Arc::new(Mutex::new(BookStatus::Active)),
        token: book.token,
    });
    (reference, receiver, publish, stop, stopped)
}

fn publish(book: &mut LibraryBook, tx: &watch::Sender<Arc<BookSnapshot>>, status: BookStatus) {
    book.status = status;
    tx.send_replace(book.snapshot());
}
async fn persist(
    book: &mut LibraryBook,
    tx: &watch::Sender<Arc<BookSnapshot>>,
) -> anyhow::Result<()> {
    let mut result = book.persist().await;
    for _ in 1..3 {
        if result.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        result = book.persist().await;
    }
    result?;
    book.persisted_revision = book.revision;
    publish(book, tx, BookStatus::Active);
    Ok(())
}
pub(crate) async fn serve_book(
    mut book: LibraryBook,
    mut commands: mpsc::Receiver<BookCommand>,
    tx: watch::Sender<Arc<BookSnapshot>>,
    mut stop: watch::Receiver<StopMode>,
    failures: mpsc::UnboundedSender<StopFailed>,
    admission: Arc<Mutex<BookStatus>>,
) -> (Uuid, BookToken) {
    let mut retry = Duration::from_secs(1);
    let mut next_retry = tokio::time::Instant::now() + retry;
    loop {
        tokio::select! {
            biased;
            changed = stop.changed() => {
                let mode = if changed.is_err() { StopMode::Close } else { *stop.borrow_and_update() };
                if mode == StopMode::Running { continue; }
                if mode != StopMode::Delete {
                    while let Ok(command) = commands.try_recv() { run(&mut book, command, &tx).await; }
                    if let Err(err) = persist(&mut book, &tx).await {
                        let _ = failures.send(StopFailed {id: book.book.id, token: book.token, error: format!("{err:#}")});
                        continue;
                    }
                }
                commands.close();
                // Every queued reply is resolved by closing its sender.
                while commands.recv().await.is_some() {}
                publish(&mut book, &tx, BookStatus::Closed);
                *admission.lock().unwrap() = BookStatus::Closed;
                break;
            }
            command = commands.recv() => {
                let Some(command) = command else { break; };
                run(&mut book, command, &tx).await;
                for _ in 1..32 {
                    if !book.has_unsaved_changes() { break; }
                    match commands.try_recv() { Ok(command) => run(&mut book, command, &tx).await, Err(_) => break }
                }
                if book.has_unsaved_changes() {
                    if let Err(err) = persist(&mut book, &tx).await { log::warn!("book {} revision {} autosave failed: {err:#}", book.book.id, book.revision); }
                    next_retry = tokio::time::Instant::now() + retry;
                }
            }
            _ = tokio::time::sleep_until(next_retry), if book.has_unsaved_changes() => {
                if persist(&mut book, &tx).await.is_err() { retry = (retry * 2).min(Duration::from_secs(30)); } else { retry = Duration::from_secs(1); }
                next_retry = tokio::time::Instant::now() + retry;
            }
        }
    }
    log::debug!(
        "book {:?} exited at persisted revision {}",
        book.token,
        book.persisted_revision
    );
    (book.book.id, book.token)
}

async fn run(book: &mut LibraryBook, command: BookCommand, tx: &watch::Sender<Arc<BookSnapshot>>) {
    match command {
        BookCommand::ApplyTranslation {
            token,
            paragraph,
            expected_text,
            source,
            target,
            translation,
            model,
            reply,
        } => {
            let result = (|| {
                anyhow::ensure!(token == book.token, "stale book owner");
                anyhow::ensure!(
                    book.book.language == source.to_639_3(),
                    "book source language changed"
                );
                anyhow::ensure!(
                    paragraph < book.book.paragraphs_count()
                        && book.book.paragraph_view(paragraph).original_text == expected_text,
                    "paragraph content changed; discarding stale translation"
                );
                book.get_or_create_translation(&target)?
                    .add_paragraph_translation(paragraph, &translation, &model);
                book.revision += 1;
                Ok(MutationReceipt {
                    revision: book.revision,
                })
            })();
            publish(book, tx, BookStatus::Active);
            let _ = reply.send(result);
        }
        BookCommand::ApplySummary {
            token,
            chapter,
            expected_source,
            expected_prior,
            summary,
            reply,
        } => {
            let result = (|| {
                anyhow::ensure!(
                    token == book.token && Arc::ptr_eq(&expected_source, &book.book),
                    "summary source changed"
                );
                anyhow::ensure!(
                    chapter < book.summaries.entries.len()
                        && book.summaries.entries[..chapter] == expected_prior,
                    "summary context changed"
                );
                Arc::make_mut(&mut book.summaries).entries[chapter] = summary;
                book.summaries_dirty = true;
                book.revision += 1;
                Ok(MutationReceipt {
                    revision: book.revision,
                })
            })();
            publish(book, tx, BookStatus::Active);
            let _ = reply.send(result);
        }
        BookCommand::Flush { revision, reply } => {
            let result = if revision > book.revision {
                Err(anyhow::anyhow!("unknown book revision"))
            } else {
                persist(book, tx).await
            };
            let _ = reply.send(result);
        }
        BookCommand::ReloadBook { modified, reply } => {
            let result = book.reload_book(modified).await;
            publish(book, tx, BookStatus::Active);
            let _ = reply.send(result);
        }
        BookCommand::ReloadTranslations {
            modified,
            from,
            to,
            reply,
        } => {
            let result = book.reload_translations(modified, from, to).await;
            publish(book, tx, BookStatus::Active);
            let _ = reply.send(result);
        }
        BookCommand::ReloadSummaries { reply } => {
            let result = book.reload_summaries().await;
            publish(book, tx, BookStatus::Active);
            let _ = reply.send(result);
        }
        BookCommand::ReadingState { reply } => {
            let result = book.reading_state().await;
            publish(book, tx, BookStatus::Active);
            let _ = reply.send(result);
        }
        BookCommand::FolderPath { reply } => {
            let result = book.folder_path().await;
            publish(book, tx, BookStatus::Active);
            let _ = reply.send(result);
        }
        BookCommand::UpdateReadingState { state, reply } => {
            let result = book.update_reading_state(state).await;
            publish(book, tx, BookStatus::Active);
            let _ = reply.send(result);
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
            book.revision += 1;
            publish(book, tx, BookStatus::Active);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Library;

    async fn fixture(capacity: usize) -> (tempfile::TempDir, Library, BookHandle) {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open_with_capacity(dir.path().join("library"), capacity)
            .await
            .unwrap();
        let id = library
            .create_book_plain("First", "hello", &Language::from_639_3("eng").unwrap())
            .await
            .unwrap();
        let book = library.get_book(&id).await.unwrap();
        (dir, library, book)
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
    #[tokio::test]
    async fn failed_reading_state_write_is_retained_and_retried_on_close() {
        let (_dir, library, handle) = fixture(2).await;
        let path = handle.snapshot().path.clone();
        handle
            .modify(|book| book.fail_next_user_write = true)
            .await
            .unwrap();
        let expected = BookReadingState {
            chapter_id: 0,
            paragraph_id: 0,
            page_offset: 3,
        };
        let result = handle.update_reading_state(expected.clone()).await;
        assert!(result.is_err());
        assert_eq!(
            handle.snapshot().user_state.reading_state,
            Some(expected.clone())
        );
        library.close().await.unwrap();
        let state = super::super::load_book_user_state(&path).await.unwrap();
        assert_eq!(state.reading_state, Some(expected));
    }
    #[tokio::test]
    async fn summaries_share_snapshots_until_changed() {
        let (_dir, library, handle) = fixture(2).await;
        let before = handle.snapshot();
        let receipt = handle
            .apply_summary(
                &before,
                0,
                ChapterSummary {
                    generated: true,
                    model: "test".into(),
                    timestamp: 1,
                    text: "summary".into(),
                },
            )
            .await
            .unwrap();
        let after = handle.snapshot();
        assert!(!before.summaries.entries[0].generated);
        assert!(after.summaries.entries[0].generated);
        handle.flush_through(receipt.revision).await.unwrap();
        assert!(
            handle
                .apply_summary(&after, usize::MAX, after.summaries.entries[0].clone())
                .await
                .is_err()
        );
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn snapshots_and_other_books_progress_while_owner_is_blocked() {
        let (_dir, library, first) = fixture(2).await;
        let release = pause(&first).await;
        let before = first.snapshot();
        let second = library
            .create_book_plain("Second", "other", &Language::from_639_3("eng").unwrap())
            .await
            .unwrap();
        library
            .get_book(&second)
            .await
            .unwrap()
            .save()
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
    #[tokio::test]
    async fn eviction_does_not_replace_an_owner_with_accepted_work() {
        let (_dir, library, first) = fixture(1).await;
        let token = first.token;
        let id = first.snapshot().book.id;
        library
            .create_book_plain("Second", "other", &Language::from_639_3("eng").unwrap())
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
        assert_ne!(replacement.token, token);
        assert_eq!(
            replacement.snapshot().user_state.folder_path,
            vec!["accepted"]
        );
        library.close().await.unwrap();
    }
    #[tokio::test]
    async fn failed_retirement_retains_dirty_state_and_owner_identity() {
        let (dir, library, first) = fixture(1).await;
        let id = first.snapshot().book.id;
        let token = first.token;
        let bad = first.snapshot().path.join("translation_zzz_yyy.dat");
        std::fs::write(&bad, b"bad").unwrap();
        let receipt = first
            .apply_translation(
                token,
                0,
                "hello".into(),
                Language::from_639_3("eng").unwrap(),
                Language::from_639_3("rus").unwrap(),
                ParagraphTranslation {
                    timestamp: 1,
                    sentences: vec![],
                    total_tokens: None,
                },
                "test".into(),
            )
            .await
            .unwrap();
        assert!(first.flush_through(receipt.revision).await.is_err());
        library
            .create_book_plain("Second", "other", &Language::from_639_3("eng").unwrap())
            .await
            .unwrap();
        drop(first);
        let recovered = library.get_book(&id).await.unwrap();
        assert_eq!(recovered.token, token);
        assert!(
            recovered
                .snapshot()
                .translation(&Language::from_639_3("rus").unwrap())
                .is_some()
        );
        std::fs::remove_file(bad).unwrap();
        recovered.save().await.unwrap();
        library.close().await.unwrap();
        drop(dir);
    }
}
