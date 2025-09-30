use crate::book::serialization::{
    read_exact_array, read_len_prefixed_vec, read_u64, read_u8, read_var_u64, read_vec_slice, validate_hash, write_u64, write_var_u64, write_vec_slice, ChecksumedWriter, Magic, Serializable, Version
};
use std::borrow::Cow;
use std::io::{self, Read, Write};

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
            &self.chapters[chapter_index].paragraphs,
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

impl Serializable for Book {
    fn serialize<TWriter: io::Write>(&self, output_stream: &mut TWriter) -> std::io::Result<()> {
        // Binary format (little-endian):
        // magic[4] = BK01
        // u8 version = 1
        // Metadata section
        // u64 metadata hash
        // u64 title_len, [u8]*
        // u64 chapters_count
        // u64 paragraphs_count
        // Data section
        // u64 strings_len (compressed), [u8]* (strings blob (zstd compressed))
        // u64 paragraphs_count
        //   repeat paragraphs_count times:
        //     u64 original_text.start, u64 original_text.len
        //     u8 has_html (0/1)
        //       if 1: u64 original_html.start, u64 original_html.len
        // u64 chapters_count
        //   repeat chapters_count times:
        //     u64 title.start, u64 title.len
        //     u64 paragraphs.start, u64 paragraphs.len
        // u64 fnv1 hash of the entire file except the hash itself

        let mut hashing_stream = ChecksumedWriter::create(output_stream);

        // Magic + version
        Magic::Book.write(&mut hashing_stream)?; // magic
        Version::V1.write_version(&mut hashing_stream)?; // version

        let mut metadata_buf = Vec::new();
        let mut metadata_buf_hasher = ChecksumedWriter::create(&mut metadata_buf);

        // Title
        write_var_u64(&mut metadata_buf_hasher, self.title.len() as u64)?;
        metadata_buf_hasher.write_all(self.title.as_bytes())?;

        // chapters count
        let chapters_count = self.chapter_count();
        write_var_u64(&mut metadata_buf_hasher, chapters_count as u64)?;

        // paragraphs count
        let paragraphs_count = (0..self.chapter_count()).into_iter().fold(0, |acc, ch| acc + self.chapter_view(ch).paragraph_count());
        write_var_u64(&mut metadata_buf_hasher, paragraphs_count as u64)?;

        // Write metadata
        // hash
        let metadata_hash = metadata_buf_hasher.current_hash();
        write_u64(&mut hashing_stream, metadata_hash)?;
        // metadata
        write_var_u64(&mut hashing_stream, metadata_buf.len() as u64)?;
        hashing_stream.write_all(&metadata_buf)?;

        // Strings blob
        let encoded = zstd::stream::encode_all(self.strings.as_slice(), 5)?;
        write_var_u64(&mut hashing_stream, encoded.len() as u64)?;
        hashing_stream.write_all(&encoded)?;

        // Paragraphs
        write_var_u64(&mut hashing_stream, self.paragraphs.len() as u64)?;
        for p in &self.paragraphs {
            write_vec_slice(&mut hashing_stream, &p.original_text)?;
            match p.original_html {
                Some(slice) => {
                    hashing_stream.write_all(&[1u8])?;
                    write_vec_slice(&mut hashing_stream, &slice)?;
                }
                None => hashing_stream.write_all(&[0u8])?,
            }
        }

        // Chapters
        write_var_u64(&mut hashing_stream, self.chapters.len() as u64)?;
        for c in &self.chapters {
            write_vec_slice(&mut hashing_stream, &c.title)?;
            write_vec_slice(&mut hashing_stream, &c.paragraphs)?;
        }

        // Hash
        let hash = hashing_stream.current_hash();
        write_u64(output_stream, hash)?;

        Ok(())
    }

    fn deserialize<TReader: io::Read + Clone>(input_stream: &mut TReader) -> std::io::Result<Self>
    where
        Self: Sized,
    {
        let hash_valid = validate_hash(input_stream)?;
        if !hash_valid {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid hash"));
        }

        // Magic
        let magic = read_exact_array::<4>(input_stream)?;
        if &magic != Magic::Book.as_bytes() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid magic"));
        }
        Version::read_version(input_stream)?; // ensure supported

