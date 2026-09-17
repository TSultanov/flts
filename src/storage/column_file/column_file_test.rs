use std::path::Path;

use super::*;
use crate::error::Result;
use crate::id::Id;
use crate::storage::arena::Arena;
use crate::storage::blob::Blob;
use crate::test_utils::TempDir;
use crate::test_utils::column_file_bytes as model;
use proptest::prelude::*;

#[test]
fn round_trips_columns_and_reads_them_lazily() {
    let dir = TempDir::new("roundtrip");
    let path = dir.join("numbers.dbs");

    let mut values = Arena::<u32>::with_chunk_rows(4);
    values
        .extend_from_slice(&(0..10).collect::<Vec<u32>>())
        .unwrap();

    let mut writer = ColumnFileWriter::create(&path, WriterId::ANONYMOUS).unwrap();
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

    assert_eq!(loaded.get(9).unwrap(), Some(&9));
    assert_eq!(
        loaded.resident_chunks(),
        1,
        "only the chunk holding row 9 was read"
    );

    loaded.prefetch().unwrap();
    assert_eq!(
        loaded.iter().collect::<Result<Vec<_>>>().unwrap(),
        (0..10u32).collect::<Vec<_>>().iter().collect::<Vec<_>>()
    );
}

struct Chapter;
struct Paragraph;

struct Doc {
    bytes: Arena<u8>,
    chapter_id: Arena<Id<Chapter>>,
    chapter_title: Arena<Blob>,
    paragraph_id: Arena<Id<Paragraph>>,
    paragraph_chapter: Arena<Id<Chapter>>,
    paragraph_text: Arena<Blob>,
    paragraph_note: Arena<Option<Id<Paragraph>>>,
}

impl Doc {
    fn new() -> Self {
        Self {
            bytes: Arena::with_chunk_rows(16),
            chapter_id: Arena::with_chunk_rows(4),
            chapter_title: Arena::with_chunk_rows(4),
            paragraph_id: Arena::with_chunk_rows(4),
            paragraph_chapter: Arena::with_chunk_rows(4),
            paragraph_text: Arena::with_chunk_rows(4),
            paragraph_note: Arena::with_chunk_rows(4),
        }
    }

    fn push_chapter(&mut self, title: &str) -> Id<Chapter> {
        let title = self.bytes.push_str(title).unwrap();
        let id = Id::mint();
        self.chapter_id.push(id).unwrap();
        self.chapter_title.push(title).unwrap();
        id
    }

    fn push_paragraph(
        &mut self,
        chapter: Id<Chapter>,
        text: &str,
        note_on: Option<Id<Paragraph>>,
    ) -> Id<Paragraph> {
        let text = self.bytes.push_str(text).unwrap();
        let id = Id::mint();
        self.paragraph_id.push(id).unwrap();
        self.paragraph_chapter.push(chapter).unwrap();
        self.paragraph_text.push(text).unwrap();
        self.paragraph_note.push(note_on).unwrap();
        id
    }

    fn paragraphs_of(&self, chapter: Id<Chapter>) -> Vec<Id<Paragraph>> {
        self.paragraph_chapter
            .iter()
            .enumerate()
            .filter_map(|(index, owner)| {
                (*owner.unwrap() == chapter).then(|| self.paragraph(index))
            })
            .collect()
    }

    fn paragraph(&self, row: usize) -> Id<Paragraph> {
        *self.paragraph_id.row(row).unwrap()
    }

    fn row_of(&self, paragraph: Id<Paragraph>) -> usize {
        self.paragraph_id
            .iter()
            .position(|held| *held.unwrap() == paragraph)
            .unwrap()
    }

    fn text_of(&self, paragraph: Id<Paragraph>) -> String {
        let blob = *self.paragraph_text.row(self.row_of(paragraph)).unwrap();
        self.bytes.text(blob).unwrap().into_owned()
    }

    fn write(&self, path: &Path) {
        let mut writer = ColumnFileWriter::create(path, WriterId::ANONYMOUS).unwrap();
        writer.column("bytes", &self.bytes).unwrap();
        writer.column("chapter.id", &self.chapter_id).unwrap();
        writer.column("chapter.title", &self.chapter_title).unwrap();
        writer.column("paragraph.id", &self.paragraph_id).unwrap();
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
            chapter_id: file.column("chapter.id").unwrap(),
            chapter_title: file.column("chapter.title").unwrap(),
            paragraph_id: file.column("paragraph.id").unwrap(),
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
    assert_eq!(file.columns().len(), 7);

    let loaded = Doc::read(&file);
    assert_eq!(loaded.paragraphs_of(one), vec![anchor]);
    assert_eq!(loaded.paragraphs_of(two).len(), 9);
    assert_eq!(loaded.text_of(anchor), "the first paragraph of chapter one");
    assert_eq!(
        loaded.text_of(loaded.paragraph(5)),
        "chapter two, paragraph 4"
    );

    assert_eq!(loaded.paragraph_note.get(0).unwrap(), Some(&None));
    assert_eq!(loaded.paragraph_note.get(1).unwrap(), Some(&Some(anchor)));
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
    one.push(7).unwrap();

    let mut writer = ColumnFileWriter::create(&path, WriterId::ANONYMOUS).unwrap();
    writer.column("empty", &empty).unwrap();
    writer.column("one", &one).unwrap();
    writer.finish().unwrap();

    let file = ColumnFile::open(&path).unwrap();
    file.verify().unwrap();

    let empty = file.column::<u32>("empty").unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.chunk_count(), 0);
    assert_eq!(empty.get(0).unwrap(), None);
    assert_eq!(file.column::<u32>("one").unwrap().get(0).unwrap(), Some(&7));
}

