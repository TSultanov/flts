use isolang::Language;
use itertools::Itertools;
use log::{error, info, warn};
use notify::{Event, EventKind, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, FileIdMap, new_debouncer};
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use uuid::Uuid;

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
    DictionaryChanged {
        modified: SystemTime,
        from: Language,
        to: Language,
    },
}

pub struct LibraryWatcher {
    path: Option<PathBuf>,
    debouncer: Debouncer<notify::RecommendedWatcher, FileIdMap>,
    change_rx: mpsc::UnboundedReceiver<LibraryFileChange>,
}

impl LibraryWatcher {
    pub fn new() -> anyhow::Result<Self> {
        let (change_tx, change_rx) = mpsc::unbounded_channel();

        let tx = change_tx.clone();
        let debouncer = new_debouncer(
            Duration::from_millis(500),
            None,
            move |result: DebounceEventResult| match result {
                Ok(events) => {
                    let deduplicated_changes = events
                        .into_iter()
                        .map(|ev| Self::classify_event(&ev))
                        .filter_map(|ev| ev)
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
                            LibraryFileChange::DictionaryChanged {
                                modified: _,
                                from,
                                to,
                            } => info!(
                                "Dictionary {} -> {} change detected",
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
            },
        )?;

        Ok(Self {
            path: None,
            debouncer,
            change_rx,
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

    pub async fn recv(&mut self) -> Option<LibraryFileChange> {
        self.change_rx.recv().await
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
                    modified: metadata.modified().unwrap(),
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
                        modified: metadata.modified().unwrap(),
                        from: Language::from_639_3(&from)?,
                        to: Language::from_639_3(&to)?,
                        uuid: Uuid::from_str(uuid).ok()?,
                    });
                }
            }

            // Dictionary file: dictionary_{src}_{tgt}{some junk from conflicts}.dat
            if filename.starts_with("dictionary_") && filename.ends_with(".dat") {
                let parts: Vec<&str> = filename
                    .trim_start_matches("dictionary_")
                    .trim_end_matches(".dat")
                    .split('_')
                    .collect();

                if parts.len() >= 2 {
                    let from: String = parts[0].chars().take(3).collect();
                    let to: String = parts[1].chars().take(3).collect();
                    return Some(LibraryFileChange::DictionaryChanged {
                        modified: metadata.modified().unwrap(),
                        from: Language::from_639_3(&from)?,
                        to: Language::from_639_3(&to)?,
                    });
                }
            }
        }

        None
    }
}
