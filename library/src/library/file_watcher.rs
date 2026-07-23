use isolang::Language;
use itertools::Itertools;
use log::{error, info, warn};
use notify::{Event, EventKind, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer_opt};
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use uuid::Uuid;

/// The `notify` backend the library watch runs on.
///
/// Everywhere except iOS this is the platform-recommended one (FSEvents,
/// inotify, ReadDirectoryChanges). On iOS "recommended" resolves to kqueue,
/// which needs an **open file descriptor per watched path** — and a recursive
/// watch opens one for every file in the tree. A real library is mostly card
/// JSON (one file per lemma, thousands of them), so the watch alone blows past
/// the 256-descriptor soft limit iOS gives an app; from then on every `open` in
/// the process fails with EMFILE, including WebKit loading system fonts
/// ("FontParser could not open ... TimesNewRoman.ttf: [24: Too many open
/// files]"). Polling costs a periodic stat sweep instead and holds no
/// descriptors.
#[cfg(target_os = "ios")]
type WatchBackend = notify::PollWatcher;
#[cfg(not(target_os = "ios"))]
type WatchBackend = notify::RecommendedWatcher;

/// How often the iOS polling backend re-stats the library tree. Long enough
/// that a few thousand stats stay negligible, short enough that a book arriving
/// over sync shows up while the reader is still looking at the shelf.
#[cfg(target_os = "ios")]
const POLL_INTERVAL: Duration = Duration::from_secs(10);

