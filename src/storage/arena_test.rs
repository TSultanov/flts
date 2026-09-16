use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::test_utils::props;
use proptest::prelude::*;
use std::sync::Mutex;

use crate::error::Result;
struct CountingSource {
    chunks: Vec<Vec<u8>>,
    reads: AtomicUsize,
}

impl ChunkSource for CountingSource {
    fn read_chunk(&self, loc: ChunkLoc) -> Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.chunks[loc.offset as usize].clone())
    }
}

fn chunked_u32(rows: u32, chunk_rows: usize) -> (Vec<Vec<u8>>, Vec<ChunkLoc>) {
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
    (chunks, locations)
}

fn lazy_u32(rows: u32, chunk_rows: usize) -> (Arena<u32>, Arc<CountingSource>) {
    let (chunks, locations) = chunked_u32(rows, chunk_rows);

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
    assert_eq!(TARGET_CHUNK_BYTES, 65_536);
    assert_eq!(default_chunk_rows(1), TARGET_CHUNK_BYTES);
    assert_eq!(default_chunk_rows(8), TARGET_CHUNK_BYTES / 8);
    assert_eq!(default_chunk_rows(TARGET_CHUNK_BYTES * 2), 1);
    assert_eq!(default_chunk_rows(0), 1);
}

#[test]
fn push_and_read_back_across_chunk_boundaries() {
    let mut arena = Arena::<u32>::with_chunk_rows(4);
    for value in 0..10u32 {
        assert_eq!(arena.push(value).unwrap(), value as usize);
    }
    assert_eq!(arena.len(), 10);
    assert_eq!(arena.chunk_count(), 3);
    for value in 0..10u32 {
        assert_eq!(arena.get(value as usize).unwrap(), Some(&value));
    }
    assert_eq!(arena.get(10).unwrap(), None);
}

#[test]
fn ranges_borrow_inside_a_chunk_and_copy_across_one() {
    let mut arena = Arena::<u32>::with_chunk_rows(4);
    arena
        .extend_from_slice(&(0..10).collect::<Vec<u32>>())
        .unwrap();

    let inside = arena.range(1, 3).unwrap();
    assert!(matches!(inside, Cow::Borrowed(_)));
    assert_eq!(&*inside, &[1, 2, 3]);

    let across = arena.range(2, 6).unwrap();
    assert!(matches!(across, Cow::Owned(_)));
    assert_eq!(&*across, &[2, 3, 4, 5, 6, 7]);

    assert!(arena.range(0, 0).unwrap().is_empty());
}

#[test]
fn extend_fills_partial_chunks_before_opening_new_ones() {
    let mut arena = Arena::<u32>::with_chunk_rows(4);
    arena.extend_from_slice(&[0, 1]).unwrap();
    arena.extend_from_slice(&[2, 3, 4, 5, 6]).unwrap();
    assert_eq!(arena.chunk_count(), 2);
    assert_eq!(arena.chunk(0).unwrap(), &[0, 1, 2, 3]);
    assert_eq!(arena.chunk(1).unwrap(), &[4, 5, 6]);
    assert_eq!(
        arena.iter().collect::<Result<Vec<_>>>().unwrap(),
        (0..7u32).collect::<Vec<_>>().iter().collect::<Vec<_>>()
    );
}

#[test]
fn a_frozen_view_shares_chunks_and_a_write_copies_only_the_tail() {
    let mut live = Arena::<u32>::with_chunk_rows(4);
    live.extend_from_slice(&(0..9).collect::<Vec<u32>>())
        .unwrap();
    let frozen = live.clone();

    assert_eq!(live.chunk_count(), 3);
    for index in 0..3 {
        assert_eq!(
            Arc::strong_count(&live.chunks[index]),
            2,
            "chunk {index} is shared"
        );
    }

    live.push(99).unwrap();

    assert_eq!(Arc::strong_count(&live.chunks[0]), 2);
    assert_eq!(Arc::strong_count(&live.chunks[1]), 2);
    assert_eq!(Arc::strong_count(&live.chunks[2]), 1);

    assert_eq!(frozen.len(), 9);
    assert_eq!(frozen.get(8).unwrap(), Some(&8));
    assert_eq!(live.len(), 10);
    assert_eq!(live.get(9).unwrap(), Some(&99));
}

