use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingSource {
    chunks: Vec<Vec<u8>>,
    reads: AtomicUsize,
}

impl ChunkSource for CountingSource {
    fn read_chunk(&self, loc: ChunkLoc) -> io::Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.chunks[loc.offset as usize].clone())
    }
}

fn lazy_u32(rows: u32, chunk_rows: usize) -> (Arena<u32>, Arc<CountingSource>) {
    let mut chunks = Vec::new();
    let mut locations = Vec::new();
    for start in (0..rows).step_by(chunk_rows) {
        let end = (start + chunk_rows as u32).min(rows);
        let mut bytes = Vec::new();
        for value in start..end {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        locations.push(ChunkLoc {
            offset: chunks.len() as u64,
            byte_len: bytes.len() as u32,
            rows: end - start,
            fnv: fnv1a(&bytes),
        });
        chunks.push(bytes);
    }

    let source = Arc::new(CountingSource {
        chunks,
        reads: AtomicUsize::new(0),
    });
    let arena =
        Arena::from_locations(locations, chunk_rows, Provenance::new(source.clone(), 1)).unwrap();
    (arena, source)
}

#[test]
fn chunk_rows_target_a_fixed_byte_budget() {
    assert_eq!(default_chunk_rows(1), TARGET_CHUNK_BYTES);
    assert_eq!(default_chunk_rows(8), TARGET_CHUNK_BYTES / 8);
    assert_eq!(default_chunk_rows(TARGET_CHUNK_BYTES * 2), 1);
    assert_eq!(default_chunk_rows(0), 1);
}

#[test]
fn push_and_read_back_across_chunk_boundaries() {
    let mut arena = Arena::<u32>::with_chunk_rows(4);
    for value in 0..10u32 {
        assert_eq!(arena.push(value), value as usize);
    }
    assert_eq!(arena.len(), 10);
    assert_eq!(arena.chunk_count(), 3);
    for value in 0..10u32 {
        assert_eq!(arena.get(value as usize), Some(&value));
    }
    assert_eq!(arena.get(10), None);
}

#[test]
fn ranges_borrow_inside_a_chunk_and_copy_across_one() {
    let mut arena = Arena::<u32>::with_chunk_rows(4);
    arena.extend_from_slice(&(0..10).collect::<Vec<u32>>());

    let inside = arena.try_range(1, 3).unwrap();
    assert!(matches!(inside, Cow::Borrowed(_)));
    assert_eq!(&*inside, &[1, 2, 3]);

    let across = arena.try_range(2, 6).unwrap();
    assert!(matches!(across, Cow::Owned(_)));
    assert_eq!(&*across, &[2, 3, 4, 5, 6, 7]);

    assert!(arena.try_range(8, 5).is_err());
    assert!(arena.try_range(0, 0).unwrap().is_empty());
}

#[test]
fn extend_fills_partial_chunks_before_opening_new_ones() {
    let mut arena = Arena::<u32>::with_chunk_rows(4);
    arena.extend_from_slice(&[0, 1]);
    arena.extend_from_slice(&[2, 3, 4, 5, 6]);
    assert_eq!(arena.chunk_count(), 2);
    assert_eq!(arena.chunk(0).unwrap(), &[0, 1, 2, 3]);
    assert_eq!(arena.chunk(1).unwrap(), &[4, 5, 6]);
    assert_eq!(
        arena.try_iter().collect::<io::Result<Vec<_>>>().unwrap(),
        (0..7u32).collect::<Vec<_>>().iter().collect::<Vec<_>>()
    );
}

#[test]
fn a_frozen_view_shares_chunks_and_a_write_copies_only_the_tail() {
    let mut live = Arena::<u32>::with_chunk_rows(4);
    live.extend_from_slice(&(0..9).collect::<Vec<u32>>());
    let frozen = live.clone();

    assert_eq!(live.chunk_count(), 3);
    for index in 0..3 {
        assert_eq!(
            Arc::strong_count(&live.chunks[index]),
            2,
            "chunk {index} is shared"
        );
    }

    live.push(99);

    assert_eq!(Arc::strong_count(&live.chunks[0]), 2);
    assert_eq!(Arc::strong_count(&live.chunks[1]), 2);
    assert_eq!(Arc::strong_count(&live.chunks[2]), 1);

    assert_eq!(frozen.len(), 9);
    assert_eq!(frozen.get(8), Some(&8));
    assert_eq!(live.len(), 10);
    assert_eq!(live.get(9), Some(&99));
}

#[test]
fn setting_a_row_leaves_a_frozen_view_alone() {
    let mut live = Arena::<u32>::with_chunk_rows(4);
    live.extend_from_slice(&(0..9).collect::<Vec<u32>>());
    let frozen = live.clone();

    live.try_set(1, 111).unwrap();
    assert_eq!(live.get(1), Some(&111));
    assert_eq!(frozen.get(1), Some(&1));
    assert_eq!(
        Arc::strong_count(&live.chunks[1]),
        2,
        "untouched chunk stays shared"
    );
    assert!(live.try_set(99, 0).is_err());
}

#[test]
fn opening_a_lazy_arena_reads_nothing_until_a_row_is_asked_for() {
    let (arena, source) = lazy_u32(10, 4);
    assert_eq!(arena.len(), 10);
    assert_eq!(arena.chunk_count(), 3);
    assert_eq!(arena.resident_chunks(), 0);
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);

    assert_eq!(arena.try_get(5).unwrap(), Some(&5));
    assert_eq!(arena.resident_chunks(), 1, "only the touched chunk loaded");
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);

    assert_eq!(arena.try_get(4).unwrap(), Some(&4));
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);

    arena.prefetch().unwrap();
    assert_eq!(arena.resident_chunks(), 3);
    assert_eq!(source.reads.load(Ordering::SeqCst), 3);
}

