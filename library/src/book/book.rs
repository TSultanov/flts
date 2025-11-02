use log::info;
use uuid::Uuid;

use crate::book::serialization::{
    ChecksumedWriter, Magic, Serializable, Version, read_exact_array, read_len_prefixed_vec,
    read_opt, read_u8, read_u64, read_var_u64, read_vec_slice, validate_hash, write_opt, write_u64,
    write_var_u64, write_vec_slice,
};
use std::borrow::Cow;
use std::io::{self, BufWriter, Write};
use std::time::Instant;

use super::soa_helpers::*;

pub struct Book {
    pub id: Uuid,
    pub title: String,
    pub language: String,
    chapters: Vec<Chapter>,
    paragraph_map: Vec<usize>,
    paragraphs: Vec<Paragraph>,
    strings: Vec<u8>,
}

struct Chapter {
    pub title: Option<VecSlice<u8>>,
    pub paragraphs: VecSlice<usize>,
}

#[derive(Clone, Copy)]
struct Paragraph {
    id: usize,
    original_html: Option<VecSlice<u8>>,
    original_text: VecSlice<u8>,
}

pub struct ChapterView<'a> {
    pub idx: usize,
    book: &'a Book,
    paragraphs: Vec<&'a Paragraph>,
    pub title: Option<Cow<'a, str>>,
}

pub struct ParagraphView<'a> {
    pub id: usize,
    pub original_html: Option<Cow<'a, str>>,
    pub original_text: Cow<'a, str>,
}

impl Book {
    pub fn create(id: Uuid, title: &str, language: &isolang::Language) -> Self {
        Book {
            title: title.to_owned(),
            id,
            language: language.to_639_3().to_string(),
            chapters: vec![],
            paragraph_map: vec![],
            paragraphs: vec![],
            strings: vec![],
        }
    }

    pub fn chapter_count(&self) -> usize {
        self.chapters.len()
    }

    pub fn chapter_view(&self, chapter_index: usize) -> ChapterView<'_> {
        let chapter = &self.chapters[chapter_index];
        let paragraph_indexes = chapter.paragraphs.slice(&self.paragraph_map);
        ChapterView {
            idx: chapter_index,
            book: self,
            title: chapter
                .title
                .map(|t| String::from_utf8_lossy(t.slice(&self.strings))),
            paragraphs: paragraph_indexes
                .iter()
                .map(|p| &self.paragraphs[*p])
                .collect(),
        }
    }

    pub fn chapter_views(&self) -> impl Iterator<Item = ChapterView<'_>> {
        (0..self.chapter_count()).map(|c| self.chapter_view(c))
    }

    pub fn paragraph_view(&self, paragraph_id: usize) -> ParagraphView<'_> {
        let paragraph = &self.paragraphs[paragraph_id];
        ParagraphView {
            id: paragraph_id,
            original_html: paragraph
                .original_html
                .map(|h| String::from_utf8_lossy(h.slice(&self.strings))),
            original_text: String::from_utf8_lossy(paragraph.original_text.slice(&self.strings)),
        }
    }

    pub fn push_chapter(&mut self, title: Option<&str>) -> usize {
        let title = title.map(|t| push_string(&mut self.strings, t));
        self.chapters.push(Chapter {
            title,
            paragraphs: VecSlice::new(0, 0),
        });
        self.chapters.len() - 1
    }

    pub fn push_paragraph(
        &mut self,
        chapter_index: usize,
        original_text: &str,
        original_html: Option<&str>,
    ) -> usize {
        let original_text = push_string(&mut self.strings, original_text);
        let original_html = original_html.map(|s| push_string(&mut self.strings, s));
        let new_paragraph = Paragraph {
            id: 0,
            original_html,
            original_text,
        };
        self.paragraphs.push(new_paragraph);
        let paragraph_id = self.paragraphs.len() - 1;
        self.paragraphs[paragraph_id].id = paragraph_id;

        let paragraphs_slice = push(
            &mut self.paragraph_map,
            &self.chapters[chapter_index].paragraphs,
            paragraph_id,
        )
        .unwrap();
        self.chapters[chapter_index].paragraphs = paragraphs_slice;
        paragraphs_slice.len - 1
    }

    pub fn paragraphs_count(&self) -> usize {
        self.chapter_views().map(|v| v.paragraph_count()).sum()
    }
}

impl<'a> ChapterView<'a> {
    pub fn paragraph_count(&self) -> usize {
        self.paragraphs.len()
    }

