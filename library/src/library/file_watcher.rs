use log::{error, info, warn};
use notify::{Event, EventKind, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer_opt};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// The `notify` backend the library watch runs on.
///
/// None on iOS: the recommended one there is kqueue, an open descriptor per
/// watched path against a 256-descriptor limit, and a polling one would only
/// repeat the sync poll, which adopts what arrived every few seconds anyway.
#[cfg(target_os = "ios")]
type WatchBackend = notify::NullWatcher;
#[cfg(not(target_os = "ios"))]
type WatchBackend = notify::RecommendedWatcher;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LibraryFileChange {
    /// Shards (or a not-yet-upgraded device's legacy files) changed on disk.
    StoreChanged { paths: Vec<PathBuf> },
}

pub struct LibraryWatcher {
    path: Option<PathBuf>,
    debouncer: Debouncer<WatchBackend, RecommendedCache>,
    change_rx: Option<UnboundedReceiver<LibraryFileChange>>,
}

/// The debounced-event handler: one change per batch, naming every file that
/// moved.
fn change_handler(
    tx: UnboundedSender<LibraryFileChange>,
) -> impl FnMut(DebounceEventResult) + Send + 'static {
    move |result: DebounceEventResult| match result {
        Ok(events) => {
            let mut paths: Vec<PathBuf> = events
                .iter()
                .flat_map(|event| LibraryWatcher::changed_data_files(event))
                .collect();
            paths.sort();
            paths.dedup();
            if !paths.is_empty() {
                info!("Library store change detected in {} files", paths.len());
                let _ = tx.send(LibraryFileChange::StoreChanged { paths });
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
            notify::Config::default(),
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

    /// The paths in `event` that carry library data: shards and peers files
    /// (`*.dbs`), or the legacy `.dat`/`.json` files an old device syncs in.
    /// Temporaries (`.syncthing.*.tmp`, `.dbsync-*.tmp`, `*~*`) are not data
    /// until they are renamed into place.
    fn changed_data_files(event: &Event) -> Vec<PathBuf> {
        if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
            return Vec::new();
        }
        event
            .paths
            .iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        !name.starts_with('.')
                            && !name.contains('~')
                            && (name.ends_with(".dbs")
                                || name.ends_with(".dat")
                                || name.ends_with(".json"))
                    })
                    && !path.components().any(|part| {
                        part.as_os_str()
                            .to_str()
                            .is_some_and(|part| part.starts_with(".flts"))
                    })
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TempDir;
    use std::time::Duration;
    use tokio::time::timeout;

    async fn next_change(
        rx: &mut UnboundedReceiver<LibraryFileChange>,
        wanted: &std::path::Path,
    ) -> bool {
        timeout(Duration::from_secs(10), async {
            while let Some(LibraryFileChange::StoreChanged { paths }) = rx.recv().await {
                if paths.iter().any(|path| path.ends_with(wanted)) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn watcher_reports_a_shard_renamed_into_place_and_no_temporaries() {
        let tmp = TempDir::new("flts_watcher_shard");
        let table = tmp.path.join("db").join("book");
        std::fs::create_dir_all(&table).unwrap();
        let mut watcher = LibraryWatcher::new().expect("watcher init");
        let mut rx = watcher.take_recv().expect("receiver available");
        watcher.set_path(&tmp.path).expect("set_path ok");

        // Let the recursive watch install before writing.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Syncthing delivers a file under a temporary name, then renames it.
        let arriving = table.join(".syncthing.00000000000000aa.v1.dbs.tmp");
        std::fs::write(&arriving, b"shard").unwrap();
        std::fs::rename(&arriving, table.join("00000000000000aa.v1.dbs")).unwrap();

        assert!(
            next_change(
                &mut rx,
                std::path::Path::new("book/00000000000000aa.v1.dbs")
            )
            .await,
            "a delivered shard must be reported"
        );
    }

    #[test]
    fn temporaries_and_local_state_are_not_data() {
        let event = |path: &str| Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![PathBuf::from(path)],
            attrs: Default::default(),
        };
        for ignored in [
            "/lib/db/book/.syncthing.00aa.v1.dbs.tmp",
            "/lib/db/book/.dbsync-0198.tmp",
            "/lib/.flts/devices.json",
            "/lib/.flts-local/cache.v1.r2.dbs",
            "/lib/1b4e28ba-2fa1-11d2-883f-0016d3cca427/book.dat~Ab12Cd34",
        ] {
            assert!(
                LibraryWatcher::changed_data_files(&event(ignored)).is_empty(),
                "{ignored} is not library data"
            );
        }
        for data in [
            "/lib/db/book/00000000000000aa.v1.dbs",
            "/lib/db/dbsync.meta/00000000000000aa.dbs",
            "/lib/1b4e28ba-2fa1-11d2-883f-0016d3cca427/book.dat",
            "/lib/cards/spa-rus/poder.json",
        ] {
            assert_eq!(LibraryWatcher::changed_data_files(&event(data)).len(), 1);
        }
    }
}
