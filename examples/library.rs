//! A library of books, authors, chapters and paragraphs, declared once and derived into columns.

use std::path::PathBuf;

use dbsync::schema::{Holds, Store, Table};
use dbsync::{ColumnFile, Result, WriterId};

#[dbsync::entity]
struct Book {
    #[index]
    title: String,
    chapters: Many<Chapter>,
    credits: Many<Credit>,
    #[through(Credit)]
    authors: Many<Author>,
}

#[dbsync::entity]
struct Author {
    #[index]
    name: String,
    #[through(Credit)]
    books: Many<Book>,
}

#[dbsync::entity]
#[distinct(book, author)]
struct Credit {
    book: One<Book>,
    author: One<Author>,
    role: String,
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
    authors: Set<Author>,
    credits: Set<Credit>,
    chapters: Set<Chapter>,
    paragraphs: Set<Paragraph>,
}

fn this_device() -> WriterId {
    WriterId::from_device_id("KRZH3XY-7PH4NJK-2WQ5LMC")
}

fn stock(library: &mut Library) -> Result<()> {
    let exupery = library.authors.push("Antoine de Saint-Exupéry")?;
    let soseki = library.authors.push("夏目漱石")?;
    let carroll = library.authors.push("Lewis Carroll")?;
    let tenniel = library.authors.push("John Tenniel")?;

    let prince = library.books.push("Le Petit Prince")?;
    library.credits.push(prince, exupery, "wrote")?;
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

    let kokoro = library.books.push("こころ")?;
    library.credits.push(kokoro, soseki, "wrote")?;
    let sensei = library.chapters.push(kokoro, "上 先生と私")?;
    library
        .paragraphs
        .push(sensei, "私はその人を常に先生と呼んでいた。")?;
    library
        .paragraphs
        .push(sensei, "筆を執っても心持は同じ事である。")?;

    let alice = library.books.push("Alice's Adventures in Wonderland")?;
    library.credits.push(alice, carroll, "wrote")?;
    library.credits.push(alice, tenniel, "illustrated")?;
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

    let glass = library.books.push("Through the Looking-Glass")?;
    library.credits.push(glass, carroll, "wrote")?;
    library.credits.push(glass, tenniel, "illustrated")?;
    let looking = library.chapters.push(glass, "Looking-Glass House")?;
    library.paragraphs.push(
        looking,
        "One thing was certain, that the white kitten had nothing to do with it.",
    )?;
    Ok(())
}

