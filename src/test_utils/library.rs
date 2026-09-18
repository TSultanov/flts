use std::borrow::Cow;
use std::ops::{Deref, DerefMut};

use crate::error::{Error, Result};
use crate::id::Id;
use crate::schema::{
    Across, AnyTable, Audited, Child, Children, Distinct, Entity, Entry, EntryMut, EnumColumn,
    Field, FieldMut, Holds, NullableColumn, NullableTextColumn, Pair, RefColumn, Rows, Store,
    StoreState, Table, TextColumn, UnitEnum, audit_reference, check_pairs,
};
use crate::schema::{IdColumn, Indexed};
use crate::storage::Arena;

pub struct Book;
pub struct Chapter;
pub struct Paragraph;
pub struct Edition;
pub struct Author;
pub struct Authorship;

impl Entity for Book {
    const NAME: &'static str = "book";

    type Table = Books;

    type Row<'a, S>
        = BookRow<'a, S>
    where
        S: Holds<Book> + 'a;

    type RowMut<'a, S>
        = BookRowMut<'a, S>
    where
        S: Holds<Book> + 'a;
}

impl Entity for Chapter {
    const NAME: &'static str = "chapter";

    type Table = Chapters;

    type Row<'a, S>
        = ChapterRow<'a, S>
    where
        S: Holds<Chapter> + 'a;

    type RowMut<'a, S>
        = ChapterRowMut<'a, S>
    where
        S: Holds<Chapter> + 'a;
}

impl Entity for Paragraph {
    const NAME: &'static str = "paragraph";

    type Table = Paragraphs;

    type Row<'a, S>
        = ParagraphRow<'a, S>
    where
        S: Holds<Paragraph> + 'a;

    type RowMut<'a, S>
        = ParagraphRowMut<'a, S>
    where
        S: Holds<Paragraph> + 'a;
}

impl Entity for Edition {
    const NAME: &'static str = "edition";

    type Table = Editions;

    type Row<'a, S>
        = EditionRow<'a, S>
    where
        S: Holds<Edition> + 'a;

    type RowMut<'a, S>
        = EditionRowMut<'a, S>
    where
        S: Holds<Edition> + 'a;
}

impl Entity for Author {
    const NAME: &'static str = "author";

    type Table = Authors;

    type Row<'a, S>
        = AuthorRow<'a, S>
    where
        S: Holds<Author> + 'a;

    type RowMut<'a, S>
        = AuthorRowMut<'a, S>
    where
        S: Holds<Author> + 'a;
}

impl Entity for Authorship {
    const NAME: &'static str = "authorship";

    type Table = Authorships;

    type Row<'a, S>
        = AuthorshipRow<'a, S>
    where
        S: Holds<Authorship> + 'a;

    type RowMut<'a, S>
        = AuthorshipRowMut<'a, S>
    where
        S: Holds<Authorship> + 'a;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Binding {
    Cloth,
    Paper,
    Leather,
}

impl UnitEnum for Binding {
    const NAME: &'static str = "binding";

    fn to_u8(self) -> u8 {
        match self {
            Binding::Cloth => 0,
            Binding::Paper => 1,
            Binding::Leather => 2,
        }
    }

    fn from_u8(code: u8) -> Result<Self> {
        match code {
            0 => Ok(Binding::Cloth),
            1 => Ok(Binding::Paper),
            2 => Ok(Binding::Leather),
            other => Err(Error::unsupported(format!("unknown binding {other}"))),
        }
    }
}

#[derive(Clone)]
pub struct Books {
    ids: IdColumn<Book>,
    title: Indexed<TextColumn, false>,
    author: TextColumn,
}

impl Default for Books {
    fn default() -> Self {
        Self {
            ids: IdColumn::default(),
            title: Indexed::named("book.title"),
            author: TextColumn::default(),
        }
    }
}

impl Table for Books {
    type Of = Book;

    fn ids(&self) -> &IdColumn<Book> {
        &self.ids
    }

    fn ids_mut(&mut self) -> &mut IdColumn<Book> {
        &mut self.ids
    }