#[test]
fn setting_a_row_leaves_a_frozen_view_alone() {
    let mut live = Arena::<u32>::with_chunk_rows(4);
    live.extend_from_slice(&(0..9).collect::<Vec<u32>>())
        .unwrap();
    let frozen = live.clone();

    live.set(1, 111).unwrap();
    assert_eq!(live.get(1).unwrap(), Some(&111));
    assert_eq!(frozen.get(1).unwrap(), Some(&1));
    assert_eq!(
        Arc::strong_count(&live.chunks[1]),
        2,
        "untouched chunk stays shared"
    );
}

#[test]
fn opening_a_lazy_arena_reads_nothing_until_a_row_is_asked_for() {
    let (arena, source) = lazy_u32(10, 4);
    assert_eq!(arena.len(), 10);
    assert_eq!(arena.chunk_count(), 3);
    assert_eq!(arena.resident_chunks(), 0);
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);

    assert_eq!(arena.get(5).unwrap(), Some(&5));
    assert_eq!(arena.resident_chunks(), 1, "only the touched chunk loaded");
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);

    assert_eq!(arena.get(4).unwrap(), Some(&4));
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);

    arena.prefetch().unwrap();
    assert_eq!(arena.resident_chunks(), 3);
    assert_eq!(source.reads.load(Ordering::SeqCst), 3);
}

#[test]
fn a_load_in_one_view_is_visible_to_every_view_sharing_the_chunk() {
    let (live, source) = lazy_u32(10, 4);
    let frozen = live.clone();

    assert_eq!(frozen.get(0).unwrap(), Some(&0));
    assert_eq!(live.get(0).unwrap(), Some(&0));
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        1,
        "the two views share the load"
    );
}

#[test]
fn appending_to_a_lazy_arena_faults_in_only_the_tail_chunk() {
    let (mut arena, source) = lazy_u32(10, 4);
    arena.push(42).unwrap();

    assert_eq!(arena.len(), 11);
    assert_eq!(arena.resident_chunks(), 1);
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        1,
        "only the short tail was read"
    );
    assert_eq!(arena.get(10).unwrap(), Some(&42));
    assert_eq!(arena.get(9).unwrap(), Some(&9));
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

    let err = tampered.get(0).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
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
    let hello = bytes.push_str("hello").unwrap();
    let long = bytes
        .push_str("a string that spans several chunks")
        .unwrap();
    let empty = bytes.push_str("").unwrap();

    assert_eq!(&*bytes.text(hello).unwrap(), "hello");
    assert_eq!(
        &*bytes.text(long).unwrap(),
        "a string that spans several chunks"
    );
    assert_eq!(&*bytes.text(empty).unwrap(), "");
    assert_eq!(hello.start, 0);
    assert_eq!(long.start, 5);
}

#[test]
fn invalid_utf8_is_replaced_rather_than_failing_the_read() {
    let mut bytes = Arena::<u8>::new();
    let blob = bytes.push_bytes(&[0xff, 0xfe]).unwrap();
    assert_eq!(&*bytes.text(blob).unwrap(), "\u{fffd}\u{fffd}");
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
    arena.set(1, 999).unwrap();

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

    arena.set(0, 5).unwrap();
    assert!(arena.chunks[0].still_on_disk().is_none());
    assert!(arena.chunks[1].still_on_disk().is_some());

    arena.push(42).unwrap();
    assert!(arena.chunks[2].still_on_disk().is_none());
}

#[test]
fn writing_out_a_resident_chunk_does_not_read_it_again() {
    let (arena, source) = lazy_u32(10, 4);
    arena.get(0).unwrap();
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);

    arena.chunk_bytes(0).unwrap();
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);
    assert_eq!(arena.resident_chunks(), 1);
}

#[test]
fn taking_owned_rows_twice_is_idempotent() {
    let (mut arena, source) = lazy_u32(10, 4);
    arena.set(0, 111).unwrap();
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);

    arena.set(1, 222).unwrap();
    arena.set(2, 333).unwrap();
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);

    let cell = Arc::make_mut(&mut arena.chunks[0]);
    assert_eq!(cell.owned_rows().as_slice(), &[111, 222, 333, 3]);
    assert_eq!(cell.owned_rows().as_slice(), &[111, 222, 333, 3]);
}

