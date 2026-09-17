use crate::error::Result;
use crate::id::Id;
use crate::schema::{
    AnyTable, Child, Entity, Field, FieldMut, RefColumn, Store, StoreState, Table, TextColumn,
};

pub struct Book;
pub struct Chapter;
pub struct Paragraph;

impl Entity for Book {
    const NAME: &'static str = "book";

    type Table = Books;
}

impl Entity for Chapter {
    const NAME: &'static str = "chapter";

    type Table = Chapters;
}

impl Entity for Paragraph {
    const NAME: &'static str = "paragraph";

    type Table = Paragraphs;
}

#[derive(Default, Clone)]
pub struct Books {
    title: TextColumn,
    author: TextColumn,
}

impl Table for Books {
    type Of = Book;

    fn fields(&self) -> Vec<Field<'_>> {
        vec![
            Field::new("title", &self.title),
            Field::new("author", &self.author),
        ]
    }

    fn fields_mut(&mut self) -> Vec<FieldMut<'_>> {
        vec![
            FieldMut::new("title", &mut self.title),
            FieldMut::new("author", &mut self.author),
        ]
    }
}

impl Books {
    pub fn push(&mut self, title: &str, author: &str) -> Result<Id<Book>> {
        self.reserve()?;
        let row = self.title.push(title)?;
        self.author.push(author)?;
        Ok(Id::from_index(row))
    }

    pub fn title(&self, book: Id<Book>) -> Result<String> {
        Ok(self.title.get(self.row_of(book)?)?.into_owned())
    }

    pub fn author(&self, book: Id<Book>) -> Result<String> {
        Ok(self.author.get(self.row_of(book)?)?.into_owned())
    }

    pub fn set_title(&mut self, book: Id<Book>, title: &str) -> Result<()> {
        let row = self.row_of(book)?;
        self.title.set(row, title)
    }
}

#[derive(Default, Clone)]
pub struct Chapters {
    book: RefColumn<Book, Chapter>,
    title: TextColumn,
}

impl Table for Chapters {
    type Of = Chapter;

    fn fields(&self) -> Vec<Field<'_>> {
        vec![
            Field::new("book", &self.book),
            Field::new("title", &self.title),
        ]
    }

    fn fields_mut(&mut self) -> Vec<FieldMut<'_>> {
        vec![
            FieldMut::new("book", &mut self.book),
            FieldMut::new("title", &mut self.title),
        ]
    }
}

impl Child<Book> for Chapter {
    fn parents(table: &Chapters) -> &RefColumn<Book, Chapter> {
        &table.book
    }
}

impl Chapters {
    pub fn push(&mut self, book: Id<Book>, title: &str) -> Result<Id<Chapter>> {
        self.reserve()?;
        let row = self.book.push(book)?;
        self.title.push(title)?;
        Ok(Id::from_index(row))
    }

    pub fn title(&self, chapter: Id<Chapter>) -> Result<String> {
        Ok(self.title.get(self.row_of(chapter)?)?.into_owned())
    }

    pub fn book(&self, chapter: Id<Chapter>) -> Result<Id<Book>> {
        self.book.get(self.row_of(chapter)?)
    }

    pub fn of_book(&self, books: usize, book: Id<Book>) -> Result<&[Id<Chapter>]> {
        self.book.children_of(books, book)
    }

    pub fn is_indexed(&self) -> bool {
        self.book.is_indexed()
    }
}

#[derive(Default, Clone)]
pub struct Paragraphs {
    chapter: RefColumn<Chapter, Paragraph>,
    text: TextColumn,
}

impl Table for Paragraphs {
    type Of = Paragraph;

    fn fields(&self) -> Vec<Field<'_>> {
        vec![
            Field::new("chapter", &self.chapter),
            Field::new("text", &self.text),
        ]
    }

    fn fields_mut(&mut self) -> Vec<FieldMut<'_>> {
        vec![
            FieldMut::new("chapter", &mut self.chapter),
            FieldMut::new("text", &mut self.text),
        ]
    }
}

impl Child<Chapter> for Paragraph {
    fn parents(table: &Paragraphs) -> &RefColumn<Chapter, Paragraph> {
        &table.chapter
    }
}

impl Paragraphs {
    pub fn push(&mut self, chapter: Id<Chapter>, text: &str) -> Result<Id<Paragraph>> {
        self.reserve()?;
        let row = self.chapter.push(chapter)?;
        self.text.push(text)?;
        Ok(Id::from_index(row))
    }

    pub fn text(&self, paragraph: Id<Paragraph>) -> Result<String> {
        Ok(self.text.get(self.row_of(paragraph)?)?.into_owned())
    }

    pub fn of_chapter(&self, chapters: usize, chapter: Id<Chapter>) -> Result<&[Id<Paragraph>]> {
        self.chapter.children_of(chapters, chapter)
    }
}

#[derive(Default, Clone)]
pub struct Library {
    state: StoreState,
    pub books: Books,
    pub chapters: Chapters,
    pub paragraphs: Paragraphs,
}

impl Store for Library {
    fn state(&self) -> &StoreState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut StoreState {
        &mut self.state
    }

    fn tables(&self) -> Vec<&dyn AnyTable> {
        vec![&self.books, &self.chapters, &self.paragraphs]
    }

    fn tables_mut(&mut self) -> Vec<&mut dyn AnyTable> {
        vec![&mut self.books, &mut self.chapters, &mut self.paragraphs]
    }
}

impl Library {
    pub fn chapters_of(&self, book: Id<Book>) -> Result<&[Id<Chapter>]> {
        self.chapters.of_book(Table::rows(&self.books), book)
    }

    pub fn paragraphs_of(&self, chapter: Id<Chapter>) -> Result<&[Id<Paragraph>]> {
        self.paragraphs
            .of_chapter(Table::rows(&self.chapters), chapter)
    }
}
