use std::{
    collections::VecDeque,
    error::Error,
    fmt::Display,
    fs::{File, create_dir},
    io::Read,
    path::PathBuf,
    process::ExitCode,
    str::FromStr,
    sync::Arc,
    time::Instant,
};

use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use display_error_chain::DisplayErrorChain;
use file_format::FileFormat;
use isolang::Language;
use library::{
    cache::TranslationsCache,
    epub_importer::EpubBook,
    library::{Library, file_watcher::LibraryWatcher},
    translator::{TranslationModel, Translator, get_translator},
};
use tokio::time::{Duration, sleep};
use tokio::{sync::Mutex, task::JoinSet};
use uuid::Uuid;
use vfs::PhysicalFS;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[arg(short, long, value_name = "FILE")]
    library_path: PathBuf,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Add book to library
    ImportBook {
        /// Book title
        #[arg(short, long, value_name = "TITLE")]
        title: String,
        /// Book language
        #[arg(short, long, value_name = "LANG")]
        language: String,
        /// Path to book file
        path: PathBuf,
    },
    /// Add book to library from EPUB
    ImportEpub {
        /// Book language
        #[arg(short, long, value_name = "LANG")]
        language: String,
        /// Path to EPUB file
        path: PathBuf,
    },
    /// List books
    List {},
    /// Translate book
    Translate {
        /// Book ID
        id: Uuid,
        /// Gemini API key
        #[arg(short, long, value_name = "KEY")]
        api_key: String,
        /// Translation language
        #[arg(short, long, value_name = "LANG")]
        translation_language: String,
        /// Number of parallel LLM requests
        #[arg(short, long, value_name = "NUM")]
        n_parallel: Option<usize>,
    },
}

#[derive(Debug)]
enum CliError {
    UnsupportedFormat(String),
}

impl Error for CliError {}

impl Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::UnsupportedFormat(format) => write!(f, "Unsupported file format '{format}'"),
        }
    }
}

async fn add_book(
    library: &Arc<Mutex<Library>>,
    title: &str,
    path: &PathBuf,
    lang: &str,
) -> anyhow::Result<()> {
    let fmt = FileFormat::from_file(path)?;

    if fmt.media_type() == "text/plain" {
        let mut data = File::open(path)?;
        let mut text = String::new();
        data.read_to_string(&mut text)?;

        let book_id = library.lock().await.create_book_plain(title, &text, &Language::from_str(lang)?).await?;
        let book = library.lock().await.get_book(&book_id)?;
        let book = book.lock().await;
        println!(
            "Created book {} (id: {})",
            book.book.title,
            book.book.id
        );
    } else {
        Err(CliError::UnsupportedFormat(fmt.media_type().to_owned()))?
    }

    Ok(())
}

async fn add_epub(library: &Arc<Mutex<Library>>, path: &PathBuf, lang: &str) -> anyhow::Result<()> {
    let epub = EpubBook::load(path)?;

    let book_id = library.lock().await.create_book_epub(&epub, &Language::from_str(lang)?).await?;
    let book = library.lock().await.get_book(&book_id)?;
    let book = book.lock().await;
    println!(
        "Created book {} (id: {})",
        book.book.title,
        book.book.id
    );

    Ok(())
}

async fn list_books(library: &Arc<Mutex<Library>>) -> anyhow::Result<()> {
    let books = library.lock().await.list_books()?;
    println!("id                                \ttitle\tchapters\tparagraphs");
    for book in books {
        println!(
            "{}\t{}\t{}\t{}",
            book.id, book.title, book.chapters_count, book.paragraphs_count
        );
        if !book.translations_metadata.is_empty() {
            println!("\tTranslations:");
            println!("\tid                                \tsrc\ttgt\tparagraphs");
            for t in book.translations_metadata {
                println!(
                    "\t{}\t{}\t{}\t{}",
                    t.id, t.source_langugage, t.target_language, t.translated_paragraphs_count
                );
            }
        }
    }

    Ok(())
}

async fn translate_paragraph(
    library: Arc<Mutex<Library>>,
    translator: &impl Translator,
    book_id: Uuid,
    tgt_lang: &Language,
    paragraph_id: usize,
    worker_id: usize,
) -> anyhow::Result<()> {
    let (translation, paragraph_text) = {
        let book = library.lock().await.get_book(&book_id)?;
        let mut book = book.lock().await;
        let translation = book.get_or_create_translation(tgt_lang).await;
        let paragraph = book.book.paragraph_view(paragraph_id);
        (translation, paragraph.original_text.to_string())
    };
    println!(
        "Worker {worker_id}: Translating paragraph {}: \"{}...\"",
        paragraph_id,
        String::from_iter(paragraph_text.chars().take(40))
    );
    let p_translation = translator.get_translation(&paragraph_text).await?;
    println!("Worker {worker_id}: Translated paragraph {}", paragraph_id);

    translation
        .lock()
        .await
        .add_paragraph_translation(paragraph_id, &p_translation)
        .await?;

    {
        let book = library.lock().await.get_book(&book_id)?;
        let mut book = book.lock().await;
        book.save().await?;
    }

    Ok(())
}