#[test]
fn reading_a_chunk_does_not_cost_it_its_reusable_location() {
    let (arena, _) = lazy_u32(10, 4);
    assert!(arena.reusable_loc(0).is_some());

    arena.get(0).unwrap();
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

    arena.set(0, 5).unwrap();
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
    assert_eq!(Provenance::new(source.clone(), 7).generation(), 7);
    assert!(provenance.is_from(&same, 1));
    assert!(
        !provenance.is_from(&same, 2),
        "a later generation is not ours"
    );
    assert!(!provenance.is_from(&other, 1), "another file is not ours");

    assert!(Arena::<u32>::new().provenance().is_none());
}

#[test]
#[should_panic(expected = "range 8..13 past end 10")]
fn a_range_past_the_end_is_a_caller_bug() {
    let mut arena = Arena::<u32>::with_chunk_rows(4);
    arena
        .extend_from_slice(&(0..10).collect::<Vec<u32>>())
        .unwrap();
    let _ = arena.range(8, 5);
}

#[test]
#[should_panic(expected = "index 99 past end 9")]
fn setting_past_the_end_is_a_caller_bug() {
    let mut arena = Arena::<u32>::with_chunk_rows(4);
    arena
        .extend_from_slice(&(0..9).collect::<Vec<u32>>())
        .unwrap();
    let _ = arena.set(99, 0);
}

#[test]
fn an_arena_is_empty_only_before_anything_is_pushed() {
    let mut arena = Arena::<u32>::new();
    assert!(arena.is_empty());
    arena.push(1).unwrap();
    assert!(!arena.is_empty());
}

#[test]
fn the_debug_of_an_arena_names_its_shape_and_residency() {
    let (arena, _) = lazy_u32(10, 4);
    let shown = format!("{arena:?}");
    for part in ["rows", "10", "chunk_rows", "chunks", "resident", "lazy"] {
        assert!(shown.contains(part), "{shown} is missing {part}");
    }
}

#[derive(Clone, Debug)]
enum ArenaStep {
    Push(u32),
    Extend(Vec<u32>),
    Set(prop::sample::Index, u32),
    FreezeClone,
}

fn arena_step() -> impl Strategy<Value = ArenaStep> {
    prop_oneof![
        4 => any::<u32>().prop_map(ArenaStep::Push),
        3 => prop::collection::vec(any::<u32>(), 0..12).prop_map(ArenaStep::Extend),
        3 => (any::<prop::sample::Index>(), any::<u32>())
                .prop_map(|(at, value)| ArenaStep::Set(at, value)),
        1 => Just(ArenaStep::FreezeClone),
    ]
}

proptest! {
    #[test]
    fn an_arena_behaves_like_the_vector_it_models(
        chunk_rows in props::chunk_rows(),
        steps in prop::collection::vec(arena_step(), 0..40),
    ) {
        let mut arena = Arena::<u32>::with_chunk_rows(chunk_rows);
        let mut model: Vec<u32> = Vec::new();
        let mut frozen: Vec<(Arena<u32>, Vec<u32>)> = Vec::new();

        for step in steps {
            match step {
                ArenaStep::Push(value) => {
                    prop_assert_eq!(arena.push(value).unwrap(), model.len());
                    model.push(value);
                }
                ArenaStep::Extend(values) => {
                    arena.extend_from_slice(&values).unwrap();
                    model.extend_from_slice(&values);
                }
                ArenaStep::Set(at, value) => {
                    if model.is_empty() {
                        continue;
                    }
                    let at = at.index(model.len());
                    arena.set(at, value).unwrap();
                    model[at] = value;
                }
                ArenaStep::FreezeClone => frozen.push((arena.clone(), model.clone())),
            }

            prop_assert_eq!(arena.len(), model.len());
            prop_assert_eq!(arena.is_empty(), model.is_empty());
            prop_assert_eq!(arena.chunk_count(), model.len().div_ceil(chunk_rows));
            prop_assert_eq!(arena.get(model.len()).unwrap(), None);
            for (at, want) in model.iter().enumerate() {
                prop_assert_eq!(arena.get(at).unwrap(), Some(want));
            }
        }

        for (snapshot, want) in frozen {
            prop_assert_eq!(snapshot.len(), want.len());
            for (at, value) in want.iter().enumerate() {
                prop_assert_eq!(snapshot.get(at).unwrap(), Some(value));
            }
        }
    }

    #[test]
    fn every_range_agrees_with_the_rows_it_spans(
        chunk_rows in props::chunk_rows(),
        values in prop::collection::vec(any::<u32>(), 0..80),
        from in any::<prop::sample::Index>(),
        span in any::<prop::sample::Index>(),
    ) {
        let mut arena = Arena::<u32>::with_chunk_rows(chunk_rows);
        arena.extend_from_slice(&values).unwrap();

        let start = from.index(values.len() + 1);
        let len = span.index(values.len() - start + 1);
        let got = arena.range(start, len).unwrap();

        prop_assert_eq!(got.as_ref(), &values[start..start + len]);

        let within_one_chunk = len == 0 || start / chunk_rows == (start + len - 1) / chunk_rows;
        prop_assert_eq!(matches!(got, Cow::Borrowed(_)), within_one_chunk);
    }
}