    fn fields(&self) -> Vec<Field<'_>> {
        vec![
            Field::new("id", &self.ids),
            Field::new("title", &self.title),
            Field::new("author", &self.author),
        ]
    }

    fn fields_mut(&mut self) -> Vec<FieldMut<'_>> {
        vec![
            FieldMut::new("id", &mut self.ids),
            FieldMut::new("title", &mut self.title),
            FieldMut::new("author", &mut self.author),
        ]
    }
}

impl Books {
    pub fn push(&mut self, title: &str, author: &str) -> Result<Id<Book>> {
        self.reserve()?;
        self.title.reach(title)?;
        let id = self.mint()?;
        self.title.push(title)?;
        self.author.push(author)?;
        Ok(id)
    }

    pub fn title(&self, book: Id<Book>) -> Result<String> {
        Ok(self.title.column().get(self.row_of(book)?)?.into_owned())
    }

    pub fn find_by_title(&self, title: &str) -> Result<Vec<Id<Book>>> {
        self.title
            .find(title)?
            .into_iter()
            .map(|row| self.id_at(row))
            .collect()
    }

    pub fn author(&self, book: Id<Book>) -> Result<String> {
        Ok(self.author.get(self.row_of(book)?)?.into_owned())
    }

    pub fn set_title(&mut self, book: Id<Book>, title: &str) -> Result<()> {
        let row = self.row_of(book)?;
        self.title.set(row, title)
    }
}

pub struct BookRow<'a, S: Holds<Book>>(Entry<'a, S, Book>);

impl<'a, S: Holds<Book>> From<Entry<'a, S, Book>> for BookRow<'a, S> {
    fn from(entry: Entry<'a, S, Book>) -> Self {
        Self(entry)
    }
}

impl<'a, S: Holds<Book>> Deref for BookRow<'a, S> {
    type Target = Entry<'a, S, Book>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, S: Holds<Book>> BookRow<'a, S> {
    pub fn title(&self) -> Result<Cow<'a, str>> {
        self.0.table().title.column().get(self.0.row())
    }

    pub fn author(&self) -> Result<Cow<'a, str>> {
        self.0.table().author.get(self.0.row())
    }

    pub fn chapters(&self) -> Result<Children<'a, S, Chapter>>
    where
        S: Holds<Chapter>,
    {
        self.0.children::<Chapter>()
    }

    pub fn editions(&self) -> Result<Children<'a, S, Edition>>
    where
        S: Holds<Edition>,
    {
        self.0.children::<Edition>()
    }

    pub fn authorships(&self) -> Result<Children<'a, S, Authorship>>
    where
        S: Holds<Authorship>,
    {
        self.0.children::<Authorship>()
    }

    pub fn authors(&self) -> Result<Across<'a, S, Author>>
    where
        S: Holds<Authorship> + Holds<Author>,
    {
        self.0.across::<Authorship, Author>()
    }
}

pub struct BookRowMut<'a, S: Holds<Book>>(EntryMut<'a, S, Book>);

impl<'a, S: Holds<Book>> From<EntryMut<'a, S, Book>> for BookRowMut<'a, S> {
    fn from(entry: EntryMut<'a, S, Book>) -> Self {
        Self(entry)
    }
}

impl<'a, S: Holds<Book>> Deref for BookRowMut<'a, S> {
    type Target = EntryMut<'a, S, Book>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, S: Holds<Book>> DerefMut for BookRowMut<'a, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<S: Holds<Book>> BookRowMut<'_, S> {
    pub fn set_title(&mut self, value: &str) -> Result<()> {
        let row = self.0.row();
        let table = self.0.table_mut();
        table.title.set(row, value)
    }
}

#[derive(Clone)]
pub struct Chapters {
    ids: IdColumn<Chapter>,
    book: RefColumn<Book, Chapter>,
    title: TextColumn,
}

impl Default for Chapters {
    fn default() -> Self {
        Self {
            ids: IdColumn::default(),
            book: RefColumn::named("chapter.book"),
            title: TextColumn::default(),
        }
    }
}

impl Table for Chapters {
    type Of = Chapter;

    fn ids(&self) -> &IdColumn<Chapter> {
        &self.ids
    }

    fn ids_mut(&mut self) -> &mut IdColumn<Chapter> {
        &mut self.ids
    }

