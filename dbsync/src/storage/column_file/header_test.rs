use super::*;
use proptest::prelude::*;

#[test]
fn the_header_is_packed_to_its_wire_size() {
    assert_eq!(Header::SIZE, 24);
    assert_eq!(size_of::<Header>(), Header::SIZE);
    assert_eq!(align_of::<Header>(), 1);
}

#[test]
fn the_header_round_trips_little_endian() {
    let mut bytes = [0u8; Header::SIZE];
    Header::new(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10).encode(&mut bytes);
    assert_eq!(&bytes[..4], b"DBS0");
    assert_eq!(&bytes[4..8], &[1, 0, 0, 0]);
    assert_eq!(bytes[8], 0x10);
    assert_eq!(bytes[23], 0x01);

    let read = Header::decode(&bytes);
    read.validate().unwrap();
    assert_eq!(read.file_uuid(), 0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
}

#[test]
fn a_bad_magic_or_version_is_rejected() {
    let mut bytes = [0u8; Header::SIZE];
    Header::new(7).encode(&mut bytes);

    let mut wrong_magic = bytes;
    wrong_magic[0] = b'X';
    let err = Header::decode(&wrong_magic).validate().unwrap_err();
    assert!(err.to_string().contains("bad column file magic"), "{err}");

    let mut wrong_version = bytes;
    wrong_version[4] = 9;
    let err = Header::decode(&wrong_version).validate().unwrap_err();
    assert!(err.to_string().contains("version 9"), "{err}");
}

#[test]
fn file_uuids_differ_between_files() {
    let a = new_file_uuid();
    let b = new_file_uuid();
    assert_ne!(a, b);
    assert_ne!(a, 0);
    assert_ne!(a >> 64, 0);
}

proptest! {
    #[test]
    fn the_header_accepts_exactly_its_own_magic_and_version(bytes: [u8; 24]) {
        let carries_magic = bytes[..4] == MAGIC;
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);

        match Header::decode(&bytes).validate() {
            Ok(()) => {
                prop_assert!(carries_magic);
                prop_assert_eq!(version, FORMAT_VERSION);
            }
            Err(err) if err.is_unsupported() => {
                prop_assert!(carries_magic);
                prop_assert_ne!(version, FORMAT_VERSION);
            }
            Err(err) => {
                prop_assert!(!carries_magic);
                prop_assert!(
                    !err.is_corrupt(),
                    "a file that is not ours is the wrong path, not a damaged file"
                );
            }
        }
    }

    #[test]
    fn a_header_round_trips_the_body_it_names(file_uuid: u128) {
        let mut bytes = [0u8; Header::SIZE];
        Header::new(file_uuid).encode(&mut bytes);

        let read = Header::decode(&bytes);
        read.validate().unwrap();
        prop_assert_eq!(read.file_uuid(), file_uuid);
    }
}