fn lazy_from_rows(rows: &[u32], chunk_rows: usize, corrupt: Option<usize>) -> Result<Arena<u32>> {
    let mut chunks = Vec::new();
    let mut locations = Vec::new();
    for &count in rows {
        let bytes = vec![0u8; count as usize * size_of::<u32>()];
        locations.push(ChunkLoc {
            offset: chunks.len() as u64,
            byte_len: bytes.len() as u32,
            rows: count,
            fnv: fnv1a(&bytes),
        });
        chunks.push(bytes);
    }

    if let Some(at) = corrupt {
        locations[at].fnv ^= 0xffff;
    }

    let source = Arc::new(CountingSource {
        chunks,
        reads: AtomicUsize::new(0),
    });
    Arena::from_locations(locations, chunk_rows, Provenance::new(source, 1))
}

proptest! {
    #[test]
    fn a_lazy_arena_refuses_a_geometry_its_own_reader_could_not_index(
        chunk_rows in 1usize..8,
        rows in prop::collection::vec(0u32..12, 0..5),
    ) {
        let last = rows.len().saturating_sub(1);
        let indexable = rows.iter().enumerate().all(|(at, &count)| {
            let count = count as usize;
            if at == last { count <= chunk_rows } else { count == chunk_rows }
        });

        match lazy_from_rows(&rows, chunk_rows, None) {
            Ok(arena) => {
                prop_assert!(indexable);
                for at in 0..arena.len() {
                    prop_assert!(arena.get(at).unwrap().is_some());
                }
            }
            Err(err) => {
                prop_assert!(!indexable);
                prop_assert!(err.is_corrupt());
            }
        }
    }

    #[test]
    fn a_blob_that_points_outside_the_arena_is_an_error_not_a_panic(
        text in prop::collection::vec(any::<u8>(), 0..40),
        start in props::interesting_u64(),
        len in props::interesting_u32(),
    ) {
        let mut arena = Arena::<u8>::with_chunk_rows(8);
        arena.extend_from_slice(&text).unwrap();

        let blob = Blob::new(start, len);
        let inside = start
            .checked_add(u64::from(len))
            .is_some_and(|end| end <= text.len() as u64);

        match arena.bytes(blob) {
            Ok(bytes) => {
                prop_assert!(inside);
                let at = start as usize;
                prop_assert_eq!(bytes.as_ref(), &text[at..at + len as usize]);
            }
            Err(err) => prop_assert!(!inside && err.is_corrupt()),
        }
    }
}

