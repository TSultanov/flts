use super::*;

#[test]
fn blob_spans_are_half_open() {
    let blob = Blob::new(10, 4);
    assert_eq!(blob.end(), 14);
    assert!(!blob.is_empty());
    assert!(Blob::EMPTY.is_empty());
}

#[test]
fn an_empty_blob_has_a_zero_length_span() {
    assert_eq!(Blob::EMPTY, Blob::default());
    assert_eq!(Blob::EMPTY.end(), 0);
    assert!(Blob::new(9, 0).is_empty());
}

#[test]
fn a_blob_round_trips_through_its_twelve_bytes() {
    let blob = Blob::new(1 << 40, 77);
    let mut buf = [0u8; <Blob as Record>::SIZE];
    blob.encode(&mut buf);
    assert_eq!(<Blob as Record>::SIZE, 12);
    assert_eq!(Blob::decode(&buf), blob);
}