    fn fields(&self) -> Vec<Field<'_>> {
        vec![
            Field::new("id", &self.ids),
            Field::new("book", &self.book),
            Field::new("title", &self.title),
        ]
    }

    fn fields_mut(&mut self) -> Vec<FieldMut<'_>> {
        vec![
            FieldMut::new("id", &mut self.ids),
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
        self.book.reach(book)?;
        let id = self.mint()?;
        self.book.push(book)?;
        self.title.push(title)?;
        Ok(id)
    }

    pub fn title(&self, chapter: Id<Chapter>) -> Result<String> {
        Ok(self.title.get(self.row_of(chapter)?)?.into_owned())
    }

    pub fn book(&self, chapter: Id<Chapter>) -> Result<Id<Book>> {
        self.book.get(self.row_of(chapter)?)
    }

    pub fn of_book(&self, book: Id<Book>) -> Result<Vec<Id<Chapter>>> {
        self.book.children_of(&self.ids, book)
    }
}

pub struct ChapterRow<'a, S: Holds<Chapter>>(Entry<'a, S, Chapter>);

impl<'a, S: Holds<Chapter>> From<Entry<'a, S, Chapter>> for ChapterRow<'a, S> {
    fn from(entry: Entry<'a, S, Chapter>) -> Self {
        Self(entry)
    }
}

impl<'a, S: Holds<Chapter>> Deref for ChapterRow<'a, S> {
    type Target = Entry<'a, S, Chapter>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, S: Holds<Chapter>> ChapterRow<'a, S> {
    pub fn title(&self) -> Result<Cow<'a, str>> {
        self.0.table().title.get(self.0.row())
    }

    pub fn book(&self) -> Result<<Book as Entity>::Row<'a, S>>
    where
        S: Holds<Book>,
    {
        let table = self.0.table();
        let row = self.0.row();
        self.0.at(table.book.get(row)?)
    }

    pub fn paragraphs(&self) -> Result<Children<'a, S, Paragraph>>
    where
        S: Holds<Paragraph>,
    {
        self.0.children::<Paragraph>()
    }
}

pub struct ChapterRowMut<'a, S: Holds<Chapter>>(EntryMut<'a, S, Chapter>);

impl<'a, S: Holds<Chapter>> From<EntryMut<'a, S, Chapter>> for ChapterRowMut<'a, S> {
    fn from(entry: EntryMut<'a, S, Chapter>) -> Self {
        Self(entry)
    }
}

impl<'a, S: Holds<Chapter>> Deref for ChapterRowMut<'a, S> {
    type Target = EntryMut<'a, S, Chapter>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, S: Holds<Chapter>> DerefMut for ChapterRowMut<'a, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<S: Holds<Chapter>> ChapterRowMut<'_, S> {
    pub fn set_book(&mut self, value: Id<Book>) -> Result<()> {
        let row = self.0.row();
        let table = self.0.table_mut();
        table.book.set(row, value)
    }
}

#[derive(Clone)]
pub struct Paragraphs {
    ids: IdColumn<Paragraph>,
    chapter: RefColumn<Chapter, Paragraph>,
    text: TextColumn,
}

impl Default for Paragraphs {
    fn default() -> Self {
        Self {
            ids: IdColumn::default(),
            chapter: RefColumn::named("paragraph.chapter"),
            text: TextColumn::default(),
        }
    }
}

impl Table for Paragraphs {
    type Of = Paragraph;

    fn ids(&self) -> &IdColumn<Paragraph> {
        &self.ids
    }

    fn ids_mut(&mut self) -> &mut IdColumn<Paragraph> {
        &mut self.ids
    }

    fn fields(&self) -> Vec<Field<'_>> {
        vec![
            Field::new("id", &self.ids),
            Field::new("chapter", &self.chapter),
            Field::new("text", &self.text),
        ]
    }

    fn fields_mut(&mut self) -> Vec<FieldMut<'_>> {
        vec![
            FieldMut::new("id", &mut self.ids),
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
        self.chapter.reach(chapter)?;
        let id = self.mint()?;
        self.chapter.push(chapter)?;
        self.text.push(text)?;
        Ok(id)
    }

    pub fn text(&self, paragraph: Id<Paragraph>) -> Result<String> {
        Ok(self.text.get(self.row_of(paragraph)?)?.into_owned())
    }

    pub fn of_chapter(&self, chapter: Id<Chapter>) -> Result<Vec<Id<Paragraph>>> {
        self.chapter.children_of(&self.ids, chapter)
    }
}

