//! A library of books, chapters and paragraphs, written to one column file and read back.

use std::path::{Path, PathBuf};

use dbsync::{
    Arena, Blob, ColumnFile, ColumnFileUpdater, ColumnFileWriter, Error, Id, Result, WriterId,
};

struct Book;
struct Chapter;
struct Paragraph;

struct Library {
    bytes: Arena<u8>,
    book_title: Arena<Blob>,
    book_author: Arena<Blob>,
    chapter_book: Arena<Id<Book>>,
    chapter_title: Arena<Blob>,
    paragraph_chapter: Arena<Id<Chapter>>,
    paragraph_text: Arena<Blob>,
}

impl Library {
    fn new() -> Self {
        Self {
            bytes: Arena::with_chunk_rows(64),
            book_title: Arena::with_chunk_rows(4),
            book_author: Arena::with_chunk_rows(4),
            chapter_book: Arena::with_chunk_rows(4),
            chapter_title: Arena::with_chunk_rows(4),
            paragraph_chapter: Arena::with_chunk_rows(4),
            paragraph_text: Arena::with_chunk_rows(4),
        }
    }

    fn shelve(&mut self, title: &str, author: &str) -> Result<Id<Book>> {
        let title = self.bytes.push_str(title)?;
        let author = self.bytes.push_str(author)?;
        let row = self.book_title.push(title)?;
        self.book_author.push(author)?;
        Ok(Id::from_index(row))
    }

    fn add_chapter(&mut self, book: Id<Book>, title: &str) -> Result<Id<Chapter>> {
        let title = self.bytes.push_str(title)?;
        let row = self.chapter_book.push(book)?;
        self.chapter_title.push(title)?;
        Ok(Id::from_index(row))
    }

    fn add_paragraph(&mut self, chapter: Id<Chapter>, text: &str) -> Result<Id<Paragraph>> {
        let text = self.bytes.push_str(text)?;
        let row = self.paragraph_chapter.push(chapter)?;
        self.paragraph_text.push(text)?;
        Ok(Id::from_index(row))
    }

    fn catalog(&self) -> Result<Vec<(Id<Book>, String, String)>> {
        let mut shelf = Vec::new();
        for row in 0..self.book_title.len() {
            let book = Id::from_index(row);
            shelf.push((book, self.title_of(book)?, self.author_of(book)?));
        }
        Ok(shelf)
    }

    fn chapters_of(&self, book: Id<Book>) -> Result<Vec<Id<Chapter>>> {
        let mut found = Vec::new();
        for (row, owner) in self.chapter_book.iter().enumerate() {
            if *owner? == book {
                found.push(Id::from_index(row));
            }
        }
        Ok(found)
    }

    fn paragraphs_of(&self, chapter: Id<Chapter>) -> Result<Vec<Id<Paragraph>>> {
        let mut found = Vec::new();
        for (row, owner) in self.paragraph_chapter.iter().enumerate() {
            if *owner? == chapter {
                found.push(Id::from_index(row));
            }
        }
        Ok(found)
    }

    fn title_of(&self, book: Id<Book>) -> Result<String> {
        self.text(*self.book_title.row(book.index())?)
    }

    fn author_of(&self, book: Id<Book>) -> Result<String> {
        self.text(*self.book_author.row(book.index())?)
    }

    fn chapter_title_of(&self, chapter: Id<Chapter>) -> Result<String> {
        self.text(*self.chapter_title.row(chapter.index())?)
    }

    fn text_of(&self, paragraph: Id<Paragraph>) -> Result<String> {
        self.text(*self.paragraph_text.row(paragraph.index())?)
    }

    fn text(&self, blob: Blob) -> Result<String> {
        Ok(self.bytes.text(blob)?.into_owned())
    }

    fn write(&self, path: &Path, writer_id: WriterId) -> Result<()> {
        let mut writer = ColumnFileWriter::create(path, writer_id)?;
        writer.column("bytes", &self.bytes)?;
        writer.column("book.title", &self.book_title)?;
        writer.column("book.author", &self.book_author)?;
        writer.column("chapter.book", &self.chapter_book)?;
        writer.column("chapter.title", &self.chapter_title)?;
        writer.column("paragraph.chapter", &self.paragraph_chapter)?;
        writer.column("paragraph.text", &self.paragraph_text)?;
        writer.finish()?;
        Ok(())
    }