/// Backend configuration. Only the polling watcher reads any of this; the
/// event-driven backends ignore it.
fn watch_config() -> notify::Config {
    #[cfg(target_os = "ios")]
    {
        // `compare_contents` would hash every file on every sweep — mtime+size
        // is what the event-driven backends effectively report anyway.
        notify::Config::default()
            .with_poll_interval(POLL_INTERVAL)
            .with_compare_contents(false)
    }
    #[cfg(not(target_os = "ios"))]
    {
        notify::Config::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LibraryFileChange {
    BookChanged {
        modified: SystemTime,
        uuid: Uuid,
    },
    TranslationChanged {
        modified: SystemTime,
        from: Language,
        to: Language,
        uuid: Uuid,
    },
    CardChanged {
        modified: SystemTime,
        from: Language,
        to: Language,
        lemma_slug: String,
    },
}

pub struct LibraryWatcher {
    path: Option<PathBuf>,
    debouncer: Debouncer<WatchBackend, RecommendedCache>,
    change_rx: Option<UnboundedReceiver<LibraryFileChange>>,
}

/// Builds the debounced-event handler: classify every event, log it, forward
/// the distinct library changes. Shared with the test that drives the polling
/// backend (iOS's) directly, so both backends run the same classification.
fn change_handler(
    tx: UnboundedSender<LibraryFileChange>,
) -> impl FnMut(DebounceEventResult) + Send + 'static {
    move |result: DebounceEventResult| match result {
        Ok(events) => {
            let deduplicated_changes = events
                .into_iter()
                .filter_map(|ev| LibraryWatcher::classify_event(&ev))
                .unique();

            for change in deduplicated_changes {
                match &change {
                    LibraryFileChange::BookChanged { modified: _, uuid } => {
                        info!("Book {uuid} change detected")
                    }
                    LibraryFileChange::TranslationChanged {
                        modified: _,
                        from,
                        to,
                        uuid,
                    } => info!(
                        "Translation {} {}->{} change detected",
                        uuid,
                        from.to_639_3(),
                        to.to_639_3()
                    ),
                    LibraryFileChange::CardChanged {
                        modified: _,
                        from,
                        to,
                        lemma_slug,
                    } => info!(
                        "Card {} {}->{} change detected",
                        lemma_slug,
                        from.to_639_3(),
                        to.to_639_3()
                    ),
                }
                let _ = tx.send(change);
            }
        }
        Err(errors) => {
            error!("File watcher errors: {:?}", errors);
        }
    }
}

impl LibraryWatcher {
    pub fn new() -> anyhow::Result<Self> {
        let (change_tx, change_rx) = unbounded_channel();

        let debouncer = new_debouncer_opt::<_, WatchBackend, RecommendedCache>(
            Duration::from_millis(500),
            None,
            change_handler(change_tx),
            RecommendedCache::new(),
            watch_config(),
        )?;

        Ok(Self {
            path: None,
            debouncer,
            change_rx: Some(change_rx),
        })
    }

    pub fn set_path(&mut self, library_path: &PathBuf) -> anyhow::Result<()> {
        if let Some(path) = &self.path {
            self.debouncer
                .unwatch(path)
                .unwrap_or_else(|err| warn!("Failed to unwatch path {:?}: {}", path, err));
        }
        self.path = Some(library_path.clone());
        self.debouncer
            .watch(library_path, RecursiveMode::Recursive)?;
        info!("Watcher path set to {library_path:?}");
        Ok(())
    }

    pub fn take_recv(&mut self) -> Option<UnboundedReceiver<LibraryFileChange>> {
        self.change_rx.take()
    }

    fn classify_event(event: &Event) -> Option<LibraryFileChange> {
        if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
            return None;
        }

        for path in &event.paths {
            let filename = path.file_name()?.to_str()?;

            // Skip temp files
            if filename.contains('~') {
                continue;
            }

            let metadata = fs::metadata(path);
            if metadata.is_err() {
                warn!(
                    "Failed to read metadata of {:?}: {}",
                    path,
                    &metadata.err().unwrap()
                );
                continue;
            }
            let metadata = metadata.unwrap();

            // Book file: {uuid}/book{some junk from conflicts}.dat
            // Translation file: {uuid}/translation_{src}_{tgt}{some junk from conflicts}.dat
            if filename.starts_with("book") && filename.ends_with(".dat") {
                let uuid = path.parent()?.file_name()?.to_str()?;
                return Some(LibraryFileChange::BookChanged {
                    modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    uuid: Uuid::from_str(uuid).ok()?,
                });
            }

            if filename.starts_with("translation_") && filename.ends_with(".dat") {
                let uuid = path.parent()?.file_name()?.to_str()?;
                let parts: Vec<&str> = filename
                    .trim_start_matches("translation_")
                    .trim_end_matches(".dat")
                    .split('_')
                    .collect();

                if parts.len() >= 2 {
                    let from: String = parts[0].chars().take(3).collect();
                    let to: String = parts[1].chars().take(3).collect();
                    return Some(LibraryFileChange::TranslationChanged {
                        modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                        from: Language::from_639_3(&from)?,
                        to: Language::from_639_3(&to)?,
                        uuid: Uuid::from_str(uuid).ok()?,
                    });
                }
            }

            // Card file: <lib>/cards/<src>-<tgt>/<lemma>_<pos>.json
            // Syncthing conflict siblings (.sync-conflict-) are merged in
            // by LibraryCardStore::load and must not surface as a separate
            // change event.
            if filename.ends_with(".json") && !filename.contains(".sync-conflict-") {
                let parent = path.parent()?;
                let deck_dir = parent.file_name()?.to_str()?;
                let grand = parent.parent()?.file_name()?.to_str()?;
                if grand == "cards"
                    && let Some((src, tgt)) = deck_dir.split_once('-')
                    && let Some(from) = Language::from_639_3(src)
                    && let Some(to) = Language::from_639_3(tgt)
                    && let Some(stem) = filename.strip_suffix(".json")
                {
                    return Some(LibraryFileChange::CardChanged {
                        modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                        from,
                        to,
                        lemma_slug: stem.to_owned(),
                    });
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TempDir;
    use std::time::Duration;
    use tokio::time::timeout;

    #[tokio::test(flavor = "multi_thread")]
    async fn watcher_emits_card_changed_for_atomic_save() {
        let tmp = TempDir::new("flts_watcher_card");
        let mut watcher = LibraryWatcher::new().expect("watcher init");
        let mut rx = watcher.take_recv().expect("receiver available");
        watcher.set_path(&tmp.path).expect("set_path ok");

        // Let the recursive watch fully install before writing.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let deck = tmp.path.join("cards").join("spa-rus");
        tokio::fs::create_dir_all(&deck).await.unwrap();

        // Mirror LibraryCardStore::save: write to temp~, then rename.
        let canonical = deck.join("hola_noun.json");
        let temp = deck.join("hola_noun.json~TEST1234");
        tokio::fs::write(&temp, br#"{"version":1}"#).await.unwrap();
        tokio::fs::rename(&temp, &canonical).await.unwrap();

        let event = timeout(Duration::from_millis(2000), rx.recv())
            .await
            .expect("watcher fired in time")
            .expect("channel still open");

        match event {
            LibraryFileChange::CardChanged {
                from,
                to,
                lemma_slug,
                ..
            } => {
                assert_eq!(from.to_639_3(), "spa");
                assert_eq!(to.to_639_3(), "rus");
                assert_eq!(lemma_slug, "hola_noun");
            }
            other => panic!("expected CardChanged, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn watcher_skips_sync_conflict_card_sibling() {
        let tmp = TempDir::new("flts_watcher_conflict");
        let mut watcher = LibraryWatcher::new().expect("watcher init");
        let mut rx = watcher.take_recv().expect("receiver available");
        watcher.set_path(&tmp.path).expect("set_path ok");
        tokio::time::sleep(Duration::from_millis(100)).await;

        let deck = tmp.path.join("cards").join("spa-rus");
        tokio::fs::create_dir_all(&deck).await.unwrap();
        let conflict = deck.join("hola_noun.sync-conflict-20260101-abc.json");
        tokio::fs::write(&conflict, br#"{"version":1}"#)
            .await
            .unwrap();

        // 1500 ms > 500 ms debounce + safety. No event should fire.
        let result = timeout(Duration::from_millis(1500), rx.recv()).await;
        assert!(
            result.is_err(),
            "no event should fire for conflict sibling; got {result:?}"
        );
    }

    /// iOS runs the polling backend (see [`WatchBackend`]), which no dev or CI
    /// host selects by default — so drive it explicitly here: a card saved the
    /// way `LibraryCardStore` saves one must still classify when the events
    /// come from a stat sweep rather than the kernel.
    #[tokio::test(flavor = "multi_thread")]
    async fn poll_backend_classifies_card_writes() {
        let tmp = TempDir::new("flts_watcher_poll");
        let (tx, mut rx) = unbounded_channel();

        let mut debouncer = new_debouncer_opt::<_, notify::PollWatcher, RecommendedCache>(
            Duration::from_millis(200),
            None,
            change_handler(tx),
            RecommendedCache::new(),
            notify::Config::default()
                .with_poll_interval(Duration::from_millis(100))
                .with_compare_contents(false),
        )
        .expect("poll debouncer init");
        debouncer
            .watch(&tmp.path, RecursiveMode::Recursive)
            .expect("watch ok");

        let deck = tmp.path.join("cards").join("spa-rus");
        tokio::fs::create_dir_all(&deck).await.unwrap();
        let canonical = deck.join("hola_noun.json");
        let temp = deck.join("hola_noun.json~TEST1234");
        tokio::fs::write(&temp, br#"{"version":1}"#).await.unwrap();
        tokio::fs::rename(&temp, &canonical).await.unwrap();

        // Generous: detection is bounded by the poll interval, not the kernel.
        let event = timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("poll watcher fired in time")
            .expect("channel still open");

        match event {
            LibraryFileChange::CardChanged {
                from,
                to,
                lemma_slug,
                ..
            } => {
                assert_eq!(from.to_639_3(), "spa");
                assert_eq!(to.to_639_3(), "rus");
                assert_eq!(lemma_slug, "hola_noun");
            }
            other => panic!("expected CardChanged, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn watcher_still_classifies_translation_files() {
        let tmp = TempDir::new("flts_watcher_translation");
        let mut watcher = LibraryWatcher::new().expect("watcher init");
        let mut rx = watcher.take_recv().expect("receiver available");
        watcher.set_path(&tmp.path).expect("set_path ok");
        tokio::time::sleep(Duration::from_millis(100)).await;

        let uuid = Uuid::new_v4();
        let book_dir = tmp.path.join(uuid.to_string());
        tokio::fs::create_dir_all(&book_dir).await.unwrap();
        let path = book_dir.join("translation_spa_rus.dat");
        tokio::fs::write(&path, b"x").await.unwrap();

        let event = timeout(Duration::from_millis(2000), rx.recv())
            .await
            .expect("watcher fired in time")
            .expect("channel still open");
        assert!(
            matches!(event, LibraryFileChange::TranslationChanged { .. }),
            "expected TranslationChanged; got {event:?}"
        );
    }
}
