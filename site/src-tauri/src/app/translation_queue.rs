use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use isolang::Language;
use library::{
    cache::TranslationsCache,
    library::Library,
    translation_stats::TranslationSizeCache,
    translator::{TranslationModel, TranslationProvider, get_translator},
};
use log::{info, warn};
use tokio::{sync::Mutex, task::JoinHandle};
use uuid::Uuid;

use crate::app::config::Config;
use crate::app::library_view::{LibraryView, ParagraphView};
use tauri::Emitter;

const TRANSLATION_PROGRESS_UPDATE_INTERVAL: Duration = Duration::from_millis(500);
const TRANSLATION_STATUS_TTL: Duration = Duration::from_secs(30);

struct TranslationRequest {
    request_id: usize,
    book_id: Uuid,
    paragraph_id: usize,
    model: TranslationModel,
    use_cache: bool,
}

#[derive(Clone, Copy)]
struct SaveNotify {
    request_id: usize,
    book_id: Uuid,
    paragraph_id: usize,
    target_language: Language,
}

pub enum TranslationRequestState {
    DoesNotExist = 0,
    Scheduled = 1,
    Translating = 2,
}

#[derive(Clone, serde::Serialize)]
pub struct TranslationStatus {
    pub request_id: usize,
    pub progress_chars: usize,
    pub expected_chars: usize,
    pub is_complete: bool,
}

pub struct TranslationQueue {
    next_request_index: AtomicUsize,
    translate_tx: flume::Sender<TranslationRequest>,

    paragraph_request_id_map: Arc<Mutex<HashMap<(Uuid, usize), usize>>>,
    request_state: Arc<Mutex<HashMap<usize, TranslationRequestState>>>,
    translation_status: Arc<Mutex<HashMap<usize, TranslationStatus>>>,

    _saver: JoinHandle<()>,
    _status_updater: JoinHandle<()>,
    _translator: JoinHandle<()>,
}

#[derive(Clone, serde::Serialize)]
struct ParagraphUpdatedPayload {
    book_id: Uuid,
    paragraph: ParagraphView,
}

impl TranslationQueue {
    pub fn init(
        library: Arc<Mutex<Library>>,
        cache: Arc<Mutex<TranslationsCache>>,
        stats_cache: Arc<Mutex<TranslationSizeCache>>,
        config: &Config,
        app: tauri::AppHandle,
    ) -> Option<Self> {
        let gemini_api_key = config.gemini_api_key.clone();
        let openai_api_key = config.openai_api_key.clone();
        let target_language = Language::from_639_3(&config.target_language_id)?;

        let (tx_save, rx_save) = flume::unbounded::<SaveNotify>();

        let translation_status: Arc<Mutex<HashMap<usize, TranslationStatus>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx_status, rx_status) = flume::unbounded::<TranslationStatus>();
        let status_updater = tokio::spawn(run_status_updater(translation_status.clone(), rx_status));

        let saver = tokio::spawn(run_saver(
            library.clone(),
            app.clone(),
            tx_status.clone(),
            rx_save,
        ));

        let (tx_translate, rx_translate) = flume::unbounded::<TranslationRequest>();

        let paragraph_request_id_map = Arc::new(Mutex::new(HashMap::new()));
        let request_state = Arc::new(Mutex::new(HashMap::new()));

        let translator = {
            let request_state = request_state.clone();
            let paragraph_request_id_map = paragraph_request_id_map.clone();
            let tx_status = tx_status.clone();
            tokio::spawn(async move {
                while let Ok(request) = rx_translate.recv_async().await {
                    let library = library.clone();
                    let cache = cache.clone();
                    let gemini_api_key = gemini_api_key.clone();
                    let openai_api_key = openai_api_key.clone();

                    request_state
                        .lock()
                        .await
                        .insert(request.request_id, TranslationRequestState::Translating);

                    handle_request(
                        library,
                        cache,
                        stats_cache.clone(),
                        target_language,
                        gemini_api_key,
                        openai_api_key,
                        tx_status.clone(),
                        &tx_save,
                        &request,
                    )
                    .await
                    .unwrap_or_else(|err| {
                        warn!(
                            "Failed to translate {}/{}: {}",
                            request.book_id, request.paragraph_id, err
                        );
                        let status = TranslationStatus {
                            request_id: request.request_id,
                            progress_chars: 0,
                            expected_chars: 0,
                            is_complete: true,
                        };
                        let _ = tx_status.send(status);
                    });

                    request_state.lock().await.remove(&request.request_id);
                    paragraph_request_id_map
                        .lock()
                        .await
                        .remove(&(request.book_id, request.paragraph_id));
                }
            })
        };

