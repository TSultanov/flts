//! A library of books, chapters and paragraphs, declared once and derived into columns.

use std::path::PathBuf;

use dbsync::schema::{Holds, Store, Table};
use dbsync::{ColumnFile, Result, WriterId};

#[dbsync::entity]
struct Book {
    #[index]
    title: String,
    author: String,
    chapters: Many<Chapter>,
}

#[dbsync::entity]
struct Chapter {
    book: One<Book>,
    title: String,
    paragraphs: Many<Paragraph>,
}

#[dbsync::entity]
struct Paragraph {
    chapter: One<Chapter>,
    text: String,
}

#[dbsync::container]
struct Library {
    books: Set<Book>,
    chapters: Set<Chapter>,
    paragraphs: Set<Paragraph>,
}

fn this_device() -> WriterId {
    WriterId::from_device_id("KRZH3XY-7PH4NJK-2WQ5LMC")
}

fn stock(library: &mut Library) -> Result<()> {
    let prince = library
        .books
        .push("Le Petit Prince", "Antoine de Saint-Exupéry")?;
    let one = library.chapters.push(prince, "Chapitre I")?;
    library
        .paragraphs
        .push(one, "Lorsque j'avais six ans j'ai vu une image.")?;
    library
        .paragraphs
        .push(one, "On disait: les serpents avalent leur proie.")?;
    let two = library.chapters.push(prince, "Chapitre II")?;
    library
        .paragraphs
        .push(two, "J'ai vécu seul, sans personne à qui parler.")?;

    let kokoro = library.books.push("こころ", "夏目漱石")?;
    let sensei = library.chapters.push(kokoro, "上 先生と私")?;
    library
        .paragraphs
        .push(sensei, "私はその人を常に先生と呼んでいた。")?;
    library
        .paragraphs
        .push(sensei, "筆を執っても心持は同じ事である。")?;

    let alice = library
        .books
        .push("Alice's Adventures in Wonderland", "Lewis Carroll")?;
    let rabbit = library.chapters.push(alice, "Down the Rabbit-Hole")?;
    library
        .paragraphs
        .push(rabbit, "Alice was beginning to get very tired.")?;
    library
        .paragraphs
        .push(rabbit, "So she was considering in her own mind.")?;
    let pool = library.chapters.push(alice, "The Pool of Tears")?;
    library
        .paragraphs
        .push(pool, "Curiouser and curiouser! cried Alice.")?;
    Ok(())
}

fn book_called<'a>(library: &'a Library, title: &str) -> Result<BookRow<'a, Library>> {
    match Holds::<Book>::table(library).find_by_title(title)?.first() {
        Some(book) => library.at(*book),
        None => Err(dbsync::Error::invalid(format!("no book called {title:?}"))),
    }
}

fn resident(library: &Library, column: &str) -> usize {
    library
        .residency()
        .into_iter()
        .find(|held| held.name == column)
        .expect("the schema names this column")
        .resident
}