    pub fn paragraph_view(&'a self, paragraph: usize) -> ParagraphView<'a> {
        let paragraph = self.paragraphs[paragraph];
        ParagraphView {
            id: paragraph.id,
            original_html: paragraph
                .original_html
                .map(|s| String::from_utf8_lossy(s.slice(&self.book.strings))),
            original_text: String::from_utf8_lossy(
                paragraph.original_text.slice(&self.book.strings),
            ),
        }
    }

    pub fn paragraphs(&'a self) -> impl Iterator<Item = ParagraphView<'a>> {
        (0..self.paragraph_count()).map(|p| self.paragraph_view(p))
    }
}

impl Serializable for Book {
    fn serialize<TWriter: io::Write>(&self, output_stream: &mut TWriter) -> std::io::Result<()> {
        // Binary format (little-endian):
        // magic[4] = BK01
        // u8 version = 1
        // Metadata section
        // u64 metadata hash
        // u8[16] id
        // u64 title_len, [u8]*
        // u64 language_len, [u8]*
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

        let total_start = Instant::now();

        let mut hashing_stream_unbuffered = ChecksumedWriter::create(output_stream);
        let mut hashing_stream = BufWriter::new(hashing_stream_unbuffered);

        // Magic + version
        let t_magic = Instant::now();
        Magic::Book.write(&mut hashing_stream)?; // magic
        Version::V1.write_version(&mut hashing_stream)?; // version
        let d_magic = t_magic.elapsed();

        // Build metadata buffer
        let t_meta_build = Instant::now();
        let mut metadata_buf = Vec::new();
        let mut metadata_buf_hasher = ChecksumedWriter::create(&mut metadata_buf);
        metadata_buf_hasher.write_all(self.id.as_bytes())?;
        // Title
        write_var_u64(&mut metadata_buf_hasher, self.title.len() as u64)?;
        metadata_buf_hasher.write_all(self.title.as_bytes())?;
        // Language
        write_var_u64(&mut metadata_buf_hasher, self.language.len() as u64)?;
        metadata_buf_hasher.write_all(self.language.as_bytes())?;
        // chapters count
        let chapters_count = self.chapter_count();
        write_var_u64(&mut metadata_buf_hasher, chapters_count as u64)?;
        // paragraphs count
        let paragraphs_count = (0..self.chapter_count())
            .fold(0, |acc, ch| acc + self.chapter_view(ch).paragraph_count());
        write_var_u64(&mut metadata_buf_hasher, paragraphs_count as u64)?;
        let metadata_hash = metadata_buf_hasher.current_hash();
        let d_meta_build = t_meta_build.elapsed();

        // Write metadata
        let t_meta_write = Instant::now();
        write_u64(&mut hashing_stream, metadata_hash)?;
        write_var_u64(&mut hashing_stream, metadata_buf.len() as u64)?;
        hashing_stream.write_all(&metadata_buf)?;
        let d_meta_write = t_meta_write.elapsed();

        // Strings blob compress
        let t_compress = Instant::now();
        let encoded = zstd::stream::encode_all(self.strings.as_slice(), -7)?;
        let d_compress = t_compress.elapsed();

        // Strings write
        let t_write_strings = Instant::now();
        write_var_u64(&mut hashing_stream, encoded.len() as u64)?;
        hashing_stream.write_all(&encoded)?;
        let d_write_strings = t_write_strings.elapsed();

        // Paragraphs
        let t_paragraphs = Instant::now();
        write_var_u64(&mut hashing_stream, self.paragraphs.len() as u64)?;
        for p in &self.paragraphs {
            write_var_u64(&mut hashing_stream, p.id as u64)?;
            write_vec_slice(&mut hashing_stream, &p.original_text)?;
            match p.original_html {
                Some(slice) => {
                    hashing_stream.write_all(&[1u8])?;
                    write_vec_slice(&mut hashing_stream, &slice)?;
                }
                None => hashing_stream.write_all(&[0u8])?,
            }
        }
        let d_paragraphs = t_paragraphs.elapsed();

        // Paragraphs map
        let t_pmap = Instant::now();
        write_var_u64(&mut hashing_stream, self.paragraph_map.len() as u64)?;
        for p in &self.paragraph_map {
            write_var_u64(&mut hashing_stream, *p as u64)?;
        }
        let d_pmap = t_pmap.elapsed();

        // Chapters
        let t_chapters = Instant::now();
        write_var_u64(&mut hashing_stream, self.chapters.len() as u64)?;
        for c in &self.chapters {
            write_opt(&mut hashing_stream, &c.title)?;
            write_vec_slice(&mut hashing_stream, &c.paragraphs)?;
        }
        let d_chapters = t_chapters.elapsed();

        // Hash
        let t_finalize = Instant::now();
        hashing_stream_unbuffered = hashing_stream.into_inner()?;
        let hash = hashing_stream_unbuffered.current_hash();
        write_u64(output_stream, hash)?;
        output_stream.flush()?;
        let d_finalize = t_finalize.elapsed();

        let total = total_start.elapsed();

        info!(
            "Serialization timings (Book):\n  - magic+version: {:?}\n  - metadata build: {:?}\n  - metadata write: {:?}\n  - strings compress ({} -> {} bytes): {:?}\n  - strings write: {:?}\n  - paragraphs ({}): {:?}\n  - paragraph map ({}): {:?}\n  - chapters ({}): {:?}\n  - finalize hash+flush: {:?}\n  - TOTAL: {:?}",
            d_magic,
            d_meta_build,
            d_meta_write,
            self.strings.len(),
            encoded.len(),
            d_compress,
            d_write_strings,
            self.paragraphs.len(),
            d_paragraphs,
            self.paragraph_map.len(),
            d_pmap,
            self.chapters.len(),
            d_chapters,
            d_finalize,
            total
        );

        Ok(())
    }

    fn deserialize<TReader: io::Seek + io::Read>(
        input_stream: &mut TReader,
    ) -> std::io::Result<Self>
    where
        Self: Sized,
    {
        let total_start = Instant::now();

        // Validate checksum
        let t_hash = Instant::now();
        let hash_valid = validate_hash(input_stream)?;
        if !hash_valid {
            log::error!("Failed to read book: Invalid hash");
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid hash"));
        }
        let d_hash = t_hash.elapsed();

        // Magic + version
        let t_magic = Instant::now();
        let magic = read_exact_array::<4>(input_stream)?;
        if &magic != Magic::Book.as_bytes() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid magic"));
        }
        Version::read_version(input_stream)?; // ensure supported
        let d_magic = t_magic.elapsed();

        // Metadata (skip hash/len, then read fields)
        let t_meta = Instant::now();
        // Skip metadata hash - it's only for when read only metadata
        _ = read_u64(input_stream)?;

        // Skip metadata size
        _ = read_var_u64(input_stream)?;

        let id = Uuid::from_bytes(read_exact_array::<16>(input_stream)?);

        // Title
        let title_len = read_var_u64(input_stream)? as usize;
        let mut title_buf = vec![0u8; title_len];
        input_stream.read_exact(&mut title_buf)?;
        let title = String::from_utf8(title_buf)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid UTF-8 in title"))?;

        // Language
        let language_len = read_var_u64(input_stream)? as usize;
        let mut language_buf = vec![0u8; language_len];
        input_stream.read_exact(&mut language_buf)?;
        let language = String::from_utf8(language_buf)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid UTF-8 in language"))?;

        // skip chapters count
        _ = read_var_u64(input_stream)?;

        // skip paragraphs count
        _ = read_var_u64(input_stream)?;
        let d_meta = t_meta.elapsed();

        // Strings blob
        let t_strings_read = Instant::now();
        let encoded_data = read_len_prefixed_vec(input_stream)?;
        let d_strings_read = t_strings_read.elapsed();
        let t_strings_decompress = Instant::now();
        let strings = zstd::stream::decode_all(encoded_data.as_slice())?;
        let d_strings_decompress = t_strings_decompress.elapsed();

        // Paragraphs
        let t_paragraphs = Instant::now();
        let paragraphs_len = read_var_u64(input_stream)? as usize;
        let mut paragraphs = Vec::with_capacity(paragraphs_len);
        for _ in 0..paragraphs_len {
            let id = read_var_u64(input_stream)? as usize;
            let original_text = read_vec_slice::<u8>(input_stream)?;
            let has_html = read_u8(input_stream)?;
            let original_html = if has_html == 1 {
                Some(read_vec_slice::<u8>(input_stream)?)
            } else {
                None
            };
            let paragraph = Paragraph {
                id,
                original_html,
                original_text,
            };
            paragraphs.push(paragraph);
        }
        let d_paragraphs = t_paragraphs.elapsed();

        // Paragraphs map
        let t_pmap = Instant::now();
        let paragraph_map_len = read_var_u64(input_stream)?;
        let mut paragraph_map = Vec::with_capacity(paragraph_map_len as usize);
        for _ in 0..paragraph_map_len {
            let p = read_var_u64(input_stream)? as usize;
            paragraph_map.push(p);
        }
        let d_pmap = t_pmap.elapsed();

        // Chapters
        let t_chapters = Instant::now();
        let chapters_len = read_var_u64(input_stream)? as usize;
        let mut chapters = Vec::with_capacity(chapters_len);
        for _ in 0..chapters_len {
            let title = read_opt(input_stream)?;
            let paragraphs_slice = read_vec_slice::<usize>(input_stream)?;
            chapters.push(Chapter {
                title,
                paragraphs: paragraphs_slice,
            });
        }
        let d_chapters = t_chapters.elapsed();

        let total = total_start.elapsed();

        info!(
            "Deserialization timings (Book):\n  - hash validate: {:?}\n  - magic+version: {:?}\n  - metadata (incl. read): {:?}\n  - strings read: {:?}\n  - strings decompress ({} -> {} bytes): {:?}\n  - paragraphs ({}): {:?}\n  - paragraph map ({}): {:?}\n  - chapters ({}): {:?}\n  - TOTAL: {:?}",
            d_hash,
            d_magic,
            d_meta,
            d_strings_read,
            encoded_data.len(),
            strings.len(),
            d_strings_decompress,
            paragraphs_len,
            d_paragraphs,
            paragraph_map_len,
            d_pmap,
            chapters_len,
            d_chapters,
            total
        );

        Ok(Book {
            id,
            title,
            language,
            chapters,
            paragraphs,
            paragraph_map,
            strings,
        })
    }
}

#[cfg(test)]
mod book_tests {
    use std::io::Cursor;

