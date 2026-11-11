//! A library of books, authors, chapters and paragraphs, declared once and derived into columns.

use std::path::PathBuf;

use dbsync::schema::{Holds, Layout, Store, Table};
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

struct Credit {
    book: One<Book>,
    author: One<Author>,
    role: String,
}

#[dbsync::entity]
struct Chapter {
    book: One<Book>,
    #[order(book)]
    number: u32,
    title: String,
    paragraphs: Many<Paragraph>,
}

#[dbsync::entity]
struct Paragraph {
    chapter: One<Chapter>,
    #[order(chapter)]
    number: u32,
    text: String,
}

#[dbsync::container(schema = "library", version = 1)]
struct Library {
    books: Set<Book>,
    authors: Set<Author>,
    credits: Set<Credit>,
    chapters: Set<Chapter>,
    paragraphs: Set<Paragraph>,
}

fn copy_store(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_store(&entry.path(), &target)?;
        } else if entry.file_name() != "lock" {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn bytes_in(dir: &std::path::Path) -> std::io::Result<u64> {
    let mut bytes = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        bytes += match entry.file_type()?.is_dir() {
            true => bytes_in(&entry.path())?,
            false => entry.metadata()?.len(),
        };
    }
    Ok(bytes)
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
    let one = library.chapters.push(prince, 1, "Chapitre I")?;
    library
        .paragraphs
        .push(one, 1, "Lorsque j'avais six ans j'ai vu une image.")?;
    library
        .paragraphs
        .push(one, 2, "On disait: les serpents avalent leur proie.")?;
    let two = library.chapters.push(prince, 2, "Chapitre II")?;
    library
        .paragraphs
        .push(two, 1, "J'ai vécu seul, sans personne à qui parler.")?;

    let kokoro = library.books.push("こころ")?;
    library.credits.push(kokoro, soseki, "wrote")?;
    let sensei = library.chapters.push(kokoro, 1, "上 先生と私")?;
    library
        .paragraphs
        .push(sensei, 1, "私はその人を常に先生と呼んでいた。")?;
    library
        .paragraphs
        .push(sensei, 2, "筆を執っても心持は同じ事である。")?;

    let alice = library.books.push("Alice's Adventures in Wonderland")?;
    library.credits.push(alice, carroll, "wrote")?;
    library.credits.push(alice, tenniel, "illustrated")?;
    let rabbit = library.chapters.push(alice, 1, "Down the Rabbit-Hole")?;
    library
        .paragraphs
        .push(rabbit, 1, "Alice was beginning to get very tired.")?;
    library
        .paragraphs
        .push(rabbit, 2, "So she was considering in her own mind.")?;
    let pool = library.chapters.push(alice, 2, "The Pool of Tears")?;
    library
        .paragraphs
        .push(pool, 1, "Curiouser and curiouser! cried Alice.")?;

    let glass = library.books.push("Through the Looking-Glass")?;
    library.credits.push(glass, carroll, "wrote")?;
    library.credits.push(glass, tenniel, "illustrated")?;
    let looking = library.chapters.push(glass, 1, "Looking-Glass House")?;
    library.paragraphs.push(
        looking,
        1,
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
    println!("a library in one directory: a shard per table per writer, and a local cache");
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
        bytes_in(&path)?
    );
    println!("     generation 1 was the empty file these columns were committed into");

    let shelf = Library::open(&path)?;
    assert_eq!(resident(&shelf, "book.title.bytes"), 0);
    assert_eq!(resident(&shelf, "paragraph.text.bytes"), 0);
    assert_eq!(resident(&shelf, dbsync::schema::CLOCK), 1);
    println!("\n3. reopened it with merge metadata loaded and text payloads cold");

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
    let alice_of = alice.id()?;
    assert_eq!(alice.chapters()?.count(), 2);
    assert_eq!(resident(&shelf, "chapter.book"), 0);
    assert!(resident(&shelf, "chapter.book.buckets") > 0);
    println!("     walking uses the parent index and never the column it indexes");
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
    println!("     author.name stayed cold -- naming rows and reading");
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
    let repeated = again.credits.push(glass, carroll, "wrote again")?;
    println!("\n7. another credit with the same parents receives its own UUID: {repeated}");
    assert_eq!(Table::rows(&again.credits), 7);
    again.verify()?;

    let before = bytes_in(&path)?;
    let cache = Layout::within(&path).cache(1);
    let untouched = ColumnFile::open(&cache)?
        .column_meta("paragraph.text")
        .expect("paragraph.text is in the file")
        .chunks[0];

    let eco = library.authors.push("Umberto Eco")?;
    let rosa = library.books.push("Il nome della rosa")?;
    library.credits.push(rosa, eco, "wrote")?;
    library.at_mut(alice_of)?.set_title("Alice in Wonderland")?;
    let published = library.commit(this_device())?;
    assert_eq!(library.at(alice_of)?.title()?, "Alice in Wonderland");
    assert_eq!(library.at(alice_of)?.authors()?.count(), 2);

    assert_eq!(
        ColumnFile::open(&cache)?
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
        bytes_in(&path)? - before
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
    stale
        .at_mut(alice_of)?
        .set_title("Alice from an older snapshot")?;
    let reconciled = stale.commit(this_device())?;
    assert_eq!(stale.all::<Book>().count(), 5);
    println!("\n10. committing the older handle automatically incorporated the newer generation");
    library = stale;

    assert_eq!(library.generation(), Some(reconciled));
    println!(
        "     every writer appends only to shards named for it, so two handles never \
         overwrite each other: the older one read what it had missed and added its own"
    );

    let annexe = WriterId::from_device_id("the annexe");
    let elsewhere: PathBuf =
        std::env::temp_dir().join(format!("dbsync-annexe-{}.dbs", std::process::id()));
    copy_store(&path, &elsewhere)?;
    let mut theirs = Library::open(&elsewhere)?;

    let calvino = theirs.authors.push("Italo Calvino")?;
    let cities = theirs.books.push("Le citta invisibili")?;
    theirs.credits.push(cities, calvino, "wrote")?;
    theirs.at_mut(alice_of)?.set_title("Alice, annotated")?;
    let alice_chapters = theirs.children_of::<Book, Chapter>(alice_of)?;
    let (rabbit_of, pool_of) = (alice_chapters[0], alice_chapters[1]);
    theirs.chapters.set_number(pool_of, 0)?;
    theirs.commit(annexe)?;

    let woolf = library.authors.push("Virginia Woolf")?;
    let waves = library.books.push("The Waves")?;
    library.credits.push(waves, woolf, "wrote")?;
    library.at_mut(alice_of)?.set_title("Alice Underground")?;
    library
        .chapters
        .set_title(rabbit_of, "Down the Rabbit Hole")?;

    println!("\n11. someone copied the file to the annexe and both shelves were worked on");
    println!("     here: added The Waves, retitled Alice and her first chapter");
    println!("     there: added Le citta invisibili, retitled Alice as well, and numbered");
    println!("     The Pool of Tears 0 so it reads before Down the Rabbit-Hole");
    println!(
        "     neither file knows about the other: {} books here, {} there",
        library.all::<Book>().count(),
        theirs.all::<Book>().count()
    );

    let report = library.merge_from(&elsewhere, this_device())?;
    library.verify()?;

    println!("\n12. merged the annexe back in");
    println!(
        "     {} rows came across, {} were already known here",
        report.taken, report.known
    );
    assert_eq!(library.all::<Book>().count(), 7);
    println!(
        "     {} books now, by {} people",
        library.all::<Book>().count(),
        library.all::<Author>().count()
    );

    assert_eq!(report.concurrent.len(), 1);
    let said = report.concurrent[0];
    println!(
        "     one row was written on both shelves without either knowing: {} {}",
        said.entity, said.id
    );
    println!("     the later write kept it and the earlier one is still there to read:");
    println!("       it says {:?}", library.at(alice_of)?.title()?);
    println!("     nobody chose that by hand; the two clocks did, and they would have");
    println!("     said the same thing had the annexe merged this shelf instead");

    let order: Vec<String> = library
        .at(alice_of)?
        .chapters()?
        .map(|chapter| Ok(chapter.title()?.into_owned()))
        .collect::<Result<_>>()?;
    assert_eq!(order, ["The Pool of Tears", "Down the Rabbit Hole"]);
    println!("     her chapters now read {order:?}: the number chosen there and the");
    println!("     title chosen here both arrived, and neither edit moved a chapter out of");
    println!("     the place its number gives it; a copy that merged the other way reads");
    println!("     them in the same order, because order is a column, not an accident of");
    println!("     which row was written last");

    let again = library.merge_from(&elsewhere, this_device())?;
    assert_eq!(again.taken, 0);
    println!(
        "     merging the same shelf again takes nothing: {} rows, all of them known",
        again.taken
    );

    let saved = library.at(waves)?.version()?.dot();
    let credits = library.children_of::<Book, Credit>(waves)?;
    library.remove(waves)?;
    assert_eq!(library.all::<Book>().count(), 6);
    for credit in &credits {
        assert!(library.at(*credit).is_err());
        assert_eq!(library.at_hidden(*credit)?.id()?, *credit);
    }
    println!("\n13. removed The Waves and retained its hidden credits");

    library.restore(waves, saved)?;
    assert_eq!(library.all::<Book>().count(), 7);
    assert_eq!(library.children_of::<Book, Credit>(waves)?, credits);
    library.verify()?;
    println!("\n14. restored the book and its credits became visible again");
    println!(
        "     {} rows of book for {} books on the shelf",
        Table::rows(&library.books),
        library.all::<Book>().count()
    );

    let prince = library.books.find_by_title("Le Petit Prince")?[0];
    let written = library.at(prince)?.version()?.dot();
    let purged = library.purge(prince)?;
    library.commit(this_device())?;
    let reclaimed = library.reclaim_purged(this_device())?;
    assert!(library.is_purged(prince)?);
    assert!(library.restore(prince, written).is_err());
    assert!(library.books.find_by_title("Le Petit Prince")?.is_empty());
    assert_eq!(library.all::<Book>().count(), 6);
    assert_eq!(library.history(prince)?.len(), 1);
    library.verify()?;
    println!("\n15. purged Le Petit Prince, and unlike a removal that cannot be undone");
    println!("     {purged} rows went with it: the book, and the credit, chapters and paragraphs");
    println!("     that cannot stand without it; its author stayed");
    println!(
        "     reclaiming then dropped the {} revisions this device wrote for them from",
        reclaimed.dropped
    );
    println!("     its shards and rebuilt the cache, so the text is gone from the directory");
    println!("     and each row is left one empty revision that says it was purged");

    std::fs::remove_dir_all(&path)?;
    std::fs::remove_dir_all(&elsewhere)?;
    Ok(())
}
