use std::{path::PathBuf, sync::Arc, time::SystemTime};

use isolang::Language;
use tokio::sync::{mpsc, oneshot, watch};

use super::{BookReadingState, LibraryBook};
use crate::book::{book::Book, translation::Translation};

/// Immutable view of a book as of some publish; the arenas inside are
/// copy-on-write, so holding one costs nothing and blocks no writer.
pub struct BookSnapshot {
    pub path: PathBuf,
    pub book: Arc<Book>,
    pub(super) translations: Vec<Arc<Translation>>,
}

impl BookSnapshot {
    pub fn translation(&self, target_language: &Language) -> Option<&Translation> {
        let target_language = target_language.to_639_3();
        self.translations.iter().map(Arc::as_ref).find(|t| {
            t.source_language == self.book.language && t.target_language == target_language
        })
    }
}

type Reply = Box<dyn FnOnce() + Send>;

enum BookCommand {
    Modify(Box<dyn FnOnce(&mut LibraryBook) -> Reply + Send>),
    Save {
        only_if_dirty: bool,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    ReloadBook {
        modified: SystemTime,
        reply: oneshot::Sender<anyhow::Result<bool>>,
    },
    ReloadTranslations {
        modified: SystemTime,
        from: Language,
        to: Language,
        reply: oneshot::Sender<anyhow::Result<bool>>,
    },
    ReadingState {
        reply: oneshot::Sender<anyhow::Result<Option<BookReadingState>>>,
    },
    UpdateReadingState {
        state: BookReadingState,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    FolderPath {
        reply: oneshot::Sender<anyhow::Result<Vec<String>>>,
    },
    UpdateFolderPath {
        folder_path: Vec<String>,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
}

/// Handle to the task that owns a `LibraryBook`. Reads take a snapshot
/// without waiting; writes queue behind whatever the task is doing.
pub struct BookRef {
    commands: mpsc::UnboundedSender<BookCommand>,
    snapshot: watch::Receiver<Arc<BookSnapshot>>,
}

pub type BookHandle = Arc<BookRef>;

impl LibraryBook {
    pub fn spawn(mut self) -> BookHandle {
        let (commands, requests) = mpsc::unbounded_channel();
        let (publish, snapshot) = watch::channel(self.snapshot());
        tokio::spawn(serve_book(self, requests, publish));
        Arc::new(BookRef { commands, snapshot })
    }
}

fn closed() -> anyhow::Error {
    anyhow::anyhow!("book task exited")
}

impl BookRef {
    pub fn snapshot(&self) -> Arc<BookSnapshot> {
        self.snapshot.borrow().clone()
    }

    pub fn is_alive(&self) -> bool {
        !self.commands.is_closed()
    }

    async fn request<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> BookCommand,
    ) -> anyhow::Result<T> {
        let (reply, response) = oneshot::channel();
        self.commands.send(build(reply)).map_err(|_| closed())?;
        response.await.map_err(|_| closed())
    }

    pub async fn modify<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut LibraryBook) -> R + Send + 'static,
    ) -> anyhow::Result<R> {
        self.request(|reply| {
            BookCommand::Modify(Box::new(move |book| {
                let value = f(book);
                Box::new(move || {
                    let _ = reply.send(value);
                })
            }))
        })
        .await
    }

    pub async fn save(&self) -> anyhow::Result<()> {
        self.request(|reply| BookCommand::Save {
            only_if_dirty: false,
            reply,
        })
        .await?
    }

    pub async fn save_if_dirty(&self) -> anyhow::Result<()> {
        self.request(|reply| BookCommand::Save {
            only_if_dirty: true,
            reply,
        })
        .await?
    }

    pub async fn reload_book(&self, modified: SystemTime) -> anyhow::Result<bool> {
        self.request(|reply| BookCommand::ReloadBook { modified, reply })
            .await?
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
        .await?
    }

    pub async fn reading_state(&self) -> anyhow::Result<Option<BookReadingState>> {
        self.request(|reply| BookCommand::ReadingState { reply })
            .await?
    }

    pub async fn update_reading_state(&self, state: BookReadingState) -> anyhow::Result<()> {
        self.request(|reply| BookCommand::UpdateReadingState { state, reply })
            .await?
    }

    pub async fn folder_path(&self) -> anyhow::Result<Vec<String>> {
        self.request(|reply| BookCommand::FolderPath { reply })
            .await?
    }

    pub async fn update_folder_path(&self, folder_path: Vec<String>) -> anyhow::Result<()> {
        self.request(|reply| BookCommand::UpdateFolderPath { folder_path, reply })
            .await?
    }
}