#[test]
fn a_load_in_one_view_is_visible_to_every_view_sharing_the_chunk() {
    let (live, source) = lazy_u32(10, 4);
    let frozen = live.clone();

    assert_eq!(frozen.try_get(0).unwrap(), Some(&0));
    assert_eq!(live.try_get(0).unwrap(), Some(&0));
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        1,
        "the two views share the load"
    );
}

#[test]
fn appending_to_a_lazy_arena_faults_in_only_the_tail_chunk() {
    let (mut arena, source) = lazy_u32(10, 4);
    arena.push(42);

    assert_eq!(arena.len(), 11);
    assert_eq!(arena.resident_chunks(), 1);
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        1,
        "only the short tail was read"
    );
    assert_eq!(arena.try_get(10).unwrap(), Some(&42));
    assert_eq!(arena.try_get(9).unwrap(), Some(&9));
}

#[test]
fn a_corrupt_chunk_is_rejected_rather_than_decoded() {
    let (arena, _) = lazy_u32(10, 4);
    let mut locations: Vec<ChunkLoc> = (0..3)
        .map(|index| ChunkLoc {
            offset: index,
            byte_len: if index == 2 { 8 } else { 16 },
            rows: if index == 2 { 2 } else { 4 },
            fnv: 0xdead_beef,
        })
        .collect();
    locations[0].fnv = 0xdead_beef;

    let source = Arc::new(CountingSource {
        chunks: (0..3)
            .map(|index| arena.chunk_bytes(index).unwrap())
            .collect(),
        reads: AtomicUsize::new(0),
    });
    let tampered = Arena::<u32>::from_locations(locations, 4, Provenance::new(source, 1)).unwrap();

    let err = tampered.try_get(0).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("failed checksum"), "{err}");
}

#[test]
fn chunk_geometry_is_validated_when_a_lazy_arena_is_built() {
    let source = Arc::new(CountingSource {
        chunks: Vec::new(),
        reads: AtomicUsize::new(0),
    });
    let short_in_the_middle = vec![
        ChunkLoc {
            offset: 0,
            byte_len: 8,
            rows: 2,
            fnv: 0,
        },
        ChunkLoc {
            offset: 1,
            byte_len: 16,
            rows: 4,
            fnv: 0,
        },
    ];
    let err =
        Arena::<u32>::from_locations(short_in_the_middle, 4, Provenance::new(source.clone(), 1))
            .unwrap_err();
    assert!(
        err.to_string().contains("only the last chunk may be short"),
        "{err}"
    );

    let wrong_width = vec![ChunkLoc {
        offset: 0,
        byte_len: 9,
        rows: 2,
        fnv: 0,
    }];
    let err = Arena::<u32>::from_locations(wrong_width, 4, Provenance::new(source, 1)).unwrap_err();
    assert!(err.to_string().contains("bytes for"), "{err}");
}

#[test]
fn byte_arenas_hand_out_blobs_that_read_back_as_text() {
    let mut bytes = Arena::<u8>::with_chunk_rows(8);
    let hello = bytes.push_str("hello");
    let long = bytes.push_str("a string that spans several chunks");
    let empty = bytes.push_str("");

    assert_eq!(&*bytes.try_str(hello).unwrap(), "hello");
    assert_eq!(
        &*bytes.try_str(long).unwrap(),
        "a string that spans several chunks"
    );
    assert_eq!(&*bytes.try_str(empty).unwrap(), "");
    assert_eq!(hello.start, 0);
    assert_eq!(long.start, 5);
}

#[test]
fn invalid_utf8_is_replaced_rather_than_failing_the_read() {
    let mut bytes = Arena::<u8>::new();
    let blob = bytes.push_bytes(&[0xff, 0xfe]);
    assert_eq!(&*bytes.try_str(blob).unwrap(), "\u{fffd}\u{fffd}");
}

#[test]
fn writing_out_an_untouched_chunk_does_not_make_it_resident() {
    let (arena, source) = lazy_u32(10, 4);

    let expected: Vec<u8> = (0..4u32).flat_map(|v| v.to_le_bytes()).collect();
    assert_eq!(arena.chunk_bytes(0).unwrap(), expected);

    assert_eq!(
        arena.resident_chunks(),
        0,
        "copied through without decoding"
    );
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);

    for index in 0..arena.chunk_count() {
        arena.chunk_bytes(index).unwrap();
    }
    assert_eq!(arena.resident_chunks(), 0);
}

