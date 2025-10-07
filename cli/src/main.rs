use std::{error::Error, fmt::Display, fs::{create_dir, File}, io::Read, path::PathBuf};

use clap::{Parser, Subcommand};
use file_format::FileFormat;
use library::library::Library;
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
    /// List books
    List {}
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

fn add_book(library: &Library, title: &str, path: &PathBuf) -> anyhow::Result<()> {
    let fmt = FileFormat::from_file(path)?;

    if fmt.media_type() == "text/plain" {
        let mut data = File::open(path)?;
        let mut text = String::new();
        data.read_to_string(&mut text)?;

        let book = library.create_book_plain(title, &text)?;
        println!("Created book {} (id: {})", book.book.title, book.book.id);
    } else {
        Err(CliError::UnsupportedFormat(fmt.media_type().to_owned()))?
    }

    Ok(())
}

fn list_books(library: &Library) -> anyhow::Result<()> {
    let books = library.list_books()?;
    println!("id                                \ttitle\tchapters\tparagraphs");
    for book in books {
        println!("{}\t{}\t{}\t{}", book.id, book.title, book.chapters_count, book.paragraphs_count);
        if !book.translations_metadata.is_empty() {
            println!("\tTranslations:");
            println!("\tid                                \tsrc\ttgt\tparagraphs");
            for t in book.translations_metadata {
                println!("\t{}\t{}\t{}\t{}", t.id, t.source_langugage, t.target_language, t.translated_paragraphs_count);
            }
        }
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if !cli.library_path.exists() {
        create_dir(cli.library_path.clone())?;
    }

    let fs = PhysicalFS::new(cli.library_path);
    let library = Library::open(fs.into())?;

    match &cli.command {
        Some(cmd) => match cmd {
            Commands::ImportBook { title, path } => {
                add_book(&library, title, path)?;
            },
            Commands::List {  } => {
                list_books(&library)?;
            }
        },
        None => {
            println!("Specify command");
        }
    }

    Ok(())
}
