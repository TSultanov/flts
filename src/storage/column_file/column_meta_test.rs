use super::*;
use crate::test_utils::props;
use proptest::prelude::*;

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
fn a_chunk_that_claims_no_bytes_cannot_overlap_anything() {
    let mut empty_inside = sample();
    empty_inside[1].chunks = vec![ChunkLoc {
        offset: FIRST + 8,
        byte_len: 0,
        rows: 0,
        fnv: 0,
    }];

    let read = read_directory(&write_directory(&empty_inside), DIRECTORY).unwrap();
    assert_eq!(read[1].chunks[0].byte_len, 0);
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

fn a_directory() -> impl Strategy<Value = (Vec<ColumnMeta>, u64)> {
    prop::collection::vec(
        (
            "[a-z.]{0,10}",
            1u32..=8,
            prop::collection::vec(0u32..6, 0..4),
        ),
        0..4,
    )
    .prop_map(|specs| {
        let mut at = HEAD_LEN;
        let mut columns = Vec::new();

        for (index, (name, elem_size, counts)) in specs.into_iter().enumerate() {
            let mut chunks = Vec::new();
            let mut rows = 0u64;
            for count in counts {
                let byte_len = count * elem_size;
                chunks.push(ChunkLoc {
                    offset: at,
                    byte_len,
                    rows: count,
                    fnv: u64::from(count) * 0x11,
                });
                at += u64::from(byte_len);
                rows += u64::from(count);
            }

            columns.push(ColumnMeta {
                name: format!("{index}{name}"),
                elem_size,
                chunk_rows: 8,
                rows,
                codec: Codec::Raw,
                chunks,
            });
        }

        (columns, at)
    })
}

proptest! {
    #[test]
    fn a_directory_round_trips_every_column_it_describes((columns, dir_offset) in a_directory()) {
        let read = read_directory(&write_directory(&columns), dir_offset).unwrap();

        prop_assert_eq!(read.len(), columns.len());
        for (read, written) in read.iter().zip(&columns) {
            prop_assert_eq!(&read.name, &written.name);
            prop_assert_eq!(read.elem_size, written.elem_size);
            prop_assert_eq!(read.chunk_rows, written.chunk_rows);
            prop_assert_eq!(read.rows, written.rows);
            prop_assert_eq!(&read.chunks, &written.chunks);
        }
    }

    #[test]
    fn content_id_does_not_depend_on_the_order_columns_were_offered(
        (columns, _) in a_directory(),
        rotation in any::<prop::sample::Index>(),
    ) {
        prop_assume!(!columns.is_empty());

        let mut shuffled = columns.clone();
        shuffled.rotate_left(rotation.index(columns.len()));

        prop_assert_eq!(content_id(&shuffled), content_id(&columns));
    }

    #[test]
    fn content_id_ignores_where_chunks_live_but_not_what_they_hold(
        (columns, _) in a_directory(),
        shift in 1u64..4096,
    ) {
        prop_assume!(columns.iter().any(|column| !column.chunks.is_empty()));
        let was = content_id(&columns);

        let mut moved = columns.clone();
        for column in &mut moved {
            for chunk in &mut column.chunks {
                chunk.offset += shift;
            }
        }
        prop_assert_eq!(content_id(&moved), was);

        let mut relabelled = columns.clone();
        relabelled[0].name.push('!');
        prop_assert_ne!(content_id(&relabelled), was);

        let mut rechunked = columns.clone();
        for column in &mut rechunked {
            for chunk in &mut column.chunks {
                chunk.fnv ^= 0xff;
            }
        }
        prop_assert_ne!(content_id(&rechunked), was);
    }

    #[test]
    fn two_columns_can_never_claim_the_same_byte(
        (columns, dir_offset) in a_directory(),
        overlap in 1u64..32,
    ) {
        prop_assert!(read_directory(&write_directory(&columns), dir_offset).is_ok());

        let occupied: Vec<(usize, usize)> = columns
            .iter()
            .enumerate()
            .flat_map(|(column, meta)| {
                meta.chunks
                    .iter()
                    .enumerate()
                    .filter(|(_, chunk)| chunk.byte_len > 0)
                    .map(move |(chunk, _)| (column, chunk))
            })
            .collect();
        prop_assume!(occupied.len() >= 2);

        let (column, chunk) = occupied[occupied.len() - 1];
        let mut clashing = columns.clone();
        let moved = &mut clashing[column].chunks[chunk];
        moved.offset -= overlap.min(moved.offset - HEAD_LEN);

        let err = read_directory(&write_directory(&clashing), dir_offset).unwrap_err();
        prop_assert!(err.to_string().contains("overlap"), "{err}");
        prop_assert!(err.is_corrupt());
    }

    #[test]
    fn read_directory_refuses_every_directory_the_writer_could_not_have_written(
        bytes in prop::collection::vec(any::<u8>(), 0..256),
        dir_offset in props::interesting_u64(),
    ) {
        if let Ok(columns) = read_directory(&bytes, dir_offset) {
            for column in &columns {
                prop_assert!(column.chunk_rows > 0);
                for chunk in &column.chunks {
                    prop_assert!(chunk.offset >= HEAD_LEN);
                    prop_assert!(chunk.offset + u64::from(chunk.byte_len) <= dir_offset);
                }
            }
        }
    }
}