pub struct ParagraphRow<'a, S: Holds<Paragraph>>(Entry<'a, S, Paragraph>);

impl<'a, S: Holds<Paragraph>> From<Entry<'a, S, Paragraph>> for ParagraphRow<'a, S> {
    fn from(entry: Entry<'a, S, Paragraph>) -> Self {
        Self(entry)
    }
}

impl<'a, S: Holds<Paragraph>> Deref for ParagraphRow<'a, S> {
    type Target = Entry<'a, S, Paragraph>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, S: Holds<Paragraph>> ParagraphRow<'a, S> {
    pub fn text(&self) -> Result<Cow<'a, str>> {
        self.0.table().text.get(self.0.row())
    }

    pub fn chapter(&self) -> Result<<Chapter as Entity>::Row<'a, S>>
    where
        S: Holds<Chapter>,
    {
        let table = self.0.table();
        let row = self.0.row();
        self.0.at(table.chapter.get(row)?)
    }
}

pub struct ParagraphRowMut<'a, S: Holds<Paragraph>>(EntryMut<'a, S, Paragraph>);

impl<'a, S: Holds<Paragraph>> From<EntryMut<'a, S, Paragraph>> for ParagraphRowMut<'a, S> {
    fn from(entry: EntryMut<'a, S, Paragraph>) -> Self {
        Self(entry)
    }
}

impl<'a, S: Holds<Paragraph>> Deref for ParagraphRowMut<'a, S> {
    type Target = EntryMut<'a, S, Paragraph>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, S: Holds<Paragraph>> DerefMut for ParagraphRowMut<'a, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<S: Holds<Paragraph>> ParagraphRowMut<'_, S> {
    pub fn set_text(&mut self, value: &str) -> Result<()> {
        let row = self.0.row();
        let table = self.0.table_mut();
        table.text.set(row, value)
    }
}

#[derive(Clone)]
pub struct Editions {
    ids: IdColumn<Edition>,
    book: RefColumn<Book, Edition>,
    isbn: NullableTextColumn,
    year: Arena<u32>,
    pages: NullableColumn<u32>,
    binding: EnumColumn<Binding>,
}

impl Default for Editions {
    fn default() -> Self {
        Self {
            ids: IdColumn::default(),
            book: RefColumn::named("edition.book"),
            isbn: NullableTextColumn::default(),
            year: Arena::new(),
            pages: NullableColumn::default(),
            binding: EnumColumn::default(),
        }
    }
}

impl Table for Editions {
    type Of = Edition;

    fn ids(&self) -> &IdColumn<Edition> {
        &self.ids
    }

    fn ids_mut(&mut self) -> &mut IdColumn<Edition> {
        &mut self.ids
    }

    fn fields(&self) -> Vec<Field<'_>> {
        vec![
            Field::new("id", &self.ids),
            Field::new("book", &self.book),
            Field::new("isbn", &self.isbn),
            Field::new("year", &self.year),
            Field::new("pages", &self.pages),
            Field::new("binding", &self.binding),
        ]
    }

    fn fields_mut(&mut self) -> Vec<FieldMut<'_>> {
        vec![
            FieldMut::new("id", &mut self.ids),
            FieldMut::new("book", &mut self.book),
            FieldMut::new("isbn", &mut self.isbn),
            FieldMut::new("year", &mut self.year),
            FieldMut::new("pages", &mut self.pages),
            FieldMut::new("binding", &mut self.binding),
        ]
    }
}

impl Child<Book> for Edition {
    fn parents(table: &Editions) -> &RefColumn<Book, Edition> {
        &table.book
    }
}

impl Editions {
    pub fn push(
        &mut self,
        book: Id<Book>,
        isbn: Option<&str>,
        year: u32,
        pages: Option<u32>,
        binding: Binding,
    ) -> Result<Id<Edition>> {
        self.reserve()?;
        self.book.reach(book)?;
        let id = self.mint()?;
        self.book.push(book)?;
        self.isbn.push(isbn)?;
        self.year.push(year)?;
        self.pages.push(pages)?;
        self.binding.push(binding)?;
        Ok(id)
    }

    pub fn book(&self, of: Id<Edition>) -> Result<Id<Book>> {
        self.book.get(self.row_of(of)?)
    }

