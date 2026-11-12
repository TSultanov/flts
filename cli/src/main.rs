use std::{
    error::Error,
    fmt::Display,
    fs::{File, create_dir},
    io::Read,
    path::{Path, PathBuf},
    process::ExitCode,
    str::FromStr,
    time::Instant,
};

use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use display_error_chain::DisplayErrorChain;
use file_format::FileFormat;
use futures_util::{StreamExt, stream::FuturesUnordered};
use isolang::Language;
use library::{
    cache::{GEMINI_PROMPT_CACHE_CAPACITY, TranslationsCache},
    epub_importer::EpubBook,
    library::Library,
    translator::{
        NoChapterContext, TranslationContext, TranslationProvider, Translator,
        catalog::{FALLBACK_GOOGLE, effective_model_id},
        gemini_cache::GeminiPromptCache,
        get_translator,
    },
};
use tokio::time::{Duration, sleep};
use uuid::Uuid;

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
    /// Dump chapter summaries for a book
    Summaries {
        /// Book ID
        id: Uuid,
        /// Only show this chapter (0-indexed)
        #[arg(short, long)]
        chapter: Option<usize>,
        /// Hide summary text; print one header line per chapter
        #[arg(long)]
        headers_only: bool,
    },
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
        /// Translation model API id (default: Gemini 3.7 Flash)
        #[arg(long, value_name = "MODEL")]
        model: Option<String>,
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
    library: &Library,
    title: &str,
    path: &PathBuf,
    lang: &str,
) -> anyhow::Result<()> {
    let fmt = FileFormat::from_file(path)?;

    if fmt.media_type() == "text/plain" {
        let mut data = File::open(path)?;
        let mut text = String::new();
        data.read_to_string(&mut text)?;

        let book_id = library
            .create_book_plain(title, &text, &Language::from_str(lang)?)
            .await?;
        let book = library.get_book(&book_id).await?.snapshot();
        println!("Created book {} (id: {})", book.book.title, book.book.id);
    } else {
        Err(CliError::UnsupportedFormat(fmt.media_type().to_owned()))?
    }

    Ok(())
}

async fn add_epub(library: &Library, path: &Path, lang: &str) -> anyhow::Result<()> {
    let epub = EpubBook::load(path)?;

    let book_id = library
        .create_book_epub(&epub, &Language::from_str(lang)?)
        .await?;
    let book = library.get_book(&book_id).await?.snapshot();
    println!("Created book {} (id: {})", book.book.title, book.book.id);

    Ok(())
}

async fn dump_summaries(
    library: &Library,
    book_id: Uuid,
    chapter_filter: Option<usize>,
    headers_only: bool,
) -> anyhow::Result<()> {
    let book = library
        .get_book(&book_id)
        .await
        .map_err(|err| anyhow::anyhow!("Book {book_id} not found in library: {err}"))?;
    let outline = book.read(|book| book.outline()).await?;
    let book = book.snapshot();
    let summaries = &book.summaries;

    let titles: Vec<Option<String>> = (0..summaries.entries.len())
        .map(|i| outline.chapters.get(i).and_then(|c| c.title.clone()))
        .collect();

    let generated = summaries.entries.iter().filter(|e| e.generated).count();
    println!("Book: {} ({})", book.book.title, book.book.id);
    println!(
        "Chapters: {} generated, {} pending",
        generated,
        summaries.entries.len() - generated,
    );
    println!();

    for (i, entry) in summaries.entries.iter().enumerate() {
        if let Some(only) = chapter_filter
            && only != i
        {
            continue;
        }

        let title = titles.get(i).and_then(|t| t.as_deref()).unwrap_or("");
        let title_suffix = if title.is_empty() {
            String::new()
        } else {
            format!(": {title}")
        };
        println!("=== Chapter {i}{title_suffix} ===");

        if !entry.generated {
            println!("pending");
            println!();
            continue;
        }
        println!(
            "generated=true model={:?} timestamp={} chars={}",
            entry.model,
            entry.timestamp,
            entry.text.chars().count(),
        );

        if !headers_only {
            println!();
            println!("{}", entry.text);
        }
        println!();
    }

    Ok(())
}

async fn list_books(library: &Library) -> anyhow::Result<()> {
    let books = library.list_books().await?;
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
    library: &Library,
    translator: &dyn Translator,
    book_id: Uuid,
    tgt_lang: &Language,
    paragraph_id: usize,
    worker_id: usize,
) -> anyhow::Result<()> {
    let book_handle = library.get_book(&book_id).await?;
    let (paragraph_text, chapter_id) = book_handle
        .read(move |book| {
            let chapter = book.outline()?.chapter_for_paragraph(paragraph_id);
            Ok((book.paragraph(paragraph_id)?.text, chapter.unwrap_or(0)))
        })
        .await?;
    println!(
        "Worker {worker_id}: Translating paragraph {}: \"{}...\"",
        paragraph_id,
        String::from_iter(paragraph_text.chars().take(40))
    );
    let p_translation = translator
        .get_translation(TranslationContext {
            paragraph_text: &paragraph_text,
            book_id,
            chapter_id,
            use_cache: true,
            callback: None,
        })
        .await?;
    println!("Worker {worker_id}: Translated paragraph {}", paragraph_id);

    let handle = book_handle;
    handle
        .apply_translation(
            paragraph_id,
            *tgt_lang,
            p_translation,
            translator.get_model(),
        )
        .await?;
    handle.flush().await?;

    Ok(())
}