#[test]
fn writing_out_a_mutated_chunk_uses_its_rows_not_the_stale_disk_bytes() {
    let (mut arena, _) = lazy_u32(10, 4);
    arena.try_set(1, 999).unwrap();

    let expected: Vec<u8> = [0u32, 999, 2, 3]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    assert_eq!(arena.chunk_bytes(0).unwrap(), expected);

    let untouched: Vec<u8> = (4..8u32).flat_map(|v| v.to_le_bytes()).collect();
    assert_eq!(arena.chunk_bytes(1).unwrap(), untouched);
    assert_eq!(arena.resident_chunks(), 1, "only the mutated chunk is held");
}

#[test]
fn a_corrupt_chunk_is_not_laundered_into_a_new_file() {
    let (good, _) = lazy_u32(8, 4);
    let chunks: Vec<Vec<u8>> = (0..2).map(|i| good.chunk_bytes(i).unwrap()).collect();
    let locations = vec![
        ChunkLoc {
            offset: 0,
            byte_len: 16,
            rows: 4,
            fnv: 0xdead_beef,
        },
        ChunkLoc {
            offset: 1,
            byte_len: 16,
            rows: 4,
            fnv: fnv1a(&chunks[1]),
        },
    ];
    let source = Arc::new(CountingSource {
        chunks,
        reads: AtomicUsize::new(0),
    });
    let arena = Arena::<u32>::from_locations(locations, 4, Provenance::new(source, 1)).unwrap();

    let err = arena.chunk_bytes(0).unwrap_err();
    assert!(err.to_string().contains("failed checksum"), "{err}");
    assert!(arena.chunk_bytes(1).is_ok());
}

#[test]
fn a_mutated_chunk_stops_reporting_a_disk_location() {
    let (mut arena, _) = lazy_u32(10, 4);
    assert!(
        arena
            .chunks
            .iter()
            .all(|cell| cell.still_on_disk().is_some())
    );

    arena.try_set(0, 5).unwrap();
    assert!(arena.chunks[0].still_on_disk().is_none());
    assert!(arena.chunks[1].still_on_disk().is_some());

    arena.try_push(42).unwrap();
    assert!(arena.chunks[2].still_on_disk().is_none());
}

#[test]
fn writing_out_a_resident_chunk_does_not_read_it_again() {
    let (arena, source) = lazy_u32(10, 4);
    arena.try_get(0).unwrap();
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);

    arena.chunk_bytes(0).unwrap();
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);
    assert_eq!(arena.resident_chunks(), 1);
}

#[test]
fn taking_owned_rows_twice_is_idempotent() {
    let (mut arena, source) = lazy_u32(10, 4);
    arena.try_set(0, 111).unwrap();
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);

    arena.try_set(1, 222).unwrap();
    arena.try_set(2, 333).unwrap();
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);

    let cell = Arc::make_mut(&mut arena.chunks[0]);
    assert_eq!(cell.owned_rows().as_slice(), &[111, 222, 333, 3]);
    assert_eq!(cell.owned_rows().as_slice(), &[111, 222, 333, 3]);
}

#[test]
fn reading_a_chunk_does_not_cost_it_its_reusable_location() {
    let (arena, _) = lazy_u32(10, 4);
    assert!(arena.reusable_loc(0).is_some());

    arena.try_get(0).unwrap();
    assert_eq!(arena.resident_chunks(), 1);
    assert!(
        arena.reusable_loc(0).is_some(),
        "a chunk that was only read is unchanged on disk"
    );
    assert!(
        arena.chunks[0].still_on_disk().is_none(),
        "but disk is no longer the cheapest way to get its bytes"
    );
}

#[test]
fn a_mutated_chunk_has_no_reusable_location() {
    let (mut arena, _) = lazy_u32(10, 4);
    arena.prefetch().unwrap();
    assert!(arena.reusable_loc(0).is_some());

    arena.try_set(0, 5).unwrap();
    assert!(arena.reusable_loc(0).is_none());
    assert!(arena.reusable_loc(1).is_some());
    assert!(arena.reusable_loc(99).is_none());
}

#[test]
fn provenance_identifies_the_source_and_generation() {
    let (arena, source) = lazy_u32(10, 4);
    let same: Arc<dyn ChunkSource> = source.clone();
    let other: Arc<dyn ChunkSource> = Arc::new(CountingSource {
        chunks: Vec::new(),
        reads: AtomicUsize::new(0),
    });

    let provenance = arena.provenance().unwrap();
    assert_eq!(provenance.generation(), 1);
    assert!(provenance.is_from(&same, 1));
    assert!(
        !provenance.is_from(&same, 2),
        "a later generation is not ours"
    );
    assert!(!provenance.is_from(&other, 1), "another file is not ours");

    assert!(Arena::<u32>::new().provenance().is_none());
}