    fn read(file: &ColumnFile) -> Result<Self> {
        Ok(Self {
            bytes: file.column("bytes")?,
            book_title: file.column("book.title")?,
            book_author: file.column("book.author")?,
            chapter_book: file.column("chapter.book")?,
            chapter_title: file.column("chapter.title")?,
            paragraph_chapter: file.column("paragraph.chapter")?,
            paragraph_text: file.column("paragraph.text")?,
        })
    }

    fn offer_to(&self, updater: &mut ColumnFileUpdater) -> Result<()> {
        updater.column("bytes", &self.bytes)?;
        updater.column("book.title", &self.book_title)?;
        updater.column("book.author", &self.book_author)?;
        updater.column("chapter.book", &self.chapter_book)?;
        updater.column("chapter.title", &self.chapter_title)?;
        updater.column("paragraph.chapter", &self.paragraph_chapter)?;
        updater.column("paragraph.text", &self.paragraph_text)?;
        Ok(())
    }

    fn resident_chunks(&self) -> usize {
        self.bytes.resident_chunks()
            + self.book_title.resident_chunks()
            + self.book_author.resident_chunks()
            + self.chapter_book.resident_chunks()
            + self.chapter_title.resident_chunks()
            + self.paragraph_chapter.resident_chunks()
            + self.paragraph_text.resident_chunks()
    }

    fn prefetch(&self) -> Result<()> {
        self.bytes.prefetch()?;
        self.book_title.prefetch()?;
        self.book_author.prefetch()?;
        self.chapter_book.prefetch()?;
        self.chapter_title.prefetch()?;
        self.paragraph_chapter.prefetch()?;
        self.paragraph_text.prefetch()
    }
}

fn this_device() -> WriterId {
    WriterId::from_device_id("KRZH3XY-7PH4NJK-2WQ5LMC")
}

fn book_called(library: &Library, title: &str) -> Result<Id<Book>> {
    for (book, shelved, _) in library.catalog()? {
        if shelved == title {
            return Ok(book);
        }
    }
    Err(Error::invalid(format!("no book called {title:?}")))
}

fn stock(library: &mut Library) -> Result<()> {
    let prince = library.shelve("Le Petit Prince", "Antoine de Saint-Exupéry")?;
    let opening = library.add_chapter(prince, "Chapitre I")?;
    library.add_paragraph(opening, "Lorsque j'avais six ans j'ai vu une image.")?;
    library.add_paragraph(opening, "On disait: les serpents avalent leur proie.")?;
    let desert = library.add_chapter(prince, "Chapitre II")?;
    library.add_paragraph(desert, "J'ai vécu seul, sans personne à qui parler.")?;

    let kokoro = library.shelve("こころ", "夏目漱石")?;
    let sensei = library.add_chapter(kokoro, "上 先生と私")?;
    library.add_paragraph(sensei, "私はその人を常に先生と呼んでいた。")?;
    library.add_paragraph(sensei, "筆を執っても心持は同じ事である。")?;
    let parents = library.add_chapter(kokoro, "中 両親と私")?;
    library.add_paragraph(parents, "卒業してから約二ヶ月の後であった。")?;

    let alice = library.shelve("Alice's Adventures in Wonderland", "Lewis Carroll")?;
    let rabbit = library.add_chapter(alice, "Down the Rabbit-Hole")?;
    library.add_paragraph(rabbit, "Alice was beginning to get very tired.")?;
    library.add_paragraph(rabbit, "So she was considering in her own mind.")?;
    let pool = library.add_chapter(alice, "The Pool of Tears")?;
    library.add_paragraph(pool, "Curiouser and curiouser! cried Alice.")?;

    Ok(())
}

