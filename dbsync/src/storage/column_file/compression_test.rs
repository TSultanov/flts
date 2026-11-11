use super::*;
use proptest::prelude::*;

fn location(bytes: &[u8], record: &[u8]) -> ChunkLoc {
    ChunkLoc {
        offset: 0,
        byte_len: bytes.len() as u32,
        rows: bytes.len() as u32,
        fnv: crate::fnv1a(bytes),
        stored_len: record.len() as u32,
        depth: 0,
    }
}

proptest! {
    #[test]
    fn arbitrary_payloads_round_trip(bytes in prop::collection::vec(any::<u8>(), 0..65537)) {
        let record = encode(&bytes, bytes.len() as u32, 0).unwrap();
        prop_assert_eq!(decode(location(&bytes, &record), &record).unwrap(), bytes);
    }
}

#[test]
fn empty_tiny_and_incompressible_payloads_are_always_zstd() {
    use rand::{RngCore, SeedableRng};
    let mut random = vec![0; 65536];
    rand::rngs::StdRng::seed_from_u64(42).fill_bytes(&mut random);
    for bytes in [vec![], vec![1], random] {
        let record = encode(&bytes, bytes.len() as u32, 0).unwrap();
        assert_eq!(&record[HEADER_LEN..][..4], &[0x28, 0xb5, 0x2f, 0xfd]);
        assert!(record.len() > bytes.len());
        assert_eq!(decode(location(&bytes, &record), &record).unwrap(), bytes);
    }
}

#[test]
fn repetitive_payloads_compress_substantially() {
    let bytes = vec![7; 65536];
    let record = encode(&bytes, bytes.len() as u32, 0).unwrap();
    assert!(record.len() < bytes.len() / 100);
    assert_eq!(decode(location(&bytes, &record), &record).unwrap(), bytes);
}

#[test]
fn truncated_corrupt_and_mismatched_records_are_rejected() {
    let bytes = vec![7; 65536];
    let record = encode(&bytes, bytes.len() as u32, 0).unwrap();
    let loc = location(&bytes, &record);
    for end in 0..record.len() {
        let mut truncated = loc;
        truncated.stored_len = end as u32;
        assert!(decode(truncated, &record[..end]).unwrap_err().is_corrupt());
    }
    for at in 0..record.len() {
        let mut damaged = record.clone();
        damaged[at] ^= 1;
        assert!(decode(loc, &damaged).unwrap_err().is_corrupt(), "byte {at}");
    }
    let mut huge = record.clone();
    huge[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    huge[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut declared = loc;
    declared.byte_len = u32::MAX;
    assert!(decode(declared, &huge).unwrap_err().is_corrupt());
}

#[test]
fn raw_bytes_trailing_bytes_and_concatenated_frames_are_rejected() {
    let bytes = b"an uncompressed legacy payload";
    assert!(
        decode(location(bytes, bytes), bytes)
            .unwrap_err()
            .is_corrupt()
    );
    let record = encode(bytes, bytes.len() as u32, 0).unwrap();
    for tail in [vec![0], record[HEADER_LEN..].to_vec()] {
        let mut extra = record.clone();
        extra.extend_from_slice(&tail);
        assert!(
            decode(location(bytes, &extra), &extra)
                .unwrap_err()
                .is_corrupt()
        );
    }
}