proptest! {
    #[test]
    fn text_pushed_into_a_byte_arena_reads_back_whole_across_chunk_boundaries(
        chunk_rows in 1usize..12,
        texts in prop::collection::vec(".{0,20}", 0..8),
    ) {
        let mut arena = Arena::<u8>::with_chunk_rows(chunk_rows);
        let blobs: Vec<Blob> = texts
            .iter()
            .map(|text| arena.push_str(text).unwrap())
            .collect();

        for (blob, want) in blobs.iter().zip(&texts) {
            let text = arena.text(*blob).unwrap();
            let bytes = arena.bytes(*blob).unwrap();
            prop_assert_eq!(text.as_ref(), want.as_str());
            prop_assert_eq!(bytes.as_ref(), want.as_bytes());
        }
    }

    #[test]
    fn default_chunk_rows_keeps_every_chunk_inside_the_byte_budget(
        elem_size in 0usize..=TARGET_CHUNK_BYTES * 2,
    ) {
        let rows = default_chunk_rows(elem_size);
        prop_assert!(rows >= 1);
        if elem_size > 0 && rows > 1 {
            prop_assert!(rows * elem_size <= TARGET_CHUNK_BYTES);
        }
    }

    #[test]
    fn reading_a_chunk_costs_exactly_one_source_read_no_matter_how_many_views_share_it(
        rows in 1u32..40,
        chunk_rows in 1usize..8,
        touches in prop::collection::vec(any::<prop::sample::Index>(), 1..12),
    ) {
        let (arena, source) = lazy_u32(rows, chunk_rows);
        let shared = arena.clone();
        let mut faulted = std::collections::HashSet::new();

        for (turn, at) in touches.iter().enumerate() {
            let at = at.index(rows as usize);
            let view = if turn.is_multiple_of(2) { &arena } else { &shared };
            prop_assert_eq!(view.get(at).unwrap(), Some(&(at as u32)));
            faulted.insert(at / chunk_rows);
        }

        prop_assert_eq!(source.reads.load(Ordering::SeqCst), faulted.len());
    }

    #[test]
    fn a_chunk_that_fails_its_checksum_is_never_decoded(
        chunk_rows in 1usize..8,
        chunks in 1usize..5,
        bad in any::<prop::sample::Index>(),
    ) {
        let rows = vec![chunk_rows as u32; chunks];
        let bad = bad.index(chunks);
        let arena = lazy_from_rows(&rows, chunk_rows, Some(bad)).unwrap();

        for at in 0..arena.len() {
            let read = arena.get(at);
            if at / chunk_rows == bad {
                prop_assert!(read.unwrap_err().is_corrupt());
            } else {
                prop_assert!(read.is_ok());
            }
        }

        prop_assert!(arena.prefetch().is_err());
        prop_assert!(arena.chunk_bytes(bad).unwrap_err().is_corrupt());
    }

    #[test]
    fn chunk_bytes_round_trips_every_chunk_whether_it_is_resident_or_not(
        rows in 0u32..40,
        chunk_rows in 1usize..8,
        read_first in any::<bool>(),
    ) {
        let (arena, _) = lazy_u32(rows, chunk_rows);
        if read_first {
            arena.prefetch().unwrap();
        }

        for at in 0..arena.chunk_count() {
            let bytes = arena.chunk_bytes(at).unwrap();
            let decoded: Vec<u32> = bytes
                .as_chunks::<{ size_of::<u32>() }>()
                .0
                .iter()
                .map(|four| u32::from_le_bytes(*four))
                .collect();
            prop_assert_eq!(decoded.as_slice(), arena.chunk(at).unwrap());
        }
    }
}

#[test]
fn a_column_read_from_many_threads_loads_each_chunk_once() {
    use std::sync::Barrier;

    for _ in 0..25 {
        let (arena, source) = lazy_u32(40, 4);
        let arena = Arc::new(arena);
        let start = Arc::new(Barrier::new(8));

        let mut readers = Vec::new();
        for turn in 0..8 {
            let arena = arena.clone();
            let start = start.clone();

            readers.push(std::thread::spawn(move || {
                start.wait();
                for at in 0..40usize {
                    let at = (at + turn * 5) % 40;
                    assert_eq!(arena.get(at).unwrap(), Some(&(at as u32)));
                }
            }));
        }
        for reader in readers {
            reader.join().unwrap();
        }

        assert_eq!(
            source.reads.load(Ordering::SeqCst),
            10,
            "eight readers of ten chunks paid for more than ten reads"
        );
        assert_eq!(arena.resident_chunks(), 10);
    }
}

struct ParkingSource {
    chunks: Vec<Vec<u8>>,
    reads: AtomicUsize,
    entered: AtomicUsize,
    released: Mutex<bool>,
    wake: std::sync::Condvar,
}

impl ParkingSource {
    fn holding(chunks: Vec<Vec<u8>>) -> Self {
        Self {
            chunks,
            reads: AtomicUsize::new(0),
            entered: AtomicUsize::new(0),
            released: Mutex::new(false),
            wake: std::sync::Condvar::new(),
        }
    }

    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}

