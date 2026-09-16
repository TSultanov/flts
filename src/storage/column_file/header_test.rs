use super::*;

#[test]
fn the_header_is_packed_to_its_wire_size() {
    assert_eq!(Header::SIZE, 8);
    assert_eq!(size_of::<Header>(), Header::SIZE);
    assert_eq!(align_of::<Header>(), 1);
}

#[test]
fn the_header_round_trips_little_endian() {
    let mut bytes = [0u8; Header::SIZE];
    Header::current().encode(&mut bytes);
    assert_eq!(bytes, [b'D', b'B', b'S', b'0', 1, 0, 0, 0]);

    let read = Header::decode(&bytes);
    read.validate().unwrap();
    let (version, reserved) = (read.version, read.reserved);
    assert_eq!((version, reserved), (FORMAT_VERSION, 0));
}

#[test]
fn a_bad_magic_or_version_is_rejected() {
    let mut bytes = [0u8; Header::SIZE];
    Header::current().encode(&mut bytes);

    let mut wrong_magic = bytes;
    wrong_magic[0] = b'X';
    let err = Header::decode(&wrong_magic).validate().unwrap_err();
    assert!(err.to_string().contains("bad column file magic"), "{err}");

    let mut wrong_version = bytes;
    wrong_version[4] = 9;
    let err = Header::decode(&wrong_version).validate().unwrap_err();
    assert!(err.to_string().contains("version 9"), "{err}");
}