fn main() -> Result<()> {
    let path: PathBuf =
        std::env::temp_dir().join(format!("dbsync-library-{}.dbs", std::process::id()));
    println!("a library in one column file");
    println!("  {}", path.display());

    let mut library = Library::new();
    stock(&mut library)?;

    let kokoro = book_called(&library, "こころ")?;
    let title = *library.book_title.row(kokoro.index())?;
    println!("\n1. shelved the books");
    println!(
        "     {} books, {} chapters, {} paragraphs, {} bytes of text",
        library.book_title.len(),
        library.chapter_book.len(),
        library.paragraph_chapter.len(),
        library.bytes.len()
    );
    println!(
        "     {:?} is {} bytes in the heap but {} characters: blobs measure bytes",
        library.title_of(kokoro)?,
        title.len,
        library.title_of(kokoro)?.chars().count()
    );
    println!("     chunks here hold 4 rows so laziness is visible at this size;");
    println!("     a real library would take default_chunk_rows(T::SIZE), about 64 KiB");

    library.write(&path, this_device())?;
    let file = ColumnFile::open(&path)?;
    file.verify()?;
    assert_eq!(file.generation(), 1);
    assert_eq!(file.columns().len(), 7);
    println!("\n2. wrote generation 1 and verified every checksum");
    println!(
        "     {} columns in {} bytes, of which the first 16 KiB is header and commit slots",
        file.columns().len(),
        std::fs::metadata(&path)?.len()
    );

    let shelf = Library::read(&file)?;
    assert_eq!(shelf.resident_chunks(), 0);
    println!("\n3. reopened it, and opening a column read no payload at all");
    println!(
        "     resident chunks across all seven columns: {}",
        shelf.resident_chunks()
    );

    for (_, title, author) in shelf.catalog()? {
        println!("     {title} — {author}");
    }
    assert!(shelf.book_title.resident_chunks() > 0);
    assert_eq!(shelf.paragraph_text.resident_chunks(), 0);
    println!(
        "     listing the catalogue left paragraph.text cold: {} of {} chunks read",
        shelf.paragraph_text.resident_chunks(),
        shelf.paragraph_text.chunk_count()
    );
    println!(
        "     it did read {} of the heap's {} chunks, because titles and paragraphs",
        shelf.bytes.resident_chunks(),
        shelf.bytes.chunk_count()
    );
    println!("     share one Arena<u8>: a blob pulls in whatever chunk it lands on");

    println!("\n4. opened one book, which is a scan of the owner column, not an index");
    let alice = book_called(&shelf, "Alice's Adventures in Wonderland")?;
    for chapter in shelf.chapters_of(alice)? {
        println!("     {}", shelf.chapter_title_of(chapter)?);
        for paragraph in shelf.paragraphs_of(chapter)? {
            println!("       {}", shelf.text_of(paragraph)?);
        }
    }
    assert!(shelf.bytes.resident_chunks() < shelf.bytes.chunk_count());
    println!(
        "     the heap is still only partly resident: {} of {} chunks",
        shelf.bytes.resident_chunks(),
        shelf.bytes.chunk_count()
    );

    let before = std::fs::metadata(&path)?.len();
    let mut acquired = Library::read(&file)?;
    let eco = acquired.shelve("Il nome della rosa", "Umberto Eco")?;
    let first = acquired.add_chapter(eco, "Primo giorno")?;
    acquired.add_paragraph(first, "Era una bella mattina di fine novembre.")?;

    let mut updater = file.update(this_device())?;
    acquired.offer_to(&mut updater)?;
    assert_eq!(updater.commit()?, 2);
    let grown = std::fs::metadata(&path)?.len() - before;
    let current = ColumnFile::open(&path)?;
    assert_eq!(
        file.column_meta("paragraph.text").unwrap().chunks[0],
        current.column_meta("paragraph.text").unwrap().chunks[0]
    );
    println!("\n5. acquired a fourth book and committed generation 2");
    println!("     the file grew by {grown} bytes: the chunks that changed, and a new directory");
    println!("     every chunk it did not touch kept its offset and was never rewritten");

    shelf.prefetch()?;
    assert_eq!(shelf.catalog()?.len(), 3);
    let reopened = Library::read(&current)?;
    assert_eq!(reopened.catalog()?.len(), 4);
    println!("\n6. the handle opened at generation 1 still serves generation 1");
    println!(
        "     it reads {} books from the same file a fresh open reads {} from",
        shelf.catalog()?.len(),
        reopened.catalog()?.len()
    );

    let refused = file.update(this_device()).unwrap_err();
    assert!(refused.to_string().contains("moved to generation 2"));
    println!("\n7. and that handle can no longer commit over the newer generation");
    println!("     {refused}");
    println!("     the caller reopens and retries; nothing was written");

    let token = ColumnFile::read_commit_token(&path)?;
    assert_eq!(token.generation, 2);
    assert!(!token.is_echo_of(this_device(), 1));
    println!(
        "     a peer polls read_commit_token for that: generation {}, not an echo of its own write",
        token.generation
    );

    std::fs::remove_file(&path)?;
    Ok(())
}
