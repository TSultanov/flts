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
};

use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use display_error_chain::DisplayErrorChain;
use file_format::FileFormat;
use isolang::Language;
use library::{
    cache::TranslationsCache,
    epub_importer::EpubBook,
    library::Library,
    translator::{TranslationModel, Translator, get_translator},
};
use tokio::sync::{Mutex};
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
        title: String,
        /// Path to book file
        #[arg(short, long, value_name = "FILE")]
        path: PathBuf,
    },
    /// Add book to library from EPUB
    ImportEpub {
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
        /// Book language
        #[arg(short, long, value_name = "LANG")]
        book_language: String,
        /// Translation language
        #[arg(short, long, value_name = "LANG")]
        translation_language: String,
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
) -> anyhow::Result<()> {
    let fmt = FileFormat::from_file(path)?;

    if fmt.media_type() == "text/plain" {
        let mut data = File::open(path)?;
        let mut text = String::new();
        data.read_to_string(&mut text)?;

        let book_id = library.lock().await.create_book_plain(title, &text).await?;
        let book = library.lock().await.get_book(&book_id)?;
        println!(
            "Created book {} (id: {})",
            book.lock().await.book.title,
            book.lock().await.book.id
        );
    } else {
        Err(CliError::UnsupportedFormat(fmt.media_type().to_owned()))?
    }

    Ok(())
}

async fn add_epub(library: &Arc<Mutex<Library>>, path: &PathBuf) -> anyhow::Result<()> {
    let epub = EpubBook::load(path)?;

    let book_id = library.lock().await.create_book_epub(&epub).await?;
    let book = library.lock().await.get_book(&book_id)?;
    println!(
        "Created book {} (id: {})",
        book.lock().await.book.title,
        book.lock().await.book.id
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
    src_lang: &Language,
    tgt_lang: &Language,
    paragraph_id: usize,
    worker_id: usize,
) -> anyhow::Result<()> {
    let (translation, paragraph_text) = {
        let book = library.lock().await.get_book(&book_id)?;
        let mut book = book.lock().await;
        let translation = book
            .get_or_create_translation(src_lang.to_639_3(), tgt_lang.to_639_3())
            .await;
        let paragraph = book.book.paragraph_view(paragraph_id);
        (translation, paragraph.original_text.to_string())
    };
    println!(
        "Worker {worker_id}: Translating paragraph {}: \"{}...\"",
        paragraph_id,
        String::from_iter(paragraph_text.chars().take(40))
    );
    let p_translation = 
        translator.get_translation(&paragraph_text)
        .await?;
    println!("Worker {worker_id}: Translated paragraph {}", paragraph_id);

    translation
        .lock()
        .await
        .add_paragraph_translation(paragraph_id, &p_translation);

    Ok(())
}

async fn translate_book(
    library: Arc<Mutex<Library>>,
    cache: Arc<Mutex<TranslationsCache>>,
    api_key: &str,
    book_id: Uuid,
    src_lang: &str,
    tgt_lang: &str,
) -> anyhow::Result<()> {
    let source_lang = isolang::Language::from_str(src_lang)?;
    let target_lang = isolang::Language::from_str(tgt_lang)?;

    let queue = Arc::new(Mutex::new(VecDeque::new()));

    {
        let book = library.lock().await.get_book(&book_id)?;
        let mut book = book.lock().await;
        let paragraph_count = book.book.paragraphs_count();

        let translation = book
            .get_or_create_translation(source_lang.to_639_3(), target_lang.to_639_3())
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
    }

    let (tx, rx) = flume::bounded(100);

    for i in 0..100 {
        let library1 = library.clone();
        let queue1 = queue.clone();
        let rx = rx.clone();
            let translator = get_translator(
            cache.clone(),
            TranslationModel::GeminiFlash,
            api_key.to_owned(),
            target_lang.to_name().to_owned(),
        )?;
        tokio::spawn(async move {
            println!("Worker {}: spawning...", i);
            let source_lang1 = source_lang.clone();
            let target_lang1 = target_lang.clone();
            while let Some(p_id) = rx.recv_async().await.ok() {
                let result = translate_paragraph(
                    library1.clone(),
                    &translator,
                    book_id,
                    &source_lang1,
                    &target_lang1,
                    p_id,
                    i
                )
                .await;

                match result {
                    Ok(_) => {}
                    Err(err) => {
                        eprintln!("Worker {i}: Error translating paragarph {p_id}: {}", err);
                        println!("Worker {i}: Retrying paragraph {p_id}");
                        queue1.lock().await.push_front(p_id);
                    }
                }
            }
        });
    }

    while let Some(p_id) = queue.lock().await.pop_front() {
        tx.send_async(p_id).await?;
    }

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
            Commands::ImportBook { title, path } => {
                add_book(&library, title, path).await?;
            }
            Commands::ImportEpub { path } => {
                add_epub(&library, path).await?;
            }
            Commands::List {} => {
                list_books(&library).await?;
            }
            Commands::Translate {
                id,
                api_key,
                book_language,
                translation_language,
            } => {
                let cache = Arc::new(Mutex::new(get_cache().await?));
                translate_book(
                    library,
                    cache,
                    api_key,
                    *id,
                    book_language,
                    translation_language,
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
