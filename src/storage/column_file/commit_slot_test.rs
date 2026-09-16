use super::*;
use crate::storage::column_file::{APPEND_MARK, HEAD_LEN};
use crate::test_utils::TempDir;
use proptest::prelude::*;

fn sample(generation: u64) -> CommitSlot {
    CommitSlot::new(
        generation,
        0xc0ffee,
        0xfeed,
        WriterId::from_raw(7),
        12288,
        64,
        0xdd,
    )
}

#[test]
fn the_slot_is_packed_to_its_wire_size() {
    assert_eq!(CommitSlot::SIZE, 80);
    assert_eq!(size_of::<CommitSlot>(), CommitSlot::SIZE);
    assert_eq!(align_of::<CommitSlot>(), 1);
}

#[test]
fn a_slot_round_trips_and_carries_its_own_checksum() {
    let bytes = sample(5).to_bytes();
    assert_eq!(&bytes[..4], b"DBSC");

    let read = CommitSlot::from_bytes(&bytes, 0xfeed).unwrap();
    let (generation, content_id, writer_id) = (read.generation, read.content_id, read.writer_id);
    assert_eq!((generation, content_id, writer_id), (5, 0xc0ffee, 7));
    let (dir_offset, dir_len) = (read.dir_offset, read.dir_len);
    assert_eq!((dir_offset, dir_len), (12288, 64));
}

#[test]
fn a_torn_slot_is_rejected_rather_than_trusted() {
    let bytes = sample(5).to_bytes();

    for at in [0, 8, 40, 71, 79] {
        let mut torn = bytes;
        torn[at] ^= 0xff;
        assert!(
            CommitSlot::from_bytes(&torn, 0xfeed).is_none(),
            "byte {at} should invalidate the slot"
        );
    }

    assert!(CommitSlot::from_bytes(&[0u8; CommitSlot::SIZE], 0xfeed).is_none());
}

#[test]
fn a_slot_from_another_file_body_is_rejected() {
    let bytes = sample(5).to_bytes();
    assert!(CommitSlot::from_bytes(&bytes, 0xfeed).is_some());
    assert!(CommitSlot::from_bytes(&bytes, 0xbeef).is_none());
}

#[test]
fn generation_zero_is_never_valid() {
    let bytes = sample(0).to_bytes();
    assert!(CommitSlot::from_bytes(&bytes, 0xfeed).is_none());
}

fn slot_bytes(generation: u64, file_uuid: u128) -> [u8; CommitSlot::SIZE] {
    CommitSlot::new(
        generation,
        0xc0ffee,
        file_uuid,
        WriterId::from_raw(7),
        APPEND_MARK,
        64,
        0xdd,
    )
    .to_bytes()
}

#[derive(Clone, Copy, Debug)]
enum SlotState {
    Absent,
    Torn,
    Valid(u64),
}

fn slot_state() -> impl Strategy<Value = SlotState> {
    prop_oneof![
        1 => Just(SlotState::Absent),
        1 => Just(SlotState::Torn),
        3 => (1u64..4).prop_map(SlotState::Valid),
    ]
}

fn file_holding(a: SlotState, b: SlotState, file_uuid: u128, path: &std::path::Path) -> File {
    let mut image = vec![0u8; HEAD_LEN as usize];
    for (state, at) in [(a, SLOT_A), (b, SLOT_B)] {
        let at = at as usize;
        let bytes = match state {
            SlotState::Absent => continue,
            SlotState::Valid(generation) => slot_bytes(generation, file_uuid),
            SlotState::Torn => {
                let mut torn = slot_bytes(3, file_uuid);
                torn[9] ^= 0xff;
                torn
            }
        };
        image[at..at + CommitSlot::SIZE].copy_from_slice(&bytes);
    }
    std::fs::write(path, &image).unwrap();
    File::open(path).unwrap()
}