#[test]
fn rewriting_a_file_streams_chunks_without_making_them_resident() {
    let dir = TempDir::new("rewrite");
    let original = dir.join("original.dbs");
    let rewritten = dir.join("rewritten.dbs");

    let mut values = Arena::<u32>::with_chunk_rows(4);
    values
        .extend_from_slice(&(0..10).collect::<Vec<u32>>())
        .unwrap();
    let mut blob_bytes = Arena::<u8>::with_chunk_rows(16);
    blob_bytes
        .push_str("text long enough to spread across several small chunks")
        .unwrap();

    let mut writer = ColumnFileWriter::create(&original, WriterId::ANONYMOUS).unwrap();
    writer.column("values", &values).unwrap();
    writer.column("bytes", &blob_bytes).unwrap();
    writer.finish().unwrap();

    let file = ColumnFile::open(&original).unwrap();
    let values = file.column::<u32>("values").unwrap();
    let blob_bytes = file.column::<u8>("bytes").unwrap();
    assert!(values.chunk_count() > 1 && blob_bytes.chunk_count() > 1);

    let mut writer = ColumnFileWriter::create(&rewritten, WriterId::ANONYMOUS).unwrap();
    writer.column("values", &values).unwrap();
    writer.column("bytes", &blob_bytes).unwrap();
    writer.finish().unwrap();

    assert_eq!(values.resident_chunks(), 0);
    assert_eq!(blob_bytes.resident_chunks(), 0);

    let before = std::fs::read(&original).unwrap();
    let after = std::fs::read(&rewritten).unwrap();
    assert_eq!(
        &before[HEAD_LEN as usize..],
        &after[HEAD_LEN as usize..],
        "a rewrite of an untouched file reproduces its payload byte for byte"
    );
    assert_eq!(
        ColumnFile::open(&original).unwrap().content_id(),
        ColumnFile::open(&rewritten).unwrap().content_id(),
        "and its content id, which is what identifies the data"
    );
    assert_ne!(
        ColumnFile::open(&original).unwrap().file_uuid(),
        ColumnFile::open(&rewritten).unwrap().file_uuid(),
        "but they are distinct file bodies"
    );
}

#[test]
fn the_test_encoder_agrees_with_the_layout_constants() {
    use crate::test_utils::column_file_bytes as model;

    assert_eq!(model::HEAD_LEN, HEAD_LEN);
    assert_eq!(model::SLOT_A, SLOT_A);
    assert_eq!(model::SLOT_B, SLOT_B);
    assert_eq!(model::APPEND_MARK, APPEND_MARK);
    assert_eq!(model::MAGIC, MAGIC);
    assert_eq!(model::SLOT_MAGIC, SLOT_MAGIC);
    assert_eq!(model::MARK_MAGIC, MARK_MAGIC);
    assert_eq!(model::VERSION, FORMAT_VERSION);
}

#[test]
fn a_file_laid_out_by_the_test_encoder_reads_back_through_the_real_reader() {
    use crate::test_utils::column_file_bytes as model;

    let dir = TempDir::new("model_file");
    let path = dir.join("model.dbs");
    let image = model::FileModel::of(vec![model::Column::of("values", 4, 4, &[4, 4, 2])]).bytes();
    std::fs::write(&path, &image).unwrap();

    let file = ColumnFile::open(&path).unwrap();
    file.verify().unwrap();
    assert_eq!(file.generation(), 1);

    let column = file.column::<u32>("values").unwrap();
    assert_eq!(column.len(), 10);
    assert_eq!(column.chunk_count(), 3);
}

#[derive(Clone, Copy, Debug)]
enum Lie {
    ChunkRowsZero,
    TailChunkTooLong,
    RowsDisagreeWithChunks,
    ElemSizeZero,
    ElemSizeHuge,
    ColumnCountTooMany,
    ColumnCountTooFew,
    DirOffsetBelowHead,
    DirOffsetPastEnd,
    DirLenHuge,
    ChunkOffsetIntoHead,
    ChunkOffsetPastDirectory,
    ChunkOverlap,
    ByteLenMax,
    ByteLenDisagreesWithRows,
    ChunkChecksumWrong,
    GenerationZero,
    CodecUnknown,
    VersionFromTheFuture,
    ForeignMagic,
    ForeignUuid,
}

