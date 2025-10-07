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
            }
        },
        None => {
            println!("Specify command");
        }
    }

    Ok(())
}