proptest! {
    #[test]
    fn newest_slot_picks_the_higher_generation_and_keeps_slot_a_on_a_tie(
        a in slot_state(),
        b in slot_state(),
    ) {
        let dir = TempDir::new("newest_slot");
        let path = dir.join("file.dbs");
        let file = file_holding(a, b, 0xfeed, &path);

        let wanted = match (a, b) {
            (SlotState::Valid(left), SlotState::Valid(right)) => {
                Some(if right > left { (SLOT_B, right) } else { (SLOT_A, left) })
            }
            (SlotState::Valid(left), _) => Some((SLOT_A, left)),
            (_, SlotState::Valid(right)) => Some((SLOT_B, right)),
            _ => None,
        };

        match (newest_slot(&file, 0xfeed).unwrap(), wanted) {
            (Some((at, slot)), Some((want_at, want_generation))) => {
                let generation = slot.generation;
                prop_assert_eq!(at, want_at);
                prop_assert_eq!(generation, want_generation);
            }
            (None, None) => {}
            (found, wanted) => {
                prop_assert!(false, "{:?} for {a:?}/{b:?}, wanted {wanted:?}", found.map(|(at, _)| at));
            }
        }
    }

    #[test]
    fn a_slot_from_another_file_is_never_the_newest(
        a in slot_state(),
        b in slot_state(),
    ) {
        let dir = TempDir::new("foreign_slot");
        let path = dir.join("file.dbs");
        let file = file_holding(a, b, 0xfeed, &path);
        prop_assert!(newest_slot(&file, 0xbeef).unwrap().is_none());
    }

    #[test]
    fn flipping_any_bit_of_a_slot_invalidates_it(
        generation in 1u64..=u64::MAX,
        at in any::<prop::sample::Index>(),
        mask in 1u8..=u8::MAX,
    ) {
        let mut bytes = slot_bytes(generation, 0xfeed);
        bytes[at.index(CommitSlot::SIZE)] ^= mask;
        prop_assert!(CommitSlot::from_bytes(&bytes, 0xfeed).is_none());
    }

    #[test]
    fn a_slot_round_trips_every_field_it_carries(
        generation in 1u64..=u64::MAX,
        content_id: u64,
        file_uuid: u128,
        writer: u64,
        dir_offset: u64,
        dir_len: u64,
        dir_fnv: u64,
    ) {
        let slot = CommitSlot::new(
            generation,
            content_id,
            file_uuid,
            WriterId::from_raw(writer),
            dir_offset,
            dir_len,
            dir_fnv,
        );
        let read = CommitSlot::from_bytes(&slot.to_bytes(), file_uuid).unwrap();

        let fields = (
            read.generation,
            read.content_id,
            read.file_uuid,
            read.writer_id,
            read.dir_offset,
            read.dir_len,
            read.dir_fnv,
        );
        prop_assert_eq!(
            fields,
            (generation, content_id, file_uuid, writer, dir_offset, dir_len, dir_fnv)
        );
        prop_assert_eq!(read.token(), slot.token());
    }

    #[test]
    fn a_slot_is_rejected_unless_it_carries_this_file_uuid_and_a_real_generation(
        generation: u64,
        file_uuid: u128,
        asked: u128,
    ) {
        let bytes = slot_bytes(generation, file_uuid);
        let accepted = CommitSlot::from_bytes(&bytes, asked).is_some();
        prop_assert_eq!(accepted, generation != 0 && file_uuid == asked);
    }
}

proptest! {
    #[test]
    fn a_directory_never_ends_before_it_starts(dir_offset: u64, dir_len: u64) {
        let slot = CommitSlot::new(
            1,
            0,
            0xfeed,
            WriterId::ANONYMOUS,
            dir_offset,
            dir_len,
            0,
        );

        match dir_offset.checked_add(dir_len) {
            Some(end) => prop_assert_eq!(slot.dir_end(), end),
            None => prop_assert_eq!(slot.dir_end(), u64::MAX),
        }
        prop_assert!(slot.dir_end() >= dir_offset);
    }
}