    use isolang::Language;

    use super::*;

    #[test]
    fn create_book() {
        let book = Book::create(Uuid::new_v4(), "Test", &Language::from_639_3("eng").unwrap());
        assert_eq!("Test", book.title);
    }

    #[test]
    fn create_book_empty_chapter() {
        let mut book = Book::create(Uuid::new_v4(), "Test", &Language::from_639_3("eng").unwrap());
        let chapter_index = book.push_chapter(Some("Test chapter"));
        let first_chapter = book.chapter_view(chapter_index);
        assert_eq!(0, chapter_index);
        assert_eq!("Test chapter", first_chapter.title.unwrap());
    }

    #[test]
    fn create_book_one_chapter_one_paragraph() {
        let mut book = Book::create(Uuid::new_v4(), "Test", &Language::from_639_3("eng").unwrap());
        let chapter_index = book.push_chapter(Some("Test chapter"));
        let paragraph_index = book.push_paragraph(chapter_index, "Test", Some("<b>Test</b>"));
        let first_chapter = book.chapter_view(0);
        let first_paragraph = first_chapter.paragraph_view(0);

        assert_eq!(0, chapter_index);
        assert_eq!(0, paragraph_index);
        assert_eq!("Test", first_paragraph.original_text);
        assert_eq!("<b>Test</b>", first_paragraph.original_html.unwrap());
    }