fn named<'a>(library: &'a Library, name: &str) -> Result<AuthorRow<'a, Library>> {
    match Holds::<Author>::table(library).find_by_name(name)?.first() {
        Some(author) => library.at(*author),
        None => Err(dbsync::Error::invalid(format!("nobody called {name:?}"))),
    }
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
        "\n1. five declarations became {} columns",
        declared.specs().len()
    );
    for spec in declared.specs() {
        println!("     {} holding {}-byte rows", spec.name, spec.elem_size);
    }
    println!("     nobody wrote those names down; Many and Set asked for none of them,");
    println!("     and nobody declared an id: every entity is given one, and a row keeps");
    println!("     the one it was given for as long as it exists");
    println!("     nobody declared a version either: every row carries the write that");
    println!("     made it, and dbsync.clock is how far this copy has counted");
    println!("     credit.book_author is the pair no two credits may share, carried whole,");
    println!("     so a book and a person are held apart by one key rather than two");
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
        4 + 6 + 9
    );
    println!("\n2. shelved the books and committed generation {generation}");
    println!(
        "     {} books by {} people in {} credits, {} chapters, {} paragraphs in {} bytes",
        Table::rows(&library.books),
        Table::rows(&library.authors),
        Table::rows(&library.credits),
        Table::rows(&library.chapters),
        Table::rows(&library.paragraphs),
        std::fs::metadata(&path)?.len()
    );
    println!("     generation 1 was the empty file these columns were committed into");

    let shelf = Library::open(&path)?;
    assert!(
        shelf
            .residency()
            .iter()
            .filter(|held| held.name != dbsync::schema::CLOCK)
            .all(|held| held.resident == 0)
    );
    assert_eq!(resident(&shelf, dbsync::schema::CLOCK), 1);
    println!("\n3. reopened it, and opening a column read no payload at all");
    println!(
        "     resident chunks across all {} columns: 1",
        shelf.specs().len()
    );
    println!("     and the one is dbsync.clock, which an open must read because a store");
    println!("     cannot name its next write without knowing how far it has counted");

    for book in shelf.all::<Book>() {
        println!("     {}", book.title()?);
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

    println!("\n6. and crossed the table that holds books and people together");
    let crossing = Library::open(&path)?;
    let looking = book_called(&crossing, "Through the Looking-Glass")?;
    assert_eq!(looking.authors()?.count(), 2);
    assert_eq!(resident(&crossing, "credit.book"), 0);
    assert!(resident(&crossing, "credit.book.buckets") > 0);
    assert!(resident(&crossing, "credit.author") > 0);
    assert_eq!(resident(&crossing, "author.name"), 0);
    assert_eq!(resident(&crossing, "author.name.bytes"), 0);
    println!(
        "     the two people behind it were named by {} chunks of credit.book.buckets and",
        resident(&crossing, "credit.book.buckets")
    );
    println!(
        "     {} of credit.author, and nothing else: a Many that names a table to cross",
        resident(&crossing, "credit.author")
    );
    println!("     walks the index on one end of the link and reads the id at the other, so");
    println!("     credit.book stayed cold, and so did author.name -- naming rows and reading");
    println!("     what they hold are separate acts, exactly as they were for a title");

    for role in looking.credits()? {
        println!("     {} {}", role.author()?.name()?, role.role()?);
    }
    assert!(resident(&crossing, "author.name.bytes") > 0);
    println!("     asking for the names is what read author.name.bytes, and the credits are");
    println!("     still rows carrying a role of their own, because the join is a table that");
    println!("     was declared rather than one that was conjured");

    let tenniel = named(&crossing, "John Tenniel")?;
    let drawn: Vec<String> = tenniel
        .books()?
        .map(|book| Ok(book.title()?.into_owned()))
        .collect::<Result<Vec<String>>>()?;
    assert_eq!(drawn.len(), 2);
    println!("     the same table answers the other way round, declared the same way: John",);
    println!("     Tenniel illustrated {} and {}", drawn[0], drawn[1]);

    let mut again = Library::open(&path)?;
    let carroll = named(&again, "Lewis Carroll")?.id()?;
    let glass = book_called(&again, "Through the Looking-Glass")?.id()?;
    let refused = again
        .credits
        .push(glass, carroll, "wrote again")
        .expect_err("this pair is already held");
    println!("\n7. offered a credit the library already holds");
    println!("     {refused}");
    assert_eq!(Table::rows(&again.credits), 6);
    again.verify()?;
    println!("     nothing was written: the pair is probed before an id is minted, so a");
    println!("     refused push leaves every column exactly the length it was");

    let before = std::fs::metadata(&path)?.len();
    let untouched = ColumnFile::open(&path)?
        .column_meta("paragraph.text")
        .expect("paragraph.text is in the file")
        .chunks[0];

    let eco = library.authors.push("Umberto Eco")?;
    let rosa = library.books.push("Il nome della rosa")?;
    library.credits.push(rosa, eco, "wrote")?;
    library
        .at_mut(alice.id()?)?
        .set_title("Alice in Wonderland")?;
    let published = library.commit(this_device())?;
    assert_eq!(library.at(alice.id()?)?.title()?, "Alice in Wonderland");
    assert_eq!(library.at(alice.id()?)?.authors()?.count(), 2);

    assert_eq!(
        ColumnFile::open(&path)?
            .column_meta("paragraph.text")
            .expect("paragraph.text is still in the file")
            .chunks[0],
        untouched
    );
    println!(
        "\n8. catalogued a fifth book, retitled a first, and committed generation {published}"
    );
    println!(
        "     the file grew by {} bytes; paragraph.text was never touched and did not move",
        std::fs::metadata(&path)?.len() - before
    );
    println!("     these columns hold thousands of rows per chunk, so at this size each is");
    println!("     one chunk; a real library appends to the tail of the column that grew");
    println!(
        "     retitling Alice did not write over her row: book now holds {} rows for {} books,",
        Table::rows(&library.books),
        library.all::<Book>().count()
    );
    println!("     because a write appends the version that replaces the one before it, and");
    println!("     the row it replaced is still there to be read and still says \"Alice\"");

    assert_eq!(shelf.all::<Book>().count(), 4);
    assert_eq!(library.all::<Book>().count(), 5);
    assert_eq!(
        Table::rows(&library.books),
        6,
        "five books in six rows: retitling Alice appended the row it replaced"
    );
    println!("\n9. the handle opened at the older generation still serves it");
    println!(
        "     it reads {} books from the same file the live one reads {} from",
        shelf.all::<Book>().count(),
        library.all::<Book>().count()
    );

    let mut stale = shelf;
    stale.books.push("Never committed")?;
    let refused = stale
        .commit(this_device())
        .expect_err("a stale handle cannot commit");
    println!("\n10. and that handle can no longer commit over the newer generation");
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
