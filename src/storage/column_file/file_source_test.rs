use super::*;

use crate::test_utils::TempDir;

#[test]
fn reads_at_an_offset_without_a_cursor() {
    let dir = TempDir::new("file_source");
    let path = dir.join("bytes.bin");
    std::fs::write(&path, b"0123456789").unwrap();

    let source = FileSource::open(&path).unwrap();
    assert_eq!(source.len().unwrap(), 10);

    let mut buf = [0u8; 4];
    source.read_at(&mut buf, 3).unwrap();
    assert_eq!(&buf, b"3456");

    source.read_at(&mut buf, 6).unwrap();
    assert_eq!(&buf, b"6789");
}

#[test]
fn a_chunk_read_returns_exactly_the_located_bytes() {
    let dir = TempDir::new("file_source_chunk");
    let path = dir.join("bytes.bin");
    std::fs::write(&path, b"0123456789").unwrap();

    let source = FileSource::open(&path).unwrap();
    let loc = ChunkLoc {
        stored_len: 0,
        depth: 0,
        offset: 2,
        byte_len: 3,
        rows: 3,
        fnv: crate::fnv1a(b"234"),
    };
    assert_eq!(source.read_chunk(loc).unwrap(), b"234");
}

#[test]
fn reading_past_the_end_is_an_error() {
    let dir = TempDir::new("file_source_eof");
    let path = dir.join("bytes.bin");
    std::fs::write(&path, b"0123").unwrap();

    let source = FileSource::open(&path).unwrap();
    let mut buf = [0u8; 8];
    assert!(source.read_at(&mut buf, 0).is_err());
    assert!(source.read_at(&mut buf, 100).is_err());
}