impl ChunkSource for ParkingSource {
    fn read_chunk(&self, loc: ChunkLoc) -> Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.entered.fetch_add(1, Ordering::SeqCst);

        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.wake.wait(released).unwrap();
        }
        Ok(self.chunks[loc.offset as usize].clone())
    }
}

struct FlakySource {
    chunks: Vec<Vec<u8>>,
    failures_left: AtomicUsize,
    reads: AtomicUsize,
}

impl ChunkSource for FlakySource {
    fn read_chunk(&self, loc: ChunkLoc) -> Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::SeqCst);

        let failing = self
            .failures_left
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if failing {
            return Err(
                std::io::Error::new(std::io::ErrorKind::Interrupted, "the disk hiccuped").into(),
            );
        }
        Ok(self.chunks[loc.offset as usize].clone())
    }
}

fn parked(
    rows: u32,
    chunk_rows: usize,
    spoil: Option<usize>,
) -> (Arc<Arena<u32>>, Arc<ParkingSource>) {
    let (chunks, mut locations) = chunked_u32(rows, chunk_rows);
    if let Some(at) = spoil {
        locations[at].fnv ^= 0xffff;
    }

    let source = Arc::new(ParkingSource::holding(chunks));
    let arena =
        Arena::from_locations(locations, chunk_rows, Provenance::new(source.clone(), 1)).unwrap();
    (Arc::new(arena), source)
}

fn wait_until(ready: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !ready() && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
}

fn race_for_row_zero(
    arena: Arc<Arena<u32>>,
    source: &ParkingSource,
    readers: usize,
) -> Vec<Result<u32>> {
    let start = Arc::new(std::sync::Barrier::new(readers));
    let arrived = Arc::new(AtomicUsize::new(0));

    let mut threads = Vec::new();
    for _ in 0..readers {
        let arena = arena.clone();
        let start = start.clone();
        let arrived = arrived.clone();

        threads.push(std::thread::spawn(move || {
            start.wait();
            arrived.fetch_add(1, Ordering::SeqCst);
            arena.row(0).copied()
        }));
    }

    wait_until(|| {
        arrived.load(Ordering::SeqCst) == readers && source.entered.load(Ordering::SeqCst) > 0
    });
    source.release();

    threads
        .into_iter()
        .map(|reader| reader.join().unwrap())
        .collect()
}

#[test]
fn a_cold_chunk_is_read_exactly_once_even_when_eight_threads_race_for_it() {
    let (arena, source) = parked(40, 4, None);
    let seen = race_for_row_zero(arena, &source, 8);

    for row in seen {
        assert_eq!(row.unwrap(), 0);
    }
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        1,
        "eight readers of one cold chunk paid for more than one read"
    );
}

#[test]
fn eight_readers_of_a_rotten_chunk_pay_for_exactly_one_read() {
    let (arena, source) = parked(40, 4, Some(0));
    let seen = race_for_row_zero(arena, &source, 8);

    for row in seen {
        assert!(row.unwrap_err().is_corrupt());
    }
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        1,
        "a reader waiting on the gate re-read instead of re-checking"
    );
}

#[test]
fn two_views_of_the_same_chunk_share_one_gate() {
    let (arena, source) = parked(40, 4, None);
    let view = Arc::new(arena.as_ref().clone());

    let start = Arc::new(std::sync::Barrier::new(2));
    let arrived = Arc::new(AtomicUsize::new(0));
    let mut readers = Vec::new();
    for held in [arena, view] {
        let start = start.clone();
        let arrived = arrived.clone();
        readers.push(std::thread::spawn(move || {
            start.wait();
            arrived.fetch_add(1, Ordering::SeqCst);
            held.row(0).copied()
        }));
    }

    wait_until(|| arrived.load(Ordering::SeqCst) == 2 && source.entered.load(Ordering::SeqCst) > 0);
    source.release();

    for reader in readers {
        assert_eq!(reader.join().unwrap().unwrap(), 0);
    }
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        1,
        "two views of one chunk each loaded it"
    );
}