        Some(Self {
            next_request_index: 0.into(),
            translate_tx: tx_translate,
            _saver: saver,
            _status_updater: status_updater,
            _translator: translator,
            paragraph_request_id_map,
            request_state,
            translation_status,
        })
    }

    pub async fn translate(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
        model: TranslationModel,
        use_cache: bool,
    ) -> anyhow::Result<usize> {
        if let Some(id) = self.get_request_id(book_id, paragraph_id).await {
            return Ok(id);
        }

        let request_id = self.next_request_index.fetch_add(1, Ordering::SeqCst);

        self.translate_tx
            .send_async(TranslationRequest {
                request_id,
                book_id,
                paragraph_id,
                model,
                use_cache,
            })
            .await?;

        self.paragraph_request_id_map
            .lock()
            .await
            .insert((book_id, paragraph_id), request_id);
        self.request_state
            .lock()
            .await
            .insert(request_id, TranslationRequestState::Scheduled);

        Ok(request_id)
    }

    pub async fn get_request_id(&self, book_id: Uuid, paragraph_id: usize) -> Option<usize> {
        self.paragraph_request_id_map
            .lock()
            .await
            .get(&(book_id, paragraph_id))
            .map(|i| *i)
    }

    pub async fn get_translation_status(&self, request_id: usize) -> Option<TranslationStatus> {
        self.translation_status
            .lock()
            .await
            .get(&request_id)
            .cloned()
    }

    pub async fn update_translation_status(&self, status: TranslationStatus) {
        self.translation_status
            .lock()
            .await
            .insert(status.request_id, status);
    }
}

async fn handle_request(
    library: Arc<Mutex<Library>>,
    cache: Arc<Mutex<TranslationsCache>>,
    stats_cache: Arc<Mutex<TranslationSizeCache>>,
    target_language: Language,
    gemini_api_key: Option<String>,
    openai_api_key: Option<String>,
    status_tx: flume::Sender<TranslationStatus>,
    save_notify: &flume::Sender<SaveNotify>,
    request: &TranslationRequest,
) -> anyhow::Result<()> {
    let (translation, paragraph_text, source_language) = {
        let book = library.lock().await.get_book(&request.book_id).await?;
        let mut book = book.lock().await;
        let translation = book.get_or_create_translation(&target_language).await;
        let paragraph = book.book.paragraph_view(request.paragraph_id);
        (
            translation,
            paragraph.original_text.to_string(),
            Language::from_639_3(&book.book.language).unwrap(),
        )
    };

    info!(
        "Translating paragraph {} with model {:?}: \"{}...\"",
        request.paragraph_id,
        request.model,
        String::from_iter(paragraph_text.chars().take(40))
    );

    let provider = request
        .model
        .provider()
        .ok_or(anyhow::anyhow!("Unknown model provider"))?;

    let api_key = match provider {
        TranslationProvider::Google => {
            gemini_api_key.ok_or(anyhow::anyhow!("No Gemini API key"))?
        }
        TranslationProvider::Openai => {
            openai_api_key.ok_or(anyhow::anyhow!("No OpenAI API key"))?
        }
    };

    let translator = get_translator(
        cache,
        provider,
        request.model,
        api_key,
        source_language,
        target_language,
    )?;

    let source_len = paragraph_text.len();
    let stats = stats_cache
        .lock()
        .await
        .get(&source_language, &target_language)
        .await;
    let expected_size = stats.estimate(source_len);
    info!(
        "Estimated translation size: {} (source len: {}, ratio: {:.1}, observations: {})",
        expected_size, source_len, stats.ratio, stats.n
    );

    let callback = {
        let status_tx = status_tx.clone();
        let request_id = request.request_id;
        struct EmitState {
            last_emit: Instant,
            last_progress: usize,
        }
        let emit_state = Arc::new(std::sync::Mutex::new(EmitState {
            last_emit: Instant::now(),
            last_progress: 0,
        }));
        Box::new(move |progress_len: usize| {
            let mut state = emit_state.lock().unwrap();
            if state.last_progress == progress_len {
                return;
            }
            if state.last_emit.elapsed() < TRANSLATION_PROGRESS_UPDATE_INTERVAL {
                return;
            }

            state.last_emit = Instant::now();
            state.last_progress = progress_len;
            drop(state);

            let status = TranslationStatus {
                request_id,
                progress_chars: progress_len,
                expected_chars: expected_size,
                is_complete: false,
            };
            let _ = status_tx.send(status);
        })
    };

    let p_translation = translator
        .get_translation(&paragraph_text, request.use_cache, Some(callback))
        .await?;
    info!("Translated paragraph {}", request.paragraph_id);

    // Measure actual translation JSON size and update stats
    let actual_size = serde_json::to_string(&p_translation)
        .map(|s| s.len())
        .unwrap_or(0);
    stats_cache
        .lock()
        .await
        .record_observation(&source_language, &target_language, source_len, actual_size)
        .await;
    info!(
        "Recorded translation stats: source_len={}, actual_size={}, ratio={:.1}",
        source_len,
        actual_size,
        actual_size as f64 / source_len as f64
    );

    translation
        .lock()
        .await
        .add_paragraph_translation(request.paragraph_id, &p_translation, request.model)
        .await?;

    save_notify
        .send_async(SaveNotify {
            request_id: request.request_id,
            book_id: request.book_id,
            paragraph_id: request.paragraph_id,
            target_language,
        })
        .await?;

    Ok(())
}

