use std::{collections::HashMap, sync::Arc, time::Duration};

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

struct TranslationRequest {
    book_id: Uuid,
    paragraph_id: usize,
}

pub struct TranslationQueue {
    translate_tx: flume::Sender<TranslationRequest>,

    _saver: JoinHandle<()>,
    _translator: JoinHandle<()>,
}

impl TranslationQueue {
    pub fn init(
        library: Arc<Mutex<Library>>,
        cache: Arc<Mutex<TranslationsCache>>,
        config: &Config,
    ) -> Option<Self> {
        let api_key = config.gemini_api_key.clone()?;
        let target_language = Language::from_639_3(&config.target_language_id)?;
        let model = TranslationModel::from(config.model);

        let (tx_save, rx_save) = flume::unbounded::<Uuid>();

        let saver = tokio::spawn(run_saver(library.clone(), rx_save));

        let (tx_translate, rx_translate) = flume::unbounded::<TranslationRequest>();

        let translator = {
            tokio::spawn(async move {
                while let Ok(request) = rx_translate.recv_async().await {
                    let library = library.clone();
                    let cache = cache.clone();
                    let api_key = api_key.clone();
                    handle_request(
                        library,
                        cache,
                        model,
                        target_language,
                        api_key,
                        &tx_save,
                        &request,
                    )
                    .await
                    .unwrap_or_else(|err| {
                        warn!(
                            "Failed to translate {}/{}: {}",
                            request.book_id, request.paragraph_id, err
                        )
                    });
                }
            })
        };

        Some(Self {
            translate_tx: tx_translate,
            _saver: saver,
            _translator: translator,
        })
    }

    pub async fn translate(&self, book_id: Uuid, paragraph_id: usize) -> anyhow::Result<()> {
        self.translate_tx
            .send_async(TranslationRequest {
                book_id,
                paragraph_id,
            })
            .await?;
        Ok(())
    }
}

async fn handle_request(
    library: Arc<Mutex<Library>>,
    cache: Arc<Mutex<TranslationsCache>>,
    model: TranslationModel,
    target_language: Language,
    api_key: String,
    save_notify: &flume::Sender<Uuid>,
    request: &TranslationRequest,
) -> anyhow::Result<()> {
    let (translation, paragraph_text, source_language) = {
        let book = library.lock().await.get_book(&request.book_id)?;
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
        "Translating paragraph {}: \"{}...\"",
        request.paragraph_id,
        String::from_iter(paragraph_text.chars().take(40))
    );

    let translator = get_translator(
        cache,
        model,
        api_key.clone(),
        source_language,
        target_language,
    )?;

    let p_translation = translator.get_translation(&paragraph_text).await?;
    info!("Translated paragraph {}", request.paragraph_id);

    translation
        .lock()
        .await
        .add_paragraph_translation(request.paragraph_id, &p_translation)
        .await?;

    save_notify.send_async(request.book_id).await?;

    Ok(())
}

async fn run_saver(library: Arc<Mutex<Library>>, rx: flume::Receiver<Uuid>) {
    let savers = Arc::new(Mutex::new(HashMap::new()));

    while let Ok(book_id) = rx.recv_async().await {
        if !savers.lock().await.contains_key(&book_id) {
            save_book(library.clone(), book_id)
                .await
                .unwrap_or_else(|err| warn!("Failed to autosave book {book_id}: {err}"));
            continue;
        }

        let saver = {
            let library = library.clone();
            let savers = savers.clone();
            async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                save_book(library, book_id)
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
    let book = library.lock().await.get_book(&book_id)?;
    let mut book = book.lock().await;
    book.save().await?;
    Ok(())
}