fn spoiled_u32(rows: u32, chunk_rows: usize, at: usize) -> (Arena<u32>, Arc<CountingSource>) {
    let (chunks, mut locations) = chunked_u32(rows, chunk_rows);
    locations[at].fnv ^= 0xffff;

    let source = Arc::new(CountingSource {
        chunks,
        reads: AtomicUsize::new(0),
    });
    let arena =
        Arena::from_locations(locations, chunk_rows, Provenance::new(source.clone(), 1)).unwrap();
    (arena, source)
}

fn flaky_u32(rows: u32, chunk_rows: usize, failures: usize) -> (Arena<u32>, Arc<FlakySource>) {
    let (chunks, locations) = chunked_u32(rows, chunk_rows);

    let source = Arc::new(FlakySource {
        chunks,
        failures_left: AtomicUsize::new(failures),
        reads: AtomicUsize::new(0),
    });
    let arena =
        Arena::from_locations(locations, chunk_rows, Provenance::new(source.clone(), 1)).unwrap();
    (arena, source)
}

#[test]
fn a_corrupt_chunk_is_read_once_and_then_remembered() {
    let (arena, source) = spoiled_u32(12, 4, 0);

    for at in 0..4 {
        assert!(arena.get(at).unwrap_err().is_corrupt());
    }
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        1,
        "every row of a rotten chunk paid for its own read"
    );
    assert_eq!(arena.resident_chunks(), 0, "damage is not rows");
    assert!(arena.get(4).is_ok(), "the healthy chunks still read");
}

#[test]
fn chunk_bytes_reports_the_remembered_corruption_without_reading_again() {
    let (arena, source) = spoiled_u32(12, 4, 0);

    assert!(arena.get(0).unwrap_err().is_corrupt());
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);

    assert!(arena.chunk_bytes(0).unwrap_err().is_corrupt());
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        1,
        "a remembered failure is neither resident nor on disk, and must not be re-read"
    );
}

#[test]
fn a_failed_read_is_retried_and_the_row_appears_once_the_source_recovers() {
    let (arena, source) = flaky_u32(12, 4, 1);

    let hiccup = arena.get(0).unwrap_err();
    assert!(!hiccup.is_corrupt(), "{hiccup}");
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);
    assert_eq!(arena.resident_chunks(), 0);
    assert!(
        arena.chunks[0].still_on_disk().is_some(),
        "a chunk that merely failed to arrive is still on disk"
    );

    assert_eq!(arena.get(0).unwrap(), Some(&0));
    assert_eq!(source.reads.load(Ordering::SeqCst), 2, "the retry happened");
}

struct PanickingSource {
    chunks: Vec<Vec<u8>>,
    panics_left: AtomicUsize,
    reads: AtomicUsize,
}

impl ChunkSource for PanickingSource {
    fn read_chunk(&self, loc: ChunkLoc) -> Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::SeqCst);

        let bursting = self
            .panics_left
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        assert!(!bursting, "this source panics on purpose");

        Ok(self.chunks[loc.offset as usize].clone())
    }
}

#[test]
fn a_source_that_panics_does_not_wedge_the_chunk_for_later_readers() {
    let (chunks, locations) = chunked_u32(12, 4);
    let source = Arc::new(PanickingSource {
        chunks,
        panics_left: AtomicUsize::new(1),
        reads: AtomicUsize::new(0),
    });
    let arena =
        Arena::<u32>::from_locations(locations, 4, Provenance::new(source.clone(), 1)).unwrap();

    let quiet = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let burst = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| arena.get(0)));
    std::panic::set_hook(quiet);

    assert!(burst.is_err(), "the source was supposed to panic");
    assert_eq!(arena.get(0).unwrap(), Some(&0), "the gate stayed usable");
    assert_eq!(source.reads.load(Ordering::SeqCst), 2);
}

proptest! {
    #[test]
    fn a_chunk_costs_one_read_for_each_failure_and_one_more_for_the_success(
        failures in 0usize..5,
        extra in 0usize..6,
    ) {
        let (arena, source) = flaky_u32(12, 4, failures);

        for _ in 0..failures {
            let hiccup = arena.get(0).unwrap_err();
            prop_assert!(!hiccup.is_corrupt());
        }
        for _ in 0..=extra {
            prop_assert_eq!(arena.get(0).unwrap(), Some(&0));
        }

        prop_assert_eq!(source.reads.load(Ordering::SeqCst), failures + 1);
    }
}
