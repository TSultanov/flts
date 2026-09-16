use super::*;

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
                    offset: 8,
                    byte_len: 32,
                    rows: 4,
                    fnv: 0x1111,
                },
                ChunkLoc {
                    offset: 40,
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
    let read = read_directory(&bytes, 1000).unwrap();

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

    let err = read_directory(&bytes, 40).unwrap_err();
    assert!(
        err.to_string().contains("outside the payload region"),
        "{err}"
    );

    let mut below_header = sample();
    below_header[0].chunks[0].offset = 1;
    let bytes = write_directory(&below_header);
    let err = read_directory(&bytes, 1000).unwrap_err();
    assert!(
        err.to_string().contains("outside the payload region"),
        "{err}"
    );
}

#[test]
fn a_truncated_directory_is_an_error() {
    let bytes = write_directory(&sample());
    let err = read_directory(&bytes[..bytes.len() - 4], 1000).unwrap_err();
    assert!(err.to_string().contains("ends mid-record"), "{err}");
}