    pub fn isbn(&self, of: Id<Edition>) -> Result<Option<Cow<'_, str>>> {
        self.isbn.get(self.row_of(of)?)
    }

    pub fn year(&self, of: Id<Edition>) -> Result<u32> {
        self.year.row(self.row_of(of)?.index()).copied()
    }

    pub fn pages(&self, of: Id<Edition>) -> Result<Option<u32>> {
        self.pages.get(self.row_of(of)?)
    }

    pub fn binding(&self, of: Id<Edition>) -> Result<Binding> {
        self.binding.get(self.row_of(of)?)
    }
}

pub struct EditionRow<'a, S: Holds<Edition>>(Entry<'a, S, Edition>);

impl<'a, S: Holds<Edition>> From<Entry<'a, S, Edition>> for EditionRow<'a, S> {
    fn from(entry: Entry<'a, S, Edition>) -> Self {
        Self(entry)
    }
}

impl<'a, S: Holds<Edition>> Deref for EditionRow<'a, S> {
    type Target = Entry<'a, S, Edition>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, S: Holds<Edition>> EditionRow<'a, S> {
    pub fn book(&self) -> Result<<Book as Entity>::Row<'a, S>>
    where
        S: Holds<Book>,
    {
        let table = self.0.table();
        let row = self.0.row();
        self.0.at(table.book.get(row)?)
    }

    pub fn isbn(&self) -> Result<Option<Cow<'a, str>>> {
        self.0.table().isbn.get(self.0.row())
    }

    pub fn year(&self) -> Result<u32> {
        self.0.table().year.row(self.0.row().index()).copied()
    }

    pub fn pages(&self) -> Result<Option<u32>> {
        self.0.table().pages.get(self.0.row())
    }

    pub fn binding(&self) -> Result<Binding> {
        self.0.table().binding.get(self.0.row())
    }
}

pub struct EditionRowMut<'a, S: Holds<Edition>>(EntryMut<'a, S, Edition>);

impl<'a, S: Holds<Edition>> From<EntryMut<'a, S, Edition>> for EditionRowMut<'a, S> {
    fn from(entry: EntryMut<'a, S, Edition>) -> Self {
        Self(entry)
    }
}

impl<'a, S: Holds<Edition>> Deref for EditionRowMut<'a, S> {
    type Target = EntryMut<'a, S, Edition>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, S: Holds<Edition>> DerefMut for EditionRowMut<'a, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<S: Holds<Edition>> EditionRowMut<'_, S> {
    pub fn set_book(&mut self, value: Id<Book>) -> Result<()> {
        let row = self.0.row();
        let table = self.0.table_mut();
        table.book.set(row, value)
    }

    pub fn set_isbn(&mut self, value: Option<&str>) -> Result<()> {
        let row = self.0.row();
        let table = self.0.table_mut();
        table.isbn.set(row, value)
    }

    pub fn set_year(&mut self, value: u32) -> Result<()> {
        let row = self.0.row();
        let table = self.0.table_mut();
        table.year.set(row.index(), value)
    }

    pub fn set_pages(&mut self, value: Option<u32>) -> Result<()> {
        let row = self.0.row();
        let table = self.0.table_mut();
        table.pages.set(row, value)
    }

    pub fn set_binding(&mut self, value: Binding) -> Result<()> {
        let row = self.0.row();
        let table = self.0.table_mut();
        table.binding.set(row, value)
    }
}

#[derive(Clone)]
pub struct Authors {
    ids: IdColumn<Author>,
    name: Indexed<TextColumn, false>,
}

impl Default for Authors {
    fn default() -> Self {
        Self {
            ids: IdColumn::default(),
            name: Indexed::named("author.name"),
        }
    }
}

impl Table for Authors {
    type Of = Author;

    fn ids(&self) -> &IdColumn<Author> {
        &self.ids
    }

    fn ids_mut(&mut self) -> &mut IdColumn<Author> {
        &mut self.ids
    }

    fn fields(&self) -> Vec<Field<'_>> {
        vec![Field::new("id", &self.ids), Field::new("name", &self.name)]
    }

    fn fields_mut(&mut self) -> Vec<FieldMut<'_>> {
        vec![
            FieldMut::new("id", &mut self.ids),
            FieldMut::new("name", &mut self.name),
        ]
    }
}