fn main() -> Result<()> {
    let path: PathBuf =
        std::env::temp_dir().join(format!("dbsync-library-{}.dbs", std::process::id()));
    println!("a library in one column file");
    println!("  {}", path.display());

    let declared = Library::default();
    println!(
        "\n1. four declarations became {} columns",
        declared.specs().len()
    );
    for spec in declared.specs() {
        println!("     {} holding {}-byte rows", spec.name, spec.elem_size);
    }
    println!("     nobody wrote those names down; Many and Set asked for none of them,");
    println!("     and nobody declared an id: every entity is given one, and a row keeps");
    println!("     the one it was given for as long as it exists");
    println!("     .buckets and .links are the hash index over the column they follow --");
    println!("     columns like any other, written by the commit and paged on demand");

    let mut library = Library::create(&path, this_device())?;
    stock(&mut library)?;
    let generation = library.commit(this_device())?;
    library.verify()?;
    assert_eq!(
        Table::rows(&library.books)
            + Table::rows(&library.chapters)
            + Table::rows(&library.paragraphs),
        3 + 5 + 8
    );
    println!("\n2. shelved the books and committed generation {generation}");
    println!(
        "     {} books, {} chapters, {} paragraphs in {} bytes",
        Table::rows(&library.books),
        Table::rows(&library.chapters),
        Table::rows(&library.paragraphs),
        std::fs::metadata(&path)?.len()
    );
    println!("     generation 1 was the empty file these columns were committed into");

    let shelf = Library::open(&path)?;
    assert!(shelf.residency().iter().all(|held| held.resident == 0));
    println!("\n3. reopened it, and opening a column read no payload at all");
    println!(
        "     resident chunks across all {} columns: 0",
        shelf.specs().len()
    );

    for book in shelf.all::<Book>() {
        println!("     {} — {}", book.title()?, book.author()?);
    }
    assert!(resident(&shelf, "book.title.bytes") > 0);
    assert_eq!(resident(&shelf, "paragraph.text"), 0);
    assert_eq!(resident(&shelf, "paragraph.text.bytes"), 0);
    println!(
        "     listing the catalogue read {} chunks of book.title.bytes",
        resident(&shelf, "book.title.bytes")
    );
    println!("     and left paragraph.text.bytes entirely cold: every string field");
    println!("     has its own heap, so a title cannot fault in a paragraph");

    let cold = Library::open(&path)?;
    let prince = book_called(&cold, "Le Petit Prince")?;
    println!("\n4. asked a handle that had read nothing for one book by its title");
    assert_eq!(resident(&cold, "book.title"), 0);
    assert_eq!(resident(&cold, "book.title.bytes"), 0);
    assert!(resident(&cold, "book.title.buckets") > 0);
    println!("     Le Petit Prince is fifteen bytes, so the whole title sits in the index:",);
    println!(
        "     finding it read {} chunks of book.title.buckets and left both book.title",
        resident(&cold, "book.title.buckets")
    );
    println!("     and book.title.bytes entirely cold");

    let alice_cold = book_called(&cold, "Alice's Adventures in Wonderland")?;
    assert_eq!(resident(&cold, "book.title"), 0);
    assert_eq!(resident(&cold, "book.title.bytes"), 1);
    println!("     a title too long to carry keeps a digest and where its bytes are, so");
    println!("     finding that one read a single chunk of book.title.bytes to confirm --");
    println!("     and still not one chunk of book.title, the column being indexed");
    println!(
        "     reading a title back is a separate act: {} and {}",
        prince.title()?,
        alice_cold.title()?
    );

    println!("\n5. opened one book, and walked it by the columns that point back at it");
    let alice = book_called(&shelf, "Alice's Adventures in Wonderland")?;
    assert_eq!(alice.chapters()?.count(), 2);
    assert_eq!(resident(&shelf, "chapter.book"), 0);
    assert!(resident(&shelf, "chapter.book.buckets") > 0);
    println!(
        "     finding them read {} chunks of chapter.book.buckets and left chapter.book",
        resident(&shelf, "chapter.book.buckets")
    );
    println!("     cold: a link carries the parent it files a child under, so the index");
    println!("     answers without reading the column it indexes");
    for chapter in alice.chapters()? {
        println!("     {}", chapter.title()?);
        assert_eq!(chapter.book()?.id()?, alice.id()?);
        for paragraph in chapter.paragraphs()? {
            println!("       {}", paragraph.text()?);
            assert_eq!(paragraph.chapter()?.id()?, chapter.id()?);
        }
    }
    println!("     nobody named a table or carried an id between the two levels");

    let before = std::fs::metadata(&path)?.len();
    let untouched = ColumnFile::open(&path)?
        .column_meta("paragraph.text")
        .expect("paragraph.text is in the file")
        .chunks[0];

    library.books.push("Il nome della rosa", "Umberto Eco")?;
    library
        .at_mut(alice.id()?)?
        .set_title("Alice in Wonderland")?;
    let published = library.commit(this_device())?;
    assert_eq!(library.at(alice.id()?)?.title()?, "Alice in Wonderland");
    assert_eq!(library.at(alice.id()?)?.author()?, "Lewis Carroll");

    assert_eq!(
        ColumnFile::open(&path)?
            .column_meta("paragraph.text")
            .expect("paragraph.text is still in the file")
            .chunks[0],
        untouched
    );
    println!(
        "\n6. catalogued a fourth book, retitled a first, and committed generation {published}"
    );
    println!(
        "     the file grew by {} bytes; paragraph.text was never touched and did not move",
        std::fs::metadata(&path)?.len() - before
    );
    println!("     these columns hold thousands of rows per chunk, so at this size each is");
    println!("     one chunk; a real library appends to the tail of the column that grew");

    assert_eq!(Table::rows(&shelf.books), 3);
    assert_eq!(Table::rows(&library.books), 4);
    println!("\n7. the handle opened at the older generation still serves it");
    println!(
        "     it reads {} books from the same file the live one reads {} from",
        Table::rows(&shelf.books),
        Table::rows(&library.books)
    );

    let mut stale = shelf;
    stale.books.push("Never committed", "Nobody")?;
    let refused = stale
        .commit(this_device())
        .expect_err("a stale handle cannot commit");
    println!("\n8. and that handle can no longer commit over the newer generation");
    println!("     {refused}");
    println!("     the caller reopens and retries; nothing was written");

    let token = ColumnFile::read_commit_token(&path)?;
    assert_eq!(token.generation, published);
    assert!(!token.is_echo_of(this_device(), generation));
    println!(
        "     a peer polls read_commit_token for that: generation {}, not an echo of its own write",
        token.generation
    );

    std::fs::remove_file(&path)?;
    Ok(())
}