async fn run_saver(
    library: Arc<Mutex<Library>>,
    app: tauri::AppHandle,
    status_tx: flume::Sender<TranslationStatus>,
    rx: flume::Receiver<SaveNotify>,
) {
    let savers = Arc::new(Mutex::new(HashMap::new()));

    while let Ok(msg) = rx.recv_async().await {
        let book_id = msg.book_id;
        if !savers.lock().await.contains_key(&book_id) {
            save_and_emit(library.clone(), app.clone(), msg)
                .await
                .unwrap_or_else(|err| warn!("Failed to autosave book {book_id}: {err}"));
            let _ = status_tx.send(TranslationStatus {
                request_id: msg.request_id,
                progress_chars: 0,
                expected_chars: 0,
                is_complete: true,
            });
            continue;
        }

        let saver = {
            let library = library.clone();
            let app = app.clone();
            let savers = savers.clone();
            let status_tx = status_tx.clone();
            let msg = msg;
            async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                save_and_emit(library.clone(), app.clone(), msg)
                    .await
                    .unwrap_or_else(|err| warn!("Failed to autosave book {book_id}: {err}"));
                savers.lock().await.remove(&book_id);
                let _ = status_tx.send(TranslationStatus {
                    request_id: msg.request_id,
                    progress_chars: 0,
                    expected_chars: 0,
                    is_complete: true,
                });
            }
        };

        let task = tokio::spawn(saver);
        savers.lock().await.insert(book_id, task);
    }
}

async fn run_status_updater(
    translation_status: Arc<Mutex<HashMap<usize, TranslationStatus>>>,
    rx: flume::Receiver<TranslationStatus>,
) {
    while let Ok(status) = rx.recv_async().await {
        let request_id = status.request_id;
        let is_complete = status.is_complete;
        translation_status.lock().await.insert(request_id, status);

        if is_complete {
            let translation_status = translation_status.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(TRANSLATION_STATUS_TTL).await;
                translation_status.lock().await.remove(&request_id);
            });
        }
    }
}

async fn save_book(library: Arc<Mutex<Library>>, book_id: Uuid) -> anyhow::Result<()> {
    let book_handle = {
        let mut library = library.lock().await;
        library.get_book(&book_id).await?
    };
    let mut book = book_handle.lock().await;
    book.save().await
}

async fn save_and_emit(
    library: Arc<Mutex<Library>>,
    app: tauri::AppHandle,
    msg: SaveNotify,
) -> anyhow::Result<()> {
    save_book(library.clone(), msg.book_id).await?;
    emit_updates(library, app, msg).await?;
    Ok(())
}

async fn emit_updates(
    library: Arc<Mutex<Library>>,
    app: tauri::AppHandle,
    msg: SaveNotify,
) -> anyhow::Result<()> {
    let lv = LibraryView::create(app.clone(), library.clone());
    let paragraph = lv
        .get_paragraph_view(msg.book_id, msg.paragraph_id, &msg.target_language)
        .await?;
    info!(
        "Emitting \"paragraph_updated\" for {}/{}",
        msg.book_id, msg.paragraph_id
    );
    app.emit(
        "paragraph_updated",
        ParagraphUpdatedPayload {
            book_id: msg.book_id,
            paragraph,
        },
    )?;

    let books = lv.list_books(Some(&msg.target_language)).await?;
    info!("Emitting \"library_updated\"");
    app.emit("library_updated", books)?;

    Ok(())
}