        // Skip metadata hash - it's only for when read only metadata
        _ = read_u64(input_stream)?;

        // Skip metadata size
        _ = read_var_u64(input_stream)?;

        // Title
        let title_len = read_var_u64(input_stream)? as usize;
        let mut title_buf = vec![0u8; title_len];
        input_stream.read_exact(&mut title_buf)?;
        let title = String::from_utf8(title_buf)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid UTF-8 in title"))?;

        // skip chapters count
        _ = read_var_u64(input_stream)?;

        // skip paragraphs count
        _ = read_var_u64(input_stream)?;

        // Strings blob
        let encoded_data = read_len_prefixed_vec(input_stream)?;
        let strings = zstd::stream::decode_all(encoded_data.as_slice())?;

        // Paragraphs
        let paragraphs_len = read_var_u64(input_stream)? as usize;
        let mut paragraphs = Vec::with_capacity(paragraphs_len);
        for _ in 0..paragraphs_len {
            let original_text = read_vec_slice::<u8>(input_stream)?;
            let has_html = read_u8(input_stream)?;
            let original_html = if has_html == 1 {
                Some(read_vec_slice::<u8>(input_stream)?)
            } else {
                None
            };
            let paragraph = Paragraph {
                original_html,
                original_text,
            };
            paragraphs.push(paragraph);
        }

        // Chapters
        let chapters_len = read_var_u64(input_stream)? as usize;
        let mut chapters = Vec::with_capacity(chapters_len);
        for _ in 0..chapters_len {
            let title = read_vec_slice::<u8>(input_stream)?;
            let paragraphs_slice = read_vec_slice::<Paragraph>(input_stream)?;
            chapters.push(Chapter {
                title,
                paragraphs: paragraphs_slice,
            });
        }

        Ok(Book {
            title,
            chapters,
            paragraphs,
            strings,
        })
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

    #[test]
    fn serialize_deserialize_round_trip() {
        let mut book = Book::create("My Book");
        book.push_chapter("Intro");
        book.push_paragraph(0, "Hello world", Some("<p>Hello <b>world</b></p>"));
        book.push_paragraph(0, "Second paragraph", None);
        book.push_chapter("Second Chapter");
        book.push_paragraph(1, "Another one", Some("<i>Another</i> one"));

        let mut buffer: Vec<u8> = vec![];
        book.serialize(&mut buffer).unwrap();

        // Deserialize
        let mut cursor: &[u8] = &buffer;
        let book2 = Book::deserialize(&mut cursor).unwrap();

        assert_eq!(book2.title, "My Book");
        assert_eq!(book2.chapter_count(), 2);
        let ch0 = book2.chapter_view(0);
        assert_eq!(ch0.title, "Intro");
        assert_eq!(ch0.paragraph_count(), 2);
        let p0 = ch0.paragraph_view(0);
        assert_eq!(p0.original_text, "Hello world");
        assert_eq!(
            p0.original_html.as_ref().unwrap(),
            "<p>Hello <b>world</b></p>"
        );
        let p1 = ch0.paragraph_view(1);
        assert_eq!(p1.original_text, "Second paragraph");
        assert!(p1.original_html.is_none());
        let ch1 = book2.chapter_view(1);
        assert_eq!(ch1.title, "Second Chapter");
        assert_eq!(ch1.paragraph_count(), 1);
        let p2 = ch1.paragraph_view(0);
        assert_eq!(p2.original_text, "Another one");
        assert_eq!(p2.original_html.as_ref().unwrap(), "<i>Another</i> one");
    }

    #[test]
    fn serialize_deserialize_corruption() {
        let mut book = Book::create("My Book");
        book.push_chapter("Intro");
        book.push_paragraph(0, "Hello world", Some("<p>Hello <b>world</b></p>"));
        book.push_paragraph(0, "Second paragraph", None);
        book.push_chapter("Second Chapter");
        book.push_paragraph(1, "Another one", Some("<i>Another</i> one"));

        let mut buffer: Vec<u8> = vec![];
        book.serialize(&mut buffer).unwrap();

        // Corrupt data
        buffer[12] = 0xae;

        // Deserialize
        let mut cursor: &[u8] = &buffer;
        let book2 = Book::deserialize(&mut cursor);
        assert!(book2.is_err());
    }
}