async fn translate_book(
    library: Arc<Mutex<Library>>,
    cache: Arc<Mutex<TranslationsCache>>,
    api_key: &str,
    book_id: Uuid,
    tgt_lang: &str,
    n_workers: usize,
) -> anyhow::Result<()> {
    let target_lang = isolang::Language::from_str(tgt_lang)?;

    let queue = Arc::new(Mutex::new(VecDeque::new()));

    let source_lang = {
        let book = library.lock().await.get_book(&book_id)?;
        let mut book = book.lock().await;
        let source_lang = Language::from_639_3(&book.book.language).unwrap();

        let paragraph_count = book.book.paragraphs_count();

        let translation = book
            .get_or_create_translation(&target_lang)
            .await;
        let untranslated_paragraphs_count =
            paragraph_count - translation.lock().await.translated_paragraphs_count();
        println!(
            "Translating book {} from {} to {}",
            book.book.title,
            source_lang.to_name(),
            target_lang.to_name()
        );
        println!(
            "Found {untranslated_paragraphs_count} untranslated paragraphs out of {}",
            paragraph_count
        );

        for chapter in book.book.chapter_views() {
            for paragraph in chapter.paragraphs() {
                if translation
                    .lock()
                    .await
                    .paragraph_view(paragraph.id)
                    .is_none()
                {
                    queue.lock().await.push_back(paragraph.id);
                }
            }
        }

        source_lang
    };

    let start_time = Instant::now();

    let (tx, rx) = flume::unbounded();

    let mut set = JoinSet::new();
    for i in 0..n_workers {
        let library1 = library.clone();
        let rx = rx.clone();
        let translator = get_translator(
            cache.clone(),
            TranslationModel::GeminiFlash,
            api_key.to_owned(),
            source_lang,
            target_lang,
        )?;
        set.spawn(async move {
            println!("Worker {}: spawning...", i);
            let target_lang1 = target_lang.clone();
            // Receive until the channel is closed (all senders dropped)
            while let Ok(p_id) = rx.recv_async().await {
                // Bounded retry inside the worker instead of re-queuing
                let mut attempt = 1u32;
                loop {
                    let result = translate_paragraph(
                        library1.clone(),
                        &translator,
                        book_id,
                        &target_lang1,
                        p_id,
                        i,
                    )
                    .await;

                    match result {
                        Ok(_) => break,
                        Err(err) => {
                            eprintln!(
                                "Worker {i}: Error translating paragraph {p_id} (attempt {attempt}): {}",
                                err
                            );
                            if attempt >= 3 {
                                eprintln!(
                                    "Worker {i}: Giving up on paragraph {p_id} after {attempt} attempts"
                                );
                                break;
                            }
                            let backoff = Duration::from_secs((attempt * 2) as u64);
                            println!(
                                "Worker {i}: Backing off {backoff:?} before retrying paragraph {p_id}"
                            );
                            sleep(backoff).await;
                            attempt += 1;
                        }
                    }
                }
            }
            println!("Worker {i}: terminated");
        });
    }

    while let Some(p_id) = queue.lock().await.pop_front() {
        tx.send_async(p_id).await?;
    }

    drop(tx);

    set.join_all().await;

    let elapsed_time = start_time.elapsed();
    println!("Translated in: {:?}", elapsed_time);

    {
        println!("Saving...");
        let book = library.lock().await.get_book(&book_id)?;
        book.lock().await.save().await?;
        println!("Saved.");
    }

    Ok(())
}

async fn get_cache() -> anyhow::Result<TranslationsCache> {
    let dirs = ProjectDirs::from("", "TS", "FLTS").unwrap();
    let cache_dir = dirs.cache_dir();
    Ok(TranslationsCache::create(cache_dir).await?)
}

#[tokio::main]
async fn main() -> ExitCode {
    match do_main().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let error_chain = DisplayErrorChain::new(
                e.as_ref() as &(dyn std::error::Error + Send + Sync + 'static)
            );
            eprintln!("{error_chain}");
            ExitCode::FAILURE
        }
    }
}

async fn do_main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if !cli.library_path.exists() {
        create_dir(cli.library_path.clone())?;
    }

    let fs = PhysicalFS::new(cli.library_path);
    let library = Arc::new(Mutex::new(Library::open(fs.into())?));

    match &cli.command {
        Some(cmd) => match cmd {
            Commands::ImportBook { title, path, language } => {
                add_book(&library, title, path, &language).await?;
            }
            Commands::ImportEpub { path, language } => {
                add_epub(&library, path, &language).await?;
            }
            Commands::List {} => {
                list_books(&library).await?;
            }
            Commands::Translate {
                id,
                api_key,
                translation_language,
                n_parallel,
            } => {
                let cache = Arc::new(Mutex::new(get_cache().await?));
                translate_book(
                    library,
                    cache,
                    api_key,
                    *id,
                    translation_language,
                    n_parallel.unwrap_or(5),
                )
                .await?;
            }
        },
        None => {
            println!("Specify command");
        }
    }

    Ok(())
}
