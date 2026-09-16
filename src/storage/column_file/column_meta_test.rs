use super::*;

const FIRST: u64 = HEAD_LEN;
const SECOND: u64 = HEAD_LEN + 32;
const DIRECTORY: u64 = HEAD_LEN + 4096;

fn sample() -> Vec<ColumnMeta> {
    vec![
        ColumnMeta {
            name: "paragraph.chapter".to_owned(),
            elem_size: 8,
            chunk_rows: 4,
            rows: 6,
            codec: Codec::Raw,
            chunks: vec![
                ChunkLoc {
                    offset: FIRST,
                    byte_len: 32,
                    rows: 4,
                    fnv: 0x1111,
                },
                ChunkLoc {
                    offset: SECOND,
                    byte_len: 16,
                    rows: 2,
                    fnv: 0x2222,
                },
            ],
        },
        ColumnMeta {
            name: "bytes".to_owned(),
            elem_size: 1,
            chunk_rows: 64,
            rows: 0,
            codec: Codec::Raw,
            chunks: Vec::new(),
        },
    ]
}

#[test]
fn the_directory_round_trips() {
    let written = sample();
    let bytes = write_directory(&written);
    let read = read_directory(&bytes, DIRECTORY).unwrap();

    assert_eq!(read.len(), 2);
    assert_eq!(read[0].name, "paragraph.chapter");
    assert_eq!(read[0].elem_size, 8);
    assert_eq!(read[0].chunk_rows, 4);
    assert_eq!(read[0].rows, 6);
    assert_eq!(read[0].chunks, written[0].chunks);
    assert_eq!(read[1].name, "bytes");
    assert!(read[1].chunks.is_empty());
}

#[test]
fn byte_len_sums_the_chunks() {
    assert_eq!(sample()[0].byte_len(), 48);
    assert_eq!(sample()[1].byte_len(), 0);
}

#[test]
fn a_chunk_outside_the_payload_region_is_rejected() {
    let bytes = write_directory(&sample());

    let err = read_directory(&bytes, FIRST + 16).unwrap_err();
    assert!(
        err.to_string().contains("outside the payload region"),
        "{err}"
    );

    let mut below_header = sample();
    below_header[0].chunks[0].offset = HEAD_LEN - 1;
    let bytes = write_directory(&below_header);
    let err = read_directory(&bytes, DIRECTORY).unwrap_err();
    assert!(
        err.to_string().contains("outside the payload region"),
        "{err}"
    );
}

#[test]
fn a_truncated_directory_is_an_error() {
    let bytes = write_directory(&sample());
    let err = read_directory(&bytes[..bytes.len() - 4], DIRECTORY).unwrap_err();
    assert!(err.to_string().contains("ends mid-record"), "{err}");
}

#[test]
fn a_chunk_range_that_overflows_is_rejected() {
    let mut overflowing = sample();
    overflowing[0].chunks[0].offset = u64::MAX - 4;
    overflowing[0].chunks[0].byte_len = 32;
    let bytes = write_directory(&overflowing);

    let err = read_directory(&bytes, DIRECTORY).unwrap_err();
    assert!(err.to_string().contains("overflows"), "{err}");
}

#[test]
fn overlapping_chunks_are_rejected() {
    let mut overlapping = sample();
    overlapping[1].chunks = vec![ChunkLoc {
        offset: FIRST + 12,
        byte_len: 32,
        rows: 32,
        fnv: 0x3333,
    }];
    let bytes = write_directory(&overlapping);

    let err = read_directory(&bytes, DIRECTORY).unwrap_err();
    assert!(err.to_string().contains("overlap at offset"), "{err}");
}

#[test]
fn adjacent_chunks_are_not_overlapping() {
    let bytes = write_directory(&sample());
    let read = read_directory(&bytes, DIRECTORY).unwrap();
    assert_eq!(read[0].chunks[0].offset + 32, read[0].chunks[1].offset);
}

fn one_column_claiming(chunk_count: u32) -> Vec<u8> {
    let mut bytes = 1u32.to_le_bytes().to_vec();
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.push(b'a');
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&4u64.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.push(Codec::Raw as u8);
    bytes.extend_from_slice(&chunk_count.to_le_bytes());
    bytes
}

#[test]
fn the_hand_built_directory_is_one_the_reader_accepts() {
    let read = read_directory(&one_column_claiming(0), DIRECTORY).unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].name, "a");
    assert!(read[0].chunks.is_empty());
}

#[test]
fn a_directory_claiming_four_billion_columns_is_rejected_without_reserving_for_them() {
    let mut lying = u32::MAX.to_le_bytes().to_vec();
    lying.extend_from_slice(&write_directory(&sample())[4..]);

    let err = read_directory(&lying, DIRECTORY).unwrap_err();
    assert!(err.to_string().contains("ends mid-record"), "{err}");
}

#[test]
fn a_column_claiming_four_billion_chunks_is_rejected_without_reserving_for_them() {
    let err = read_directory(&one_column_claiming(u32::MAX), DIRECTORY).unwrap_err();
    assert!(err.to_string().contains("ends mid-record"), "{err}");
}

#[test]
fn a_column_chunked_zero_rows_at_a_time_is_rejected() {
    let mut unchunked = sample();
    unchunked[0].chunk_rows = 0;
    let bytes = write_directory(&unchunked);

    let err = read_directory(&bytes, DIRECTORY).unwrap_err();
    assert!(err.to_string().contains("zero rows at a time"), "{err}");
    assert!(err.is_corrupt());
}
