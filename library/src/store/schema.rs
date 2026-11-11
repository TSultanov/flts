//! The library's tables. Row ids are derived from content identity, so two
//! devices that write the same thing independently name the same row.

use dbsync::Id;
use uuid::Uuid;

#[dbsync::field_enum]
#[derive(Debug, PartialEq, Eq)]
pub enum AnkiStateCode {
    Active = 0,
    Suspended = 1,
    Deleted = 2,
}

#[dbsync::entity]
pub struct Book {
    title: String,
    language: String,
    chapters_count: u32,
    paragraphs_count: u32,
}

#[dbsync::entity]
pub struct Chapter {
    book: One<Book>,
    #[order(book)]
    number: u32,
    title: Option<String>,
}

#[dbsync::entity]
pub struct Paragraph {
    chapter: One<Chapter>,
    /// Flat index across the whole book.
    #[order(chapter)]
    number: u32,
    text: String,
    html: Option<String>,
}

#[dbsync::entity]
pub struct Translation {
    book: One<Book>,
    source_language: String,
    target_language: String,
}

/// One immutable version of one paragraph's translation.
#[dbsync::entity]
pub struct ParagraphTranslation {
    translation: One<Translation>,
    #[order(translation)]
    paragraph: u32,
    timestamp: u64,
    model: String,
    total_tokens: Option<u64>,
    /// No sentences: an LLM refusal, which reads as untranslated.
    empty: bool,
    /// JSON of `translation_import::ParagraphTranslation`.
    content: Vec<u8>,
}

/// Immutable; the newest timestamp per chapter is the current summary.
#[dbsync::entity]
pub struct ChapterSummary {
    book: One<Book>,
    #[order(book)]
    chapter: u32,
    timestamp: u64,
    model: String,
    text: String,
}

#[dbsync::entity]
pub struct ReadingPosition {
    book: One<Book>,
    chapter: u32,
    paragraph: u32,
    page_offset: u32,
}

#[dbsync::entity]
pub struct BookFolder {
    book: One<Book>,
    /// JSON array of folder names.
    path: String,
}

/// The reader's place as an old-format file had it. Only the import writes
/// these, and a position the app wrote outranks one whatever their stamps:
/// a device importing late must not bring an old place back.
#[dbsync::entity]
pub struct ImportedPosition {
    book: One<Book>,
    chapter: u32,
    paragraph: u32,
    page_offset: u32,
}

/// Likewise for the folder.
#[dbsync::entity]
pub struct ImportedFolder {
    book: One<Book>,
    /// JSON array of folder names.
    path: String,
}

#[dbsync::entity]
pub struct Card {
    /// `<src>-<tgt>`, ISO 639-3.
    #[index]
    pair: String,
    slug: String,
    lemma: String,
    anki_state: Option<AnkiStateCode>,
    interval_days: Option<f64>,
    ease_factor: Option<f64>,
    fsrs_difficulty: Option<f64>,
    fsrs_stability: Option<f64>,
}

#[dbsync::entity]
pub struct CardTranslation {
    card: One<Card>,
    #[order(card)]
    position: u32,
    part_of_speech: String,
    translation: String,
}

#[dbsync::entity]
pub struct CardExample {
    card: One<Card>,
    source: String,
    translation: String,
    book_id: Vec<u8>,
    chapter: u32,
    paragraph: u32,
}

#[dbsync::container(schema = "flts-library", version = 1)]
pub struct LibraryDb {
    books: Set<Book>,
    chapters: Set<Chapter>,
    paragraphs: Set<Paragraph>,
    translations: Set<Translation>,
    paragraph_translations: Set<ParagraphTranslation>,
    chapter_summaries: Set<ChapterSummary>,
    reading_positions: Set<ReadingPosition>,
    book_folders: Set<BookFolder>,
    imported_positions: Set<ImportedPosition>,
    imported_folders: Set<ImportedFolder>,
    cards: Set<Card>,
    card_translations: Set<CardTranslation>,
    card_examples: Set<CardExample>,
}

pub fn to_db(id: Uuid) -> dbsync::Uuid {
    dbsync::Uuid::from_bytes(*id.as_bytes())
}

pub fn from_db(id: dbsync::Uuid) -> Uuid {
    Uuid::from_bytes(id.to_bytes())
}

pub fn book_id(id: Uuid) -> Id<Book> {
    Id::from_uuid(to_db(id))
}

pub fn derived<T>(parent: dbsync::Uuid, name: &str) -> Id<T> {
    Id::derived(parent, name.as_bytes())
}

/// Namespace for rows with no parent row to derive from.
pub const ROOT: dbsync::Uuid = dbsync::Uuid::from_u128(0x666c_7473_6c69_6272_6172_7900_0000_0001);

impl LibraryDb {
    /// Whole-row writes: one revision, where per-field setters would append one each.
    pub fn put_reading_position(
        &mut self,
        book: Id<Book>,
        chapter: u32,
        paragraph: u32,
        page_offset: u32,
    ) -> dbsync::Result<()> {
        use dbsync::schema::Table;
        let id = derived::<ReadingPosition>(book.uuid(), "reading");
        if self.reading_positions.versions_of(id)?.is_empty() {
            self.reading_positions
                .insert(id, book, chapter, paragraph, page_offset)
        } else {
            self.reading_positions
                .rewrite(id, book, chapter, paragraph, page_offset)
        }
    }

    pub fn put_book_folder(&mut self, book: Id<Book>, path: &str) -> dbsync::Result<()> {
        use dbsync::schema::Table;
        let id = derived::<BookFolder>(book.uuid(), "folder");
        if self.book_folders.versions_of(id)?.is_empty() {
            self.book_folders.insert(id, book, path)
        } else {
            self.book_folders.set_path(id, path)
        }
    }

    pub fn put_imported_position(
        &mut self,
        book: Id<Book>,
        chapter: u32,
        paragraph: u32,
        page_offset: u32,
    ) -> dbsync::Result<()> {
        use dbsync::schema::Table;
        let id = derived::<ImportedPosition>(book.uuid(), "imported-reading");
        if self.imported_positions.versions_of(id)?.is_empty() {
            self.imported_positions
                .insert(id, book, chapter, paragraph, page_offset)
        } else {
            self.imported_positions
                .rewrite(id, book, chapter, paragraph, page_offset)
        }
    }

    pub fn put_imported_folder(&mut self, book: Id<Book>, path: &str) -> dbsync::Result<()> {
        use dbsync::schema::Table;
        let id = derived::<ImportedFolder>(book.uuid(), "imported-folder");
        if self.imported_folders.versions_of(id)?.is_empty() {
            self.imported_folders.insert(id, book, path)
        } else {
            self.imported_folders.set_path(id, path)
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn put_card(
        &mut self,
        id: Id<Card>,
        pair: &str,
        slug: &str,
        lemma: &str,
        anki_state: Option<AnkiStateCode>,
        interval_days: Option<f64>,
        ease_factor: Option<f64>,
        fsrs_difficulty: Option<f64>,
        fsrs_stability: Option<f64>,
    ) -> dbsync::Result<()> {
        use dbsync::schema::Table;
        if self.cards.versions_of(id)?.is_empty() {
            self.cards.insert(
                id,
                pair,
                slug,
                lemma,
                anki_state,
                interval_days,
                ease_factor,
                fsrs_difficulty,
                fsrs_stability,
            )
        } else {
            self.cards.rewrite(
                id,
                pair,
                slug,
                lemma,
                anki_state,
                interval_days,
                ease_factor,
                fsrs_difficulty,
                fsrs_stability,
            )
        }
    }
}