impl Authors {
    pub fn push(&mut self, name: &str) -> Result<Id<Author>> {
        self.reserve()?;
        self.name.reach(name)?;
        let id = self.mint()?;
        self.name.push(name)?;
        Ok(id)
    }

    pub fn name(&self, author: Id<Author>) -> Result<String> {
        Ok(self.name.column().get(self.row_of(author)?)?.into_owned())
    }

    pub fn find_by_name(&self, name: &str) -> Result<Vec<Id<Author>>> {
        self.name
            .find(name)?
            .into_iter()
            .map(|row| self.id_at(row))
            .collect()
    }
}

pub struct AuthorRow<'a, S: Holds<Author>>(Entry<'a, S, Author>);

impl<'a, S: Holds<Author>> From<Entry<'a, S, Author>> for AuthorRow<'a, S> {
    fn from(entry: Entry<'a, S, Author>) -> Self {
        Self(entry)
    }
}

impl<'a, S: Holds<Author>> Deref for AuthorRow<'a, S> {
    type Target = Entry<'a, S, Author>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, S: Holds<Author>> AuthorRow<'a, S> {
    pub fn name(&self) -> Result<Cow<'a, str>> {
        self.0.table().name.column().get(self.0.row())
    }

    pub fn authorships(&self) -> Result<Children<'a, S, Authorship>>
    where
        S: Holds<Authorship>,
    {
        self.0.children::<Authorship>()
    }

    pub fn books(&self) -> Result<Across<'a, S, Book>>
    where
        S: Holds<Authorship> + Holds<Book>,
    {
        self.0.across::<Authorship, Book>()
    }
}

pub struct AuthorRowMut<'a, S: Holds<Author>>(EntryMut<'a, S, Author>);

impl<'a, S: Holds<Author>> From<EntryMut<'a, S, Author>> for AuthorRowMut<'a, S> {
    fn from(entry: EntryMut<'a, S, Author>) -> Self {
        Self(entry)
    }
}

impl<'a, S: Holds<Author>> Deref for AuthorRowMut<'a, S> {
    type Target = EntryMut<'a, S, Author>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, S: Holds<Author>> DerefMut for AuthorRowMut<'a, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Clone)]
pub struct Authorships {
    ids: IdColumn<Authorship>,
    book: RefColumn<Book, Authorship>,
    author: RefColumn<Author, Authorship>,
    book_author: Distinct<Book, Author>,
}

impl Default for Authorships {
    fn default() -> Self {
        Self {
            ids: IdColumn::default(),
            book: RefColumn::named("authorship.book"),
            author: RefColumn::named("authorship.author"),
            book_author: Indexed::named("authorship.book_author"),
        }
    }
}

impl Table for Authorships {
    type Of = Authorship;

    fn ids(&self) -> &IdColumn<Authorship> {
        &self.ids
    }

    fn ids_mut(&mut self) -> &mut IdColumn<Authorship> {
        &mut self.ids
    }

    fn fields(&self) -> Vec<Field<'_>> {
        vec![
            Field::new("id", &self.ids),
            Field::new("book", &self.book),
            Field::new("author", &self.author),
            Field::new("book_author", &self.book_author),
        ]
    }

    fn fields_mut(&mut self) -> Vec<FieldMut<'_>> {
        vec![
            FieldMut::new("id", &mut self.ids),
            FieldMut::new("book", &mut self.book),
            FieldMut::new("author", &mut self.author),
            FieldMut::new("book_author", &mut self.book_author),
        ]
    }
}

impl Child<Book> for Authorship {
    fn parents(table: &Authorships) -> &RefColumn<Book, Authorship> {
        &table.book
    }
}

impl Child<Author> for Authorship {
    fn parents(table: &Authorships) -> &RefColumn<Author, Authorship> {
        &table.author
    }
}

impl Authorships {
    pub fn push(&mut self, book: Id<Book>, author: Id<Author>) -> Result<Id<Authorship>> {
        let pair = Pair::of(book, author);
        self.reserve()?;
        self.book.reach(book)?;
        self.author.reach(author)?;
        self.book_author.reach(pair)?;
        self.book_author.refuse_duplicate(pair)?;
        let id = self.mint()?;
        self.book.push(book)?;
        self.author.push(author)?;
        self.book_author.push(pair)?;
        Ok(id)
    }

