//! Evolve the Library example while preserving every book's identity and revision history.
//! Run with `cargo run --example library_migration`.

use dbsync::schema::{Migration, Migrations, Store, Table};
use dbsync::{Result, Uuid, WriterId};

mod v1 {
    #[dbsync::entity]
    pub struct Book {
        #[index]
        title: String,
        chapters: Many<Chapter>,
        credits: Many<Credit>,
        #[through(Credit)]
        authors: Many<Author>,
    }
    #[dbsync::entity]
    pub struct Author {
        #[index]
        name: String,
        #[through(Credit)]
        books: Many<Book>,
    }
    #[dbsync::entity]
    pub struct Credit {
        book: One<Book>,
        author: One<Author>,
        role: String,
    }
    #[dbsync::entity]
    pub struct Chapter {
        book: One<Book>,
        title: String,
        paragraphs: Many<Paragraph>,
    }
    #[dbsync::entity]
    pub struct Paragraph {
        chapter: One<Chapter>,
        text: String,
    }
    #[dbsync::container(schema = "library", version = 1)]
    pub struct Library {
        books: Set<Book>,
        authors: Set<Author>,
        credits: Set<Credit>,
        chapters: Set<Chapter>,
        paragraphs: Set<Paragraph>,
    }
}

mod v2 {
    #[dbsync::entity]
    pub struct Book {
        #[index]
        title: String,
        subtitle: Option<String>,
        chapters: Many<Chapter>,
        credits: Many<Credit>,
        #[through(Credit)]
        authors: Many<Author>,
    }
    #[dbsync::entity]
    pub struct Author {
        #[index]
        name: String,
        #[through(Credit)]
        books: Many<Book>,
    }
    #[dbsync::entity]
    pub struct Credit {
        book: One<Book>,
        author: One<Author>,
        role: String,
    }
    #[dbsync::entity]
    pub struct Chapter {
        book: One<Book>,
        title: String,
        paragraphs: Many<Paragraph>,
    }
    #[dbsync::entity]
    pub struct Paragraph {
        chapter: One<Chapter>,
        content: String,
    }
    #[dbsync::container(schema = "library", version = 2)]
    pub struct Library {
        books: Set<Book>,
        authors: Set<Author>,
        credits: Set<Credit>,
        chapters: Set<Chapter>,
        paragraphs: Set<Paragraph>,
    }
}

fn migrations() -> Migrations<v2::Library> {
    // These functions run on every stored revision, including deleted and superseded ones.
    // Keep published conversions immutable and deterministic across replicas.
    Migrations::new().step(
        Migration::<v1::Library, v2::Library>::new()
            .map::<v1::Book, v2::Book>(|book| {
                Ok(v2::BookValue {
                    title: book.title,
                    subtitle: None,
                })
            })
            .map::<v1::Author, v2::Author>(|author| Ok(v2::AuthorValue { name: author.name }))
            .map::<v1::Credit, v2::Credit>(|credit| {
                Ok(v2::CreditValue {
                    book: credit.book.cast(),
                    author: credit.author.cast(),
                    role: credit.role,
                })
            })
            .map::<v1::Chapter, v2::Chapter>(|chapter| {
                Ok(v2::ChapterValue {
                    book: chapter.book.cast(),
                    title: chapter.title,
                })
            })
            .map::<v1::Paragraph, v2::Paragraph>(|paragraph| {
                Ok(v2::ParagraphValue {
                    chapter: paragraph.chapter.cast(),
                    content: paragraph.text,
                })
            }),
    )
}

struct DemoFile(std::path::PathBuf);
impl Drop for DemoFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn main() -> Result<()> {
    let demo = DemoFile(
        std::env::temp_dir().join(format!("dbsync-library-migration-{}.dbs", Uuid::now_v7())),
    );
    let path = &demo.0;
    let writer = WriterId::from_device_id("library-migration-example");
    let mut old = v1::Library::create(path, writer)?;
    let author = old.authors.push("Lewis Carroll")?;
    let book = old.books.push("Alice")?;
    let credit = old.credits.push(book, author, "wrote")?;
    let chapter = old.chapters.push(book, "Down the Rabbit-Hole")?;
    let paragraph = old.paragraphs.push(chapter, "Alice was tired.")?;
    old.commit(writer)?;
    old.books
        .set_title(book, "Alice's Adventures in Wonderland")?;
    old.commit(writer)?;
    let history = old.history(book)?;
    let clock = old.clock().clone();
    println!(
        "Version {}: {} book revisions",
        old.schema_metadata().version,
        history.len()
    );
    assert!(v2::Library::open(path).is_err_and(|err| err.is_unsupported()));

    let (mut library, report) = migrations().migrate(path)?;
    println!(
        "Applied {} migration: added Book.subtitle and renamed Paragraph.text to content",
        report.steps.len()
    );
    for step in &report.steps {
        println!(
            "  {} -> {}: {} converted revisions",
            step.from, step.to, step.converted_revisions
        );
    }
    let new_book = book.cast::<v2::Book>();
    assert_eq!(library.history(new_book)?.len(), history.len());
    assert_eq!(library.clock(), &clock);
    assert_eq!(library.books.subtitle(new_book)?, None);
    assert_eq!(library.chapters.book(chapter.cast())?, new_book);
    assert_eq!(library.credits.book(credit.cast())?, new_book);
    assert_eq!(library.credits.author(credit.cast())?.uuid(), author.uuid());
    assert_eq!(
        library.paragraphs.content(paragraph.cast())?,
        "Alice was tired."
    );
    assert_eq!(
        library
            .books
            .find_by_title("Alice's Adventures in Wonderland")?,
        vec![new_book]
    );
    println!(
        "  UUID {} and all history and references survived",
        new_book.uuid()
    );

    library
        .books
        .set_subtitle(new_book, Some("An illustrated edition"))?;
    library.commit(writer)?;
    library.verify()?;
    let (reopened, repeated) = migrations().migrate(path)?;
    assert!(repeated.steps.is_empty());
    assert_eq!(
        reopened.books.subtitle(new_book)?,
        Some("An illustrated edition".into())
    );
    assert_eq!(Table::rows(&reopened.books), 3);
    println!(
        "Version {}: new edits work; repeating migration makes no changes",
        reopened.schema_metadata().version
    );
    Ok(())
}
