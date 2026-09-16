use super::*;
use crate::storage::column_file::{MAGIC, SLOT_MAGIC};

fn sample() -> AppendMark {
    AppendMark::new(0xfeed, 20_000, 5)
}

#[test]
fn the_mark_is_packed_to_its_wire_size() {
    assert_eq!(AppendMark::SIZE, 48);
    assert_eq!(size_of::<AppendMark>(), AppendMark::SIZE);
    assert_eq!(align_of::<AppendMark>(), 1);
}

#[test]
fn a_mark_round_trips_and_carries_its_own_checksum() {
    let bytes = sample().to_bytes();
    assert_eq!(&bytes[..4], b"DBSA");

    let read = AppendMark::from_bytes(&bytes, 0xfeed).unwrap();
    let (append_from, generation) = (read.append_from, read.generation);
    assert_eq!((append_from, generation), (20_000, 5));
}

#[test]
fn a_torn_mark_is_rejected_rather_than_trusted() {
    let bytes = sample().to_bytes();

    for at in 0..AppendMark::SIZE {
        let mut torn = bytes;
        torn[at] ^= 0xff;
        assert!(
            AppendMark::from_bytes(&torn, 0xfeed).is_none(),
            "byte {at} should invalidate the mark"
        );
    }

    assert!(AppendMark::from_bytes(&[0u8; AppendMark::SIZE], 0xfeed).is_none());
}

#[test]
fn a_mark_from_another_file_body_is_rejected() {
    let bytes = sample().to_bytes();
    assert!(AppendMark::from_bytes(&bytes, 0xfeed).is_some());
    assert!(AppendMark::from_bytes(&bytes, 0xbeef).is_none());
}

#[test]
fn generation_zero_is_never_valid() {
    let bytes = AppendMark::new(0xfeed, 20_000, 0).to_bytes();
    assert!(AppendMark::from_bytes(&bytes, 0xfeed).is_none());
}

#[test]
fn the_mark_does_not_share_a_magic_with_the_other_records() {
    assert_ne!(MARK_MAGIC, MAGIC);
    assert_ne!(MARK_MAGIC, SLOT_MAGIC);
}