fn a_lie() -> impl Strategy<Value = Lie> {
    prop::sample::select(vec![
        Lie::ChunkRowsZero,
        Lie::TailChunkTooLong,
        Lie::RowsDisagreeWithChunks,
        Lie::ElemSizeZero,
        Lie::ElemSizeHuge,
        Lie::ColumnCountTooMany,
        Lie::ColumnCountTooFew,
        Lie::DirOffsetBelowHead,
        Lie::DirOffsetPastEnd,
        Lie::DirLenHuge,
        Lie::ChunkOffsetIntoHead,
        Lie::ChunkOffsetPastDirectory,
        Lie::ChunkOverlap,
        Lie::ByteLenMax,
        Lie::ByteLenDisagreesWithRows,
        Lie::ChunkChecksumWrong,
        Lie::GenerationZero,
        Lie::CodecUnknown,
        Lie::VersionFromTheFuture,
        Lie::ForeignMagic,
        Lie::ForeignUuid,
    ])
}

fn told(mut file: model::FileModel, lie: Lie) -> model::FileModel {
    let column = &mut file.columns[0];
    match lie {
        Lie::ChunkRowsZero => column.chunk_rows = 0,
        Lie::TailChunkTooLong => {
            let rows = column.chunk_rows as u32 + 3;
            let elem_size = column.elem_size;
            if let Some(tail) = column.chunks.last_mut() {
                *tail = model::Chunk::of(rows, elem_size);
            }
        }
        Lie::RowsDisagreeWithChunks => column.rows = Some(9_999),
        Lie::ElemSizeZero => column.elem_size = 0,
        Lie::ElemSizeHuge => column.elem_size = u32::MAX,
        Lie::ColumnCountTooMany => file.column_count = Some(64),
        Lie::ColumnCountTooFew => file.column_count = Some(0),
        Lie::DirOffsetBelowHead => file.dir_offset = Some(64),
        Lie::DirOffsetPastEnd => file.dir_offset = Some(model::HEAD_LEN * 64),
        Lie::DirLenHuge => file.dir_len = Some(u64::MAX / 2),
        Lie::ChunkOffsetIntoHead => column.chunks[0].offset = Some(128),
        Lie::ChunkOffsetPastDirectory => {
            column.chunks[0].offset = Some(model::HEAD_LEN * 1024);
        }
        Lie::ChunkOverlap => {
            let first = model::HEAD_LEN;
            for chunk in &mut column.chunks {
                chunk.offset = Some(first);
            }
        }
        Lie::ByteLenMax => column.chunks[0].byte_len = Some(u32::MAX),
        Lie::ByteLenDisagreesWithRows => column.chunks[0].byte_len = Some(3),
        Lie::ChunkChecksumWrong => column.chunks[0].fnv = Some(0xdead_beef),
        Lie::GenerationZero => file.generation = 0,
        Lie::CodecUnknown => column.codec = 7,
        Lie::VersionFromTheFuture => file.version = 99,
        Lie::ForeignMagic => file.magic = *b"XXXX",
        Lie::ForeignUuid => file.file_uuid ^= 1,
    }
    file
}

proptest! {
    #[test]
    fn a_self_consistent_lie_is_caught_by_a_guard_and_never_by_a_crash(
        lie in a_lie(),
        chunks in 1usize..4,
        chunk_rows in 1u64..6,
        tail in 1u32..7,
    ) {
        let dir = TempDir::new("hostile");
        let path = dir.join("hostile.dbs");
        let honest_path = dir.join("honest.dbs");

        let mut counts = vec![chunk_rows as u32; chunks - 1];
        counts.push(tail.min(chunk_rows as u32));
        let honest = model::FileModel::of(vec![model::Column::of(
            "values",
            4,
            chunk_rows,
            &counts,
        )]);

        std::fs::write(&honest_path, honest.bytes()).unwrap();
        prop_assert!(
            ColumnFile::open(&honest_path).is_ok(),
            "the base file must be one the reader accepts, or every lie lands on the wrong guard"
        );

        std::fs::write(&path, told(honest, lie).bytes()).unwrap();

        let Ok(file) = ColumnFile::open(&path) else {
            return Ok(());
        };
        let _ = file.verify();
        let _ = ColumnFile::read_commit_token(&path);
        let _ = file.live_bytes();

        let Ok(column) = file.column::<u32>("values") else {
            return Ok(());
        };
        for at in 0..column.len() {
            let _ = column.get(at);
        }
        let _ = column.range(0, column.len());
        let _ = column.prefetch();
    }
}
