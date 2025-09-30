use std::{hash::Hasher, io::{self, Cursor, Read}};

use crate::book::serialization::{read_exact_array, read_len_prefixed_vec, read_u64, read_var_u64, validate_hash, Magic, Version};

pub struct BookMetadata {
    pub title: String,
    pub chapters_count: usize,
    pub paragraphs_count: usize,
}

impl BookMetadata {
    pub fn read_metadata<TReader: io::Read>(input_stream: &mut TReader) -> io::Result<Self>
    where
        Self: Sized,
    {
        // Magic
        let magic = read_exact_array::<4>(input_stream)?;
        if &magic != Magic::Book.as_bytes() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid magic"));
        }
        Version::read_version(input_stream)?; // ensure supported

        // hash
        let metadata_hash = read_u64(input_stream)?;

        // Read metadata
        let metadata_buf = read_len_prefixed_vec(input_stream)?;

        let mut hasher = fnv::FnvHasher::default();
        hasher.write(&metadata_buf);
        if hasher.finish() != metadata_hash {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid metadata hash"));
        }

        let mut cursor = Cursor::new(metadata_buf);

        // Title
        let title_len = read_var_u64(&mut cursor)? as usize;
        let mut title_buf = vec![0u8; title_len];
        cursor.read_exact(&mut title_buf)?;
        let title = String::from_utf8(title_buf)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid UTF-8 in title"))?;

        let chapters_count = read_var_u64(&mut cursor)? as usize;

        let paragraphs_count = read_var_u64(&mut cursor)? as usize;

        Ok(BookMetadata {
            title: title,
            chapters_count,
            paragraphs_count,
        })
    }
}

#[cfg(test)]
mod book_metadata_tests {
    use crate::book::{book::Book, book_metadata::BookMetadata, serialization::Serializable};

    #[test]
    fn test_metadata_roundtrip() {
        let mut book = Book::create("My Book");
        book.push_chapter("Intro");
        book.push_paragraph(0, "Hello world", Some("<p>Hello <b>world</b></p>"));
        book.push_paragraph(0, "Second paragraph", None);
        book.push_chapter("Second Chapter");
        book.push_paragraph(1, "Another one", Some("<i>Another</i> one"));

        let mut buffer: Vec<u8> = vec![];
        book.serialize(&mut buffer).unwrap();

        let mut cursor: &[u8] = &buffer;
        let metadata = BookMetadata::read_metadata(&mut cursor).unwrap();

        assert_eq!(metadata.title, "My Book");
        assert_eq!(metadata.chapters_count, 2);
        assert_eq!(metadata.paragraphs_count, 3);
    }

    #[test]
    fn test_metadata_corruption() {
        let mut book = Book::create("My Book");
        book.push_chapter("Intro");
        book.push_paragraph(0, "Hello world", Some("<p>Hello <b>world</b></p>"));
        book.push_paragraph(0, "Second paragraph", None);
        book.push_chapter("Second Chapter");
        book.push_paragraph(1, "Another one", Some("<i>Another</i> one"));

        let mut buffer: Vec<u8> = vec![];
        book.serialize(&mut buffer).unwrap();

        buffer[15] = 0xae;

        let mut cursor: &[u8] = &buffer;
        let metadata = BookMetadata::read_metadata(&mut cursor);

        assert!(metadata.is_err());
    }
}