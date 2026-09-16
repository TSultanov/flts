use super::*;

#[test]
fn the_footer_is_packed_to_its_wire_size() {
    assert_eq!(Footer::SIZE, 28);
    assert_eq!(size_of::<Footer>(), Footer::SIZE);
    assert_eq!(align_of::<Footer>(), 1);
}

#[test]
fn the_footer_round_trips_little_endian() {
    let mut bytes = [0u8; Footer::SIZE];
    Footer::new(0x1122, 0x33, 0x44).encode(&mut bytes);
    assert_eq!(&bytes[..2], &[0x22, 0x11]);
    assert_eq!(&bytes[24..], b"DBS0");

    let read = Footer::decode(&bytes);
    read.validate().unwrap();
    let (dir_offset, dir_len, dir_fnv) = (read.dir_offset, read.dir_len, read.dir_fnv);
    assert_eq!((dir_offset, dir_len, dir_fnv), (0x1122, 0x33, 0x44));
}

#[test]
fn a_bad_footer_magic_is_rejected() {
    let mut bytes = [0u8; Footer::SIZE];
    Footer::new(1, 2, 3).encode(&mut bytes);

    bytes[27] = b'X';
    let err = Footer::decode(&bytes).validate().unwrap_err();
    assert!(err.to_string().contains("truncated"), "{err}");
}