/// Runs commands in arrival order. Everything already queued is drained
/// and run before one publish, so a burst of writes yields one snapshot,
/// and replies fire only after that publish so a caller's next `snapshot()`
/// sees its own write.
async fn serve_book(
    mut book: LibraryBook,
    mut commands: mpsc::UnboundedReceiver<BookCommand>,
    publish: watch::Sender<Arc<BookSnapshot>>,
) {
    while let Some(first) = commands.recv().await {
        let mut batch = vec![first];
        while let Ok(next) = commands.try_recv() {
            batch.push(next);
        }

        let mut replies = Vec::with_capacity(batch.len());
        let mut changed = false;
        for command in batch {
            let (reply, touched) = run(&mut book, command).await;
            replies.push(reply);
            changed |= touched;
        }

        if changed {
            publish.send_replace(book.snapshot());
        }
        for reply in replies {
            reply();
        }
    }
}

fn deferred<T: Send + 'static>(reply: oneshot::Sender<T>, value: T) -> Reply {
    Box::new(move || {
        let _ = reply.send(value);
    })
}

async fn run(book: &mut LibraryBook, command: BookCommand) -> (Reply, bool) {
    match command {
        BookCommand::Modify(f) => (f(book), true),
        BookCommand::Save {
            only_if_dirty,
            reply,
        } => {
            let result = if only_if_dirty && !book.has_unsaved_changes() {
                Ok(())
            } else {
                book.save().await
            };
            (deferred(reply, result), true)
        }
        BookCommand::ReloadBook { modified, reply } => {
            (deferred(reply, book.reload_book(modified).await), true)
        }
        BookCommand::ReloadTranslations {
            modified,
            from,
            to,
            reply,
        } => (
            deferred(reply, book.reload_translations(modified, from, to).await),
            true,
        ),
        BookCommand::ReadingState { reply } => (deferred(reply, book.reading_state().await), false),
        BookCommand::UpdateReadingState { state, reply } => (
            deferred(reply, book.update_reading_state(state).await),
            false,
        ),
        BookCommand::FolderPath { reply } => (deferred(reply, book.folder_path().await), false),
        BookCommand::UpdateFolderPath { folder_path, reply } => (
            deferred(reply, book.update_folder_path(folder_path).await),
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TempDir;
    use uuid::Uuid;

    fn plain_book(temp_dir: &TempDir) -> LibraryBook {
        let id = Uuid::new_v4();
        let language = Language::from_639_3("eng").unwrap();
        LibraryBook::create(
            temp_dir.path.join(id.to_string()),
            Book::create(id, "Handle Test", &language),
        )
    }

    #[tokio::test]
    async fn snapshot_reflects_modify_and_earlier_snapshot_is_unchanged() {
        let temp_dir = TempDir::new("flts_handle");
        let handle = plain_book(&temp_dir).spawn();
        let before = handle.snapshot();

        handle
            .modify(|book| {
                Arc::make_mut(&mut book.book).push_chapter(Some("One"));
            })
            .await
            .unwrap();

        let after = handle.snapshot();
        assert_eq!(before.book.chapter_count(), 0);
        assert_eq!(after.book.chapter_count(), 1);
        assert!(!Arc::ptr_eq(&before, &after));
    }

    #[tokio::test]
    async fn rejected_modify_leaves_snapshot_untouched() {
        let temp_dir = TempDir::new("flts_handle");
        let handle = plain_book(&temp_dir).spawn();
        let before = handle.snapshot();
        let stale = Arc::new(Book::create(
            Uuid::new_v4(),
            "Other",
            &Language::from_639_3("eng").unwrap(),
        ));

        let result: anyhow::Result<()> = handle
            .modify(move |book| {
                if !Arc::ptr_eq(&book.book, &stale) {
                    anyhow::bail!("book changed under the writer");
                }
                Arc::make_mut(&mut book.book).push_chapter(None);
                Ok(())
            })
            .await
            .unwrap();

        assert!(result.is_err());
        assert!(Arc::ptr_eq(&before.book, &handle.snapshot().book));
    }

    #[tokio::test]
    async fn task_exits_when_last_handle_drops() {
        let temp_dir = TempDir::new("flts_handle");
        let baseline = tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks();
        let handle = plain_book(&temp_dir).spawn();
        assert!(handle.is_alive());
        drop(handle);

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
        .expect("book task must exit once its handle drops");
    }
}