    pub fn book(&self, authorship: Id<Authorship>) -> Result<Id<Book>> {
        self.book.get(self.row_of(authorship)?)
    }

    pub fn author(&self, authorship: Id<Authorship>) -> Result<Id<Author>> {
        self.author.get(self.row_of(authorship)?)
    }

    pub fn of_book(&self, book: Id<Book>) -> Result<Vec<Id<Authorship>>> {
        self.book.children_of(&self.ids, book)
    }

    pub fn of_author(&self, author: Id<Author>) -> Result<Vec<Id<Authorship>>> {
        self.author.children_of(&self.ids, author)
    }

    pub fn quietly_repoint_author(&mut self, row: crate::schema::Row, author: Id<Author>) {
        self.author.set(row, author).unwrap();
    }

    fn set_book(&mut self, row: crate::schema::Row, book: Id<Book>) -> Result<()> {
        self.book_author
            .set(row, Pair::of(book, self.author.get(row)?))?;
        self.book.set(row, book)
    }

    fn set_author(&mut self, row: crate::schema::Row, author: Id<Author>) -> Result<()> {
        self.book_author
            .set(row, Pair::of(self.book.get(row)?, author))?;
        self.author.set(row, author)
    }
}

pub struct AuthorshipRow<'a, S: Holds<Authorship>>(Entry<'a, S, Authorship>);

impl<'a, S: Holds<Authorship>> From<Entry<'a, S, Authorship>> for AuthorshipRow<'a, S> {
    fn from(entry: Entry<'a, S, Authorship>) -> Self {
        Self(entry)
    }
}

impl<'a, S: Holds<Authorship>> Deref for AuthorshipRow<'a, S> {
    type Target = Entry<'a, S, Authorship>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, S: Holds<Authorship>> AuthorshipRow<'a, S> {
    pub fn book(&self) -> Result<BookRow<'a, S>>
    where
        S: Holds<Book>,
    {
        let table = self.0.table();
        let row = self.0.row();
        self.0.at(table.book.get(row)?)
    }

    pub fn author(&self) -> Result<AuthorRow<'a, S>>
    where
        S: Holds<Author>,
    {
        let table = self.0.table();
        let row = self.0.row();
        self.0.at(table.author.get(row)?)
    }
}

pub struct AuthorshipRowMut<'a, S: Holds<Authorship>>(EntryMut<'a, S, Authorship>);

impl<'a, S: Holds<Authorship>> From<EntryMut<'a, S, Authorship>> for AuthorshipRowMut<'a, S> {
    fn from(entry: EntryMut<'a, S, Authorship>) -> Self {
        Self(entry)
    }
}

impl<'a, S: Holds<Authorship>> Deref for AuthorshipRowMut<'a, S> {
    type Target = EntryMut<'a, S, Authorship>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, S: Holds<Authorship>> DerefMut for AuthorshipRowMut<'a, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<S: Holds<Authorship>> AuthorshipRowMut<'_, S> {
    pub fn set_book(&mut self, value: Id<Book>) -> Result<()> {
        let row = self.0.row();
        let table = self.0.table_mut();
        table.set_book(row, value)
    }

    pub fn set_author(&mut self, value: Id<Author>) -> Result<()> {
        let row = self.0.row();
        let table = self.0.table_mut();
        table.set_author(row, value)
    }
}

#[derive(Default, Clone)]
pub struct Library {
    state: StoreState,
    pub books: Books,
    pub chapters: Chapters,
    pub paragraphs: Paragraphs,
    pub editions: Editions,
    pub authors: Authors,
    pub authorships: Authorships,
}

impl Store for Library {
    fn state(&self) -> &StoreState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut StoreState {
        &mut self.state
    }

    fn tables(&self) -> Vec<&dyn AnyTable> {
        vec![
            &self.books,
            &self.chapters,
            &self.paragraphs,
            &self.editions,
            &self.authors,
            &self.authorships,
        ]
    }

    fn tables_mut(&mut self) -> Vec<&mut dyn AnyTable> {
        vec![
            &mut self.books,
            &mut self.chapters,
            &mut self.paragraphs,
            &mut self.editions,
            &mut self.authors,
            &mut self.authorships,
        ]
    }

    fn audit(&self) -> Result<()> {
        <Book as Audited<Self>>::audit_in(self)?;
        <Chapter as Audited<Self>>::audit_in(self)?;
        <Paragraph as Audited<Self>>::audit_in(self)?;
        <Edition as Audited<Self>>::audit_in(self)?;
        <Author as Audited<Self>>::audit_in(self)?;
        <Authorship as Audited<Self>>::audit_in(self)
    }
}

impl<S: Holds<Book>> Audited<S> for Book {
    fn audit_in(store: &S) -> Result<()> {
        Table::audit(Holds::<Book>::table(store))
    }
}

impl<S: Holds<Chapter> + Holds<Book>> Audited<S> for Chapter {
    fn audit_in(store: &S) -> Result<()> {
        Table::audit(Holds::<Chapter>::table(store))?;
        audit_reference::<S, Book, Chapter>(store)
    }
}

impl<S: Holds<Paragraph> + Holds<Chapter>> Audited<S> for Paragraph {
    fn audit_in(store: &S) -> Result<()> {
        Table::audit(Holds::<Paragraph>::table(store))?;
        audit_reference::<S, Chapter, Paragraph>(store)
    }
}

impl<S: Holds<Edition> + Holds<Book>> Audited<S> for Edition {
    fn audit_in(store: &S) -> Result<()> {
        Table::audit(Holds::<Edition>::table(store))?;
        audit_reference::<S, Book, Edition>(store)
    }
}

impl<S: Holds<Author>> Audited<S> for Author {
    fn audit_in(store: &S) -> Result<()> {
        Table::audit(Holds::<Author>::table(store))
    }
}

impl<S: Holds<Authorship> + Holds<Book> + Holds<Author>> Audited<S> for Authorship {
    fn audit_in(store: &S) -> Result<()> {
        let links = Holds::<Authorship>::table(store);
        Table::audit(links)?;
        check_pairs::<Book, Author, Authorship>(&links.book_author, &links.book, &links.author)?;
        audit_reference::<S, Book, Authorship>(store)?;
        audit_reference::<S, Author, Authorship>(store)
    }
}

impl Holds<Book> for Library {
    fn table(&self) -> &Books {
        &self.books
    }

    fn table_mut(&mut self) -> &mut Books {
        &mut self.books
    }
}

impl Holds<Chapter> for Library {
    fn table(&self) -> &Chapters {
        &self.chapters
    }

    fn table_mut(&mut self) -> &mut Chapters {
        &mut self.chapters
    }
}

impl Holds<Paragraph> for Library {
    fn table(&self) -> &Paragraphs {
        &self.paragraphs
    }

    fn table_mut(&mut self) -> &mut Paragraphs {
        &mut self.paragraphs
    }
}

impl Holds<Edition> for Library {
    fn table(&self) -> &Editions {
        &self.editions
    }

    fn table_mut(&mut self) -> &mut Editions {
        &mut self.editions
    }
}

impl Holds<Author> for Library {
    fn table(&self) -> &Authors {
        &self.authors
    }

    fn table_mut(&mut self) -> &mut Authors {
        &mut self.authors
    }
}

impl Holds<Authorship> for Library {
    fn table(&self) -> &Authorships {
        &self.authorships
    }

    fn table_mut(&mut self) -> &mut Authorships {
        &mut self.authorships
    }
}

impl Library {
    pub fn at<E>(&self, id: Id<E>) -> Result<E::Row<'_, Self>>
    where
        Self: Holds<E>,
        E: Entity,
    {
        Ok(Entry::of(self, id)?.into())
    }

    pub fn at_mut<E>(&mut self, id: Id<E>) -> Result<E::RowMut<'_, Self>>
    where
        Self: Holds<E>,
        E: Entity,
    {
        Ok(EntryMut::of(self, id)?.into())
    }

    pub fn all<E>(&self) -> Rows<'_, Self, E>
    where
        Self: Holds<E>,
        E: Entity,
    {
        Rows::all(self)
    }

    pub fn chapters_of(&self, book: Id<Book>) -> Result<Vec<Id<Chapter>>> {
        self.chapters.of_book(book)
    }

    pub fn paragraphs_of(&self, chapter: Id<Chapter>) -> Result<Vec<Id<Paragraph>>> {
        self.paragraphs.of_chapter(chapter)
    }
}
