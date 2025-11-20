use std::{
    hash::Hasher,
    io::{self, Cursor, Read},
};

use uuid::Uuid;

use crate::book::serialization::{
    Magic, Version, read_exact_array, read_len_prefixed_vec, read_u64, read_var_u64,
};

pub struct BookMetadata {
    pub id: Uuid,
    pub title: String,
    pub language: String,
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
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid metadata hash",
            ));
        }

        let mut cursor = Cursor::new(metadata_buf);

        let id = Uuid::from_bytes(read_exact_array(&mut cursor)?);

        // Title
        let title_len = read_var_u64(&mut cursor)? as usize;
        let mut title_buf = vec![0u8; title_len];
        cursor.read_exact(&mut title_buf)?;
        let title = String::from_utf8(title_buf)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid UTF-8 in title"))?;

        // Language
        let language_len = read_var_u64(&mut cursor)? as usize;
        let mut language_buf = vec![0u8; language_len];
        cursor.read_exact(&mut language_buf)?;
        let language = String::from_utf8(language_buf)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid UTF-8 in language"))?;

        let chapters_count = read_var_u64(&mut cursor)? as usize;

        let paragraphs_count = read_var_u64(&mut cursor)? as usize;

        Ok(BookMetadata {
            id,
            title,
            language,
            chapters_count,
            paragraphs_count,
        })
    }
}

#[cfg(test)]
mod book_metadata_tests {
    use isolang::Language;
    use uuid::Uuid;

    use crate::book::{book::Book, book_metadata::BookMetadata, serialization::Serializable};

    #[test]
    fn test_metadata_roundtrip() {
        let language = "eng";
        let mut book = Book::create(
            Uuid::new_v4(),
            "My Book",
            &Language::from_639_3(language).unwrap(),
        );
        book.push_chapter(Some("Intro"));
        book.push_paragraph(0, "Hello world", Some("<p>Hello <b>world</b></p>"));
        book.push_paragraph(0, "Second paragraph", None);
        book.push_chapter(Some("Second Chapter"));
        book.push_paragraph(1, "Another one", Some("<i>Another</i> one"));

        let mut buffer: Vec<u8> = vec![];
        book.serialize(&mut buffer).unwrap();

        let mut cursor: &[u8] = &buffer;
        let metadata = BookMetadata::read_metadata(&mut cursor).unwrap();

        assert_eq!(metadata.title, "My Book");
        assert_eq!(metadata.chapters_count, 2);
        assert_eq!(metadata.paragraphs_count, 3);
        assert_eq!(metadata.language, language);
    }

    #[test]
    fn test_metadata_corruption() {
        let language = "eng";
        let mut book = Book::create(
            Uuid::new_v4(),
            "My Book",
            &Language::from_639_3(language).unwrap(),
        );
        book.push_chapter(Some("Intro"));
        book.push_paragraph(0, "Hello world", Some("<p>Hello <b>world</b></p>"));
        book.push_paragraph(0, "Second paragraph", None);
        book.push_chapter(Some("Second Chapter"));
        book.push_paragraph(1, "Another one", Some("<i>Another</i> one"));

        let mut buffer: Vec<u8> = vec![];
        book.serialize(&mut buffer).unwrap();

        buffer[15] = 0xae;

        let mut cursor: &[u8] = &buffer;
        let metadata = BookMetadata::read_metadata(&mut cursor);

        assert!(metadata.is_err());
    }
}
