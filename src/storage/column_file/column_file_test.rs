use std::path::Path;

use super::*;
use crate::id::Id;
use crate::storage::arena::Arena;
use crate::storage::blob::Blob;
use crate::test_utils::TempDir;

#[test]
fn round_trips_columns_and_reads_them_lazily() {
    let dir = TempDir::new("roundtrip");
    let path = dir.join("numbers.dbs");

    let mut values = Arena::<u32>::with_chunk_rows(4);
    values.extend_from_slice(&(0..10).collect::<Vec<u32>>());

    let mut writer = ColumnFileWriter::create(&path).unwrap();
    writer.column("values", &values).unwrap();
    writer.finish().unwrap();

    let file = ColumnFile::open(&path).unwrap();
    let meta = file.column_meta("values").unwrap();
    assert_eq!(meta.rows, 10);
    assert_eq!(meta.elem_size, 4);
    assert_eq!(meta.chunks.len(), 3);
    assert_eq!(meta.byte_len(), 40);

    let loaded = file.column::<u32>("values").unwrap();
    assert_eq!(loaded.len(), 10);
    assert_eq!(
        loaded.resident_chunks(),
        0,
        "opening a column reads no payload"
    );

    assert_eq!(loaded.try_get(9).unwrap(), Some(&9));
    assert_eq!(
        loaded.resident_chunks(),
        1,
        "only the chunk holding row 9 was read"
    );

    loaded.prefetch().unwrap();
    assert_eq!(
        loaded
            .try_iter()
            .collect::<std::io::Result<Vec<_>>>()
            .unwrap(),
        (0..10u32).collect::<Vec<_>>().iter().collect::<Vec<_>>()
    );
}

struct Chapter;
struct Paragraph;

struct Doc {
    bytes: Arena<u8>,
    chapter_title: Arena<Blob>,
    paragraph_chapter: Arena<Id<Chapter>>,
    paragraph_text: Arena<Blob>,
    paragraph_note: Arena<Option<Id<Paragraph>>>,
}

impl Doc {
    fn new() -> Self {
        Self {
            bytes: Arena::with_chunk_rows(16),
            chapter_title: Arena::with_chunk_rows(4),
            paragraph_chapter: Arena::with_chunk_rows(4),
            paragraph_text: Arena::with_chunk_rows(4),
            paragraph_note: Arena::with_chunk_rows(4),
        }
    }

    fn push_chapter(&mut self, title: &str) -> Id<Chapter> {
        let title = self.bytes.push_str(title);
        Id::from_index(self.chapter_title.push(title))
    }

    fn push_paragraph(
        &mut self,
        chapter: Id<Chapter>,
        text: &str,
        note_on: Option<Id<Paragraph>>,
    ) -> Id<Paragraph> {
        let text = self.bytes.push_str(text);
        let index = self.paragraph_chapter.push(chapter);
        self.paragraph_text.push(text);
        self.paragraph_note.push(note_on);
        Id::from_index(index)
    }

    fn paragraphs_of(&self, chapter: Id<Chapter>) -> Vec<Id<Paragraph>> {
        self.paragraph_chapter
            .try_iter()
            .enumerate()
            .filter_map(|(index, owner)| {
                (*owner.unwrap() == chapter).then(|| Id::from_index(index))
            })
            .collect()
    }

    fn text_of(&self, paragraph: Id<Paragraph>) -> String {
        let blob = *self.paragraph_text.get(paragraph.index()).unwrap();
        self.bytes.try_str(blob).unwrap().into_owned()
    }

    fn write(&self, path: &Path) {
        let mut writer = ColumnFileWriter::create(path).unwrap();
        writer.column("bytes", &self.bytes).unwrap();
        writer.column("chapter.title", &self.chapter_title).unwrap();
        writer
            .column("paragraph.chapter", &self.paragraph_chapter)
            .unwrap();
        writer
            .column("paragraph.text", &self.paragraph_text)
            .unwrap();
        writer
            .column("paragraph.note", &self.paragraph_note)
            .unwrap();
        writer.finish().unwrap();
    }

    fn read(file: &ColumnFile) -> Self {
        Self {
            bytes: file.column("bytes").unwrap(),
            chapter_title: file.column("chapter.title").unwrap(),
            paragraph_chapter: file.column("paragraph.chapter").unwrap(),
            paragraph_text: file.column("paragraph.text").unwrap(),
            paragraph_note: file.column("paragraph.note").unwrap(),
        }
    }
}

#[test]
fn tables_join_on_ids_and_appends_never_relocate_rows() {
    let mut doc = Doc::new();
    let one = doc.push_chapter("One");
    let two = doc.push_chapter("Two");

    doc.push_paragraph(one, "first of one", None);
    let in_two = doc.push_paragraph(two, "only of two", None);
    let before = doc.paragraphs_of(two);

    doc.push_paragraph(one, "second of one", Some(in_two));

    assert_eq!(
        doc.paragraphs_of(two),
        before,
        "chapter two's ids are untouched"
    );
    assert_eq!(doc.paragraphs_of(one).len(), 2);
    assert_eq!(doc.text_of(doc.paragraphs_of(one)[1]), "second of one");
}

#[test]
fn a_document_round_trips_through_a_column_file() {
    let dir = TempDir::new("doc");
    let path = dir.join("doc.dbs");

    let mut doc = Doc::new();
    let one = doc.push_chapter("One");
    let two = doc.push_chapter("Two");
    let anchor = doc.push_paragraph(one, "the first paragraph of chapter one", None);
    for index in 0..9 {
        doc.push_paragraph(
            two,
            &format!("chapter two, paragraph {index}"),
            Some(anchor),
        );
    }
    doc.write(&path);

    let file = ColumnFile::open(&path).unwrap();
    file.verify().unwrap();
    assert_eq!(file.columns().len(), 5);

    let loaded = Doc::read(&file);
    assert_eq!(loaded.paragraphs_of(one), vec![anchor]);
    assert_eq!(loaded.paragraphs_of(two).len(), 9);
    assert_eq!(loaded.text_of(anchor), "the first paragraph of chapter one");
    assert_eq!(
        loaded.text_of(Id::from_index(5)),
        "chapter two, paragraph 4"
    );

    assert_eq!(*loaded.paragraph_note.get(0).unwrap(), None);
    assert_eq!(*loaded.paragraph_note.get(1).unwrap(), Some(anchor));
    assert_eq!(
        file.column_meta("paragraph.note").unwrap().elem_size,
        file.column_meta("paragraph.chapter").unwrap().elem_size
    );

    let sampled = Doc::read(&file);
    let _ = sampled.text_of(anchor);
    assert!(sampled.bytes.resident_chunks() < sampled.bytes.chunk_count());
}

#[test]
fn an_empty_column_round_trips() {
    let dir = TempDir::new("empty_column");
    let path = dir.join("empty.dbs");

    let empty = Arena::<u32>::new();
    let mut one = Arena::<u32>::new();
    one.push(7);

    let mut writer = ColumnFileWriter::create(&path).unwrap();
    writer.column("empty", &empty).unwrap();
    writer.column("one", &one).unwrap();
    writer.finish().unwrap();

    let file = ColumnFile::open(&path).unwrap();
    file.verify().unwrap();

    let empty = file.column::<u32>("empty").unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.chunk_count(), 0);
    assert_eq!(empty.try_get(0).unwrap(), None);
    assert_eq!(
        file.column::<u32>("one").unwrap().try_get(0).unwrap(),
        Some(&7)
    );
}
