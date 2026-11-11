use super::*;
use proptest::prelude::*;

fn base(bytes: &[u8]) -> ChunkLoc {
    ChunkLoc {
        offset: super::super::HEAD_LEN,
        byte_len: bytes.len() as u32,
        rows: bytes.len() as u32,
        fnv: fnv1a(bytes),
        stored_len: bytes.len() as u32,
        depth: 0,
    }
}
fn location(before: &[u8], after: &[u8], delta: &[u8]) -> ChunkLoc {
    ChunkLoc {
        offset: super::super::HEAD_LEN + before.len() as u64,
        byte_len: after.len() as u32,
        rows: after.len() as u32,
        fnv: fnv1a(after),
        stored_len: delta.len() as u32,
        depth: 1,
    }
}
fn resign(bytes: &mut [u8]) {
    let end = bytes.len() - 8;
    let checksum = fnv1a(&bytes[..end]);
    bytes[end..].copy_from_slice(&checksum.to_le_bytes());
}

proptest! {
    #[test]
    fn sparse_ranges_round_trip_with_growth_and_truncation(
        length in 512usize..2048,
        change in -128i32..128,
        edits in prop::collection::vec((0usize..4096, any::<u8>()), 0..12),
        width in prop::sample::select(vec![1usize, 4, 8]),
    ) {
        let before = vec![13; length];
        let mut after = before.clone();
        after.resize((length as i32 + change) as usize, 7);
        let size = after.len();
        for (at, value) in edits { after[at % size] = value; }
        if let Some(delta) = encode(base(&before), &before, &after, width) {
            let (found, cursor) = parse(location(&before, &after, &delta), &delta).unwrap();
            prop_assert_eq!(found, base(&before));
            prop_assert_eq!(apply(before, after.len(), cursor).unwrap(), after);
        }
    }
}

#[test]
fn invalid_ranges_and_forward_bases_fail_even_with_valid_checksums() {
    let before = vec![0; 4096];
    let mut after = before.clone();
    after[0] = 1;
    let delta = encode(base(&before), &before, &after, 1).unwrap();
    let loc = location(&before, &after, &delta);
    let mut bad_range = delta.clone();
    bad_range[42..46].copy_from_slice(&u32::MAX.to_le_bytes());
    resign(&mut bad_range);
    let (_, cursor) = parse(loc, &bad_range).unwrap();
    assert!(apply(before, after.len(), cursor).is_err());

    let mut forward = delta;
    forward[9..17].copy_from_slice(&loc.offset.to_le_bytes());
    resign(&mut forward);
    assert!(parse(loc, &forward).is_err());
}
