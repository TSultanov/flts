use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use isolang::Language;
use library::{
    cache::TranslationsCache,
    library::Library,
    translator::{TranslationModel, Translator, get_translator},
};
use log::{info, warn};
use tokio::{sync::Mutex, task::JoinHandle};
use uuid::Uuid;

use crate::app::config::Config;
use crate::app::library_view::LibraryView;
use tauri::Emitter;

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
    source_language: Language,
    target_language: Language,
}

pub enum TranslationRequestState {
    DoesNotExist = 0,
    Scheduled = 1,
    Translating = 2,
}

pub struct TranslationQueue {
    next_request_index: AtomicUsize,
    translate_tx: flume::Sender<TranslationRequest>,

    paragraph_request_id_map: Arc<Mutex<HashMap<(Uuid, usize), usize>>>,
    request_state: Arc<Mutex<HashMap<usize, TranslationRequestState>>>,

    _saver: JoinHandle<()>,
    _translator: JoinHandle<()>,
}

impl TranslationQueue {
    pub fn init(
        library: Arc<Mutex<Library>>,
        cache: Arc<Mutex<TranslationsCache>>,
        config: &Config,
        app: tauri::AppHandle,
    ) -> Option<Self> {
        let api_key = config.gemini_api_key.clone()?;
        let target_language = Language::from_639_3(&config.target_language_id)?;

        let (tx_save, rx_save) = flume::unbounded::<SaveNotify>();

        let saver = tokio::spawn(run_saver(library.clone(), app.clone(), rx_save));

        let (tx_translate, rx_translate) = flume::unbounded::<TranslationRequest>();

        let paragraph_request_id_map = Arc::new(Mutex::new(HashMap::new()));
        let request_state = Arc::new(Mutex::new(HashMap::new()));

        let translator = {
            let request_state = request_state.clone();
            let paragraph_request_id_map = paragraph_request_id_map.clone();
            tokio::spawn(async move {
                while let Ok(request) = rx_translate.recv_async().await {
                    let library = library.clone();
                    let cache = cache.clone();
                    let api_key = api_key.clone();

                    request_state
                        .lock()
                        .await
                        .insert(request.request_id, TranslationRequestState::Translating);

                    handle_request(library, cache, target_language, api_key, &tx_save, &request)
                        .await
                        .unwrap_or_else(|err| {
                            warn!(
                                "Failed to translate {}/{}: {}",
                                request.book_id, request.paragraph_id, err
                            );
                            info!(
                                "Emitting \"translation_request_complete\" for request {}",
                                request.request_id
                            );
                            app.emit("translation_request_complete", request.request_id)
                                .unwrap_or_else(|err| {
                                    warn!(
                                        "Failed to notify frontend about failed translation: {}",
                                        err
                                    )
                                });
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
            _translator: translator,
            paragraph_request_id_map,
            request_state,
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
}

async fn handle_request(
    library: Arc<Mutex<Library>>,
    cache: Arc<Mutex<TranslationsCache>>,
    target_language: Language,
    api_key: String,
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

    let translator = get_translator(
        cache,
        request.model,
        api_key.clone(),
        source_language,
        target_language,
    )?;

    let p_translation = translator
        .get_translation(&paragraph_text, request.use_cache)
        .await?;
    info!("Translated paragraph {}", request.paragraph_id);

    translation
        .lock()
        .await
        .add_paragraph_translation(request.paragraph_id, &p_translation, request.model)
        .await?;

    save_notify
        .send_async(SaveNotify {
            request_id: request.request_id,
            book_id: request.book_id,
            source_language,
            target_language,
        })
        .await?;

    Ok(())
}

async fn run_saver(
    library: Arc<Mutex<Library>>,
    app: tauri::AppHandle,
    rx: flume::Receiver<SaveNotify>,
) {
    let savers = Arc::new(Mutex::new(HashMap::new()));

    while let Ok(msg) = rx.recv_async().await {
        let book_id = msg.book_id;
        if !savers.lock().await.contains_key(&book_id) {
            save_and_emit(library.clone(), app.clone(), msg)
                .await
                .unwrap_or_else(|err| warn!("Failed to autosave book {book_id}: {err}"));
            continue;
        }

        let saver = {
            let library = library.clone();
            let app = app.clone();
            let savers = savers.clone();
            let msg = msg;
            async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                save_and_emit(library.clone(), app.clone(), msg)
                    .await
                    .unwrap_or_else(|err| warn!("Failed to autosave book {book_id}: {err}"));
                savers.lock().await.remove(&book_id);
            }
        };

        let task = tokio::spawn(saver);
        savers.lock().await.insert(book_id, task);
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
    info!(
        "Emitting \"translation_request_complete\" for request {}",
        msg.request_id
    );
    app.emit("translation_request_complete", msg.request_id)?;

    info!("Emitting \"book_updated\" for {}", msg.book_id);
    app.emit("book_updated", msg.book_id)?;

    let lv = LibraryView::create(app.clone(), library.clone());
    let books = lv.list_books(Some(&msg.target_language)).await?;
    info!("Emitting \"library_updated\"");
    app.emit("library_updated", books)?;

    let payload = (
        msg.source_language.to_639_3(),
        msg.target_language.to_639_3(),
    );
    info!("Emitting \"dictionary_updated\" for {payload:?}",);
    app.emit("dictionary_updated", payload)?;
    Ok(())
}