async fn translate_book(
    library: Library,
    cache: TranslationsCache,
    gemini_prompt_cache: GeminiPromptCache,
    api_key: &str,
    book_id: Uuid,
    tgt_lang: &str,
    n_workers: usize,
    model: &str,
) -> anyhow::Result<()> {
    let target_lang = isolang::Language::from_str(tgt_lang)?;

    let queue;

    let source_lang = {
        let handle = library.get_book(&book_id).await?;
        let book = handle.snapshot();
        let source_lang = Language::from_639_3(&book.book.language).unwrap();

        let paragraph_count = book.book.paragraphs_count();
        let untranslated_ids: Vec<usize> = handle
            .read(move |book| {
                let outline = book.outline()?;
                let mut ids = Vec::new();
                for chapter in &outline.chapters {
                    let translated = book.translated(&target_lang, &chapter.paragraphs)?;
                    ids.extend(
                        chapter
                            .paragraphs
                            .iter()
                            .zip(translated)
                            .filter(|(_, translated)| !translated)
                            .map(|(id, _)| *id),
                    );
                }
                Ok(ids)
            })
            .await?;
        println!(
            "Translating book {} from {} to {}",
            book.book.title,
            source_lang.to_name(),
            target_lang.to_name()
        );
        println!(
            "Found {} untranslated paragraphs out of {}",
            untranslated_ids.len(),
            paragraph_count
        );
        queue = untranslated_ids;

        source_lang
    };

    let model = effective_model_id(TranslationProvider::Google, model);

    let start_time = Instant::now();

    let (tx, rx) = flume::unbounded();

    let context_provider = NoChapterContext;
    let mut set = FuturesUnordered::new();
    for i in 0..n_workers {
        let library1 = &library;
        let rx = rx.clone();
        let translator = get_translator(
            &cache,
            &context_provider,
            &gemini_prompt_cache,
            TranslationProvider::Google,
            &model,
            api_key.to_owned(),
            source_lang,
            target_lang,
        )?;
        set.push(async move {
            println!("Worker {}: spawning...", i);
            let target_lang1 = target_lang;
            while let Ok(p_id) = rx.recv_async().await {
                let mut attempt = 1u32;
                loop {
                    let result = translate_paragraph(
                        library1,
                        translator.as_ref(),
                        book_id,
                        &target_lang1,
                        p_id,
                        i,
                    )
                    .await;

                    match result {
                        Ok(_) => {
                            break
                        },
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

    for p_id in queue {
        tx.send_async(p_id).await?;
    }

    drop(tx);

    while set.next().await.is_some() {}

    library.flush().await?;

    let elapsed_time = start_time.elapsed();
    println!("Translated in: {:?}", elapsed_time);

    Ok(())
}

async fn get_cache() -> anyhow::Result<(TranslationsCache, library::task::TaskOwner)> {
    let dirs = ProjectDirs::from("", "TS", "FLTS").unwrap();
    let cache_dir = dirs.cache_dir();
    TranslationsCache::create(cache_dir).await
}

async fn get_gemini_prompt_cache() -> anyhow::Result<(GeminiPromptCache, library::task::TaskOwner)>
{
    let dirs = ProjectDirs::from("", "TS", "FLTS").unwrap();
    let cache_dir = dirs.cache_dir().join("gemini_caches");
    GeminiPromptCache::open(&cache_dir, GEMINI_PROMPT_CACHE_CAPACITY).await
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

    let library_path = cli.library_path.clone();
    if !library_path.exists() {
        create_dir(library_path.clone())?;
    }

    let library = Library::open(library_path).await?;

    match &cli.command {
        Some(cmd) => match cmd {
            Commands::ImportBook {
                title,
                path,
                language,
            } => {
                add_book(&library, title, path, language).await?;
            }
            Commands::ImportEpub { path, language } => {
                add_epub(&library, path, language).await?;
            }
            Commands::List {} => {
                list_books(&library).await?;
            }
            Commands::Summaries {
                id,
                chapter,
                headers_only,
            } => {
                dump_summaries(&library, *id, *chapter, *headers_only).await?;
            }
            Commands::Translate {
                id,
                api_key,
                translation_language,
                n_parallel,
                model,
            } => {
                let (cache, cache_owner) = get_cache().await?;
                let (gemini_prompt_cache, gemini_owner) = get_gemini_prompt_cache().await?;
                let model = model.as_deref().unwrap_or(FALLBACK_GOOGLE);
                translate_book(
                    library,
                    cache,
                    gemini_prompt_cache,
                    api_key,
                    *id,
                    translation_language,
                    n_parallel.unwrap_or(5),
                    model,
                )
                .await?;
                cache_owner.shutdown().await.map_err(anyhow::Error::msg)?;
                gemini_owner.shutdown().await.map_err(anyhow::Error::msg)?;
            }
        },
        None => {
            println!("Specify command");
        }
    }

    Ok(())
}
