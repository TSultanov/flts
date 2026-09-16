use super::*;

fn sample(generation: u64) -> CommitSlot {
    CommitSlot::new(
        generation,
        0xc0ffee,
        0xfeed,
        WriterId::from_raw(7),
        12288,
        64,
        0xdd,
        12352,
    )
}

#[test]
fn the_slot_is_packed_to_its_wire_size() {
    assert_eq!(CommitSlot::SIZE, 88);
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

    for at in [0, 8, 40, 72, 87] {
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