    #[test]
    fn serialize_deserialize_round_trip() {
        let mut book = Book::create(Uuid::new_v4(), "My Book", &Language::from_639_3("eng").unwrap());
        let chapter_index = book.push_chapter(Some("Intro"));
        let first_paragraph = book.push_paragraph(
            chapter_index,
            "Hello world",
            Some("<p>Hello <b>world</b></p>"),
        );
        let second_paragraph = book.push_paragraph(chapter_index, "Second paragraph", None);
        let second_chapter_index = book.push_chapter(Some("Second Chapter"));
        let second_chapter_first_paragraph = book.push_paragraph(
            second_chapter_index,
            "Another one",
            Some("<i>Another</i> one"),
        );

        let mut buffer: Vec<u8> = vec![];
        book.serialize(&mut buffer).unwrap();

        // Deserialize
        let mut cursor = Cursor::new(buffer);
        let book2 = Book::deserialize(&mut cursor).unwrap();

        assert_eq!(0, chapter_index);
        assert_eq!(1, second_chapter_index);
        assert_eq!(0, first_paragraph);
        assert_eq!(1, second_paragraph);
        assert_eq!(0, second_chapter_first_paragraph);
        assert_eq!(book2.title, "My Book");
        assert_eq!(book2.chapter_count(), 2);
        let ch0 = book2.chapter_view(0);
        assert_eq!(ch0.title.as_ref().unwrap(), "Intro");
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
        assert_eq!(ch1.title.as_ref().unwrap(), "Second Chapter");
        assert_eq!(ch1.paragraph_count(), 1);
        let p2 = ch1.paragraph_view(0);
        assert_eq!(p2.original_text, "Another one");
        assert_eq!(p2.original_html.as_ref().unwrap(), "<i>Another</i> one");
    }

    #[test]
    fn serialize_deserialize_corruption() {
        let mut book = Book::create(Uuid::new_v4(), "My Book", &Language::from_639_3("eng").unwrap());
        book.push_chapter(Some("Intro"));
        book.push_paragraph(0, "Hello world", Some("<p>Hello <b>world</b></p>"));
        book.push_paragraph(0, "Second paragraph", None);
        book.push_chapter(Some("Second Chapter"));
        book.push_paragraph(1, "Another one", Some("<i>Another</i> one"));

        let mut buffer: Vec<u8> = vec![];
        book.serialize(&mut buffer).unwrap();

        // Corrupt data
        buffer[12] = 0xae;

        // Deserialize
        let mut cursor = Cursor::new(buffer);
        let book2 = Book::deserialize(&mut cursor);
        assert!(book2.is_err());
    }
}
