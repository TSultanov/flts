use std::borrow::Cow;
use super::soa_helpers::*;

pub struct Book {
    pub title: String,
    chapters: Vec<Chapter>,
    paragraphs: Vec<Paragraph>,
    strings: Vec<u8>,
}

struct Chapter {
    pub title: VecSlice<u8>,
    pub paragraphs: VecSlice<Paragraph>,
}

#[derive(Clone, Copy)]
struct Paragraph {
    original_html: Option<VecSlice<u8>>,
    original_text: VecSlice<u8>,
}

pub struct ChapterView<'a> {
    book: &'a Book,
    paragraphs: &'a [Paragraph],
    pub title: Cow<'a, str>,
}

pub struct ParagraphView<'a> {
    pub original_html: Option<Cow<'a, str>>,
    pub original_text: Cow<'a, str>,
}

impl Book {
    pub fn create(title: &str) -> Self {
        Book {
            title: title.to_owned(),
            chapters: vec![],
            paragraphs: vec![],
            strings: vec![],
        }
    }

    pub fn chapter_count(&self) -> usize {
        return self.chapters.len();
    }

    pub fn chapter_view(&self, chapter_index: usize) -> ChapterView {
        let chapter = &self.chapters[chapter_index];
        return ChapterView {
            book: self,
            title: String::from_utf8_lossy(chapter.title.slice(&self.strings)),
            paragraphs: chapter.paragraphs.slice(&self.paragraphs),
        };
    }

    pub fn push_chapter(&mut self, title: &str) {
        let title = push_string(&mut self.strings, title);
        self.chapters.push(Chapter {
            title: title,
            paragraphs: VecSlice::new(0, 0),
        });
    }

    pub fn push_paragraph(
        &mut self,
        chapter_index: usize,
        original_text: &str,
        original_html: Option<&str>,
    ) {
        let original_text = push_string(&mut self.strings, original_text);
        let original_html = original_html.map(|s| push_string(&mut self.strings, s));
        let new_paragraph = Paragraph {
            original_html,
            original_text,
        };
        let paragraphs_slice = push(
            &mut self.paragraphs,
            self.chapters[chapter_index].paragraphs,
            new_paragraph,
        )
        .unwrap();
        self.chapters[chapter_index].paragraphs = paragraphs_slice;
    }
}

impl<'a> ChapterView<'a> {
    pub fn paragraph_count(&self) -> usize {
        return self.paragraphs.len();
    }

    pub fn paragraph_view(&'a self, paragraph: usize) -> ParagraphView<'a> {
        let paragraph = &self.paragraphs[paragraph];
        return ParagraphView {
            original_html: paragraph
                .original_html
                .map(|s| String::from_utf8_lossy(s.slice(&self.book.strings))),
            original_text: String::from_utf8_lossy(
                paragraph.original_text.slice(&self.book.strings),
            ),
        };
    }
}

#[cfg(test)]
mod book_tests {
    use super::*;

    #[test]
    fn create_book() {
        let book = Book::create("Test");
        assert_eq!("Test", book.title);
    }

    #[test]
    fn create_book_empty_chapter() {
        let mut book = Book::create("Test");
        book.push_chapter("Test chapter");
        let first_chapter = book.chapter_view(0);
        assert_eq!("Test chapter", first_chapter.title);
    }

    #[test]
    fn create_book_one_chapter_one_paragraph() {
        let mut book = Book::create("Test");
        book.push_chapter("Test chapter");
        book.push_paragraph(0, "Test", Some("<b>Test</b>"));
        let first_chapter = book.chapter_view(0);
        let first_paragraph = first_chapter.paragraph_view(0);

        assert_eq!("Test", first_paragraph.original_text);
        assert_eq!("<b>Test</b>", first_paragraph.original_html.unwrap());
    }
}
