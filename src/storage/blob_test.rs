use super::*;
use crate::test_utils::props;
use proptest::prelude::*;

#[test]
fn blob_spans_are_half_open() {
    let blob = Blob::new(10, 4);
    assert_eq!(blob.end(), Some(14));
    assert!(!blob.is_empty());
    assert!(Blob::EMPTY.is_empty());
}

#[test]
fn an_empty_blob_has_a_zero_length_span() {
    assert_eq!(Blob::EMPTY, Blob::default());
    assert_eq!(Blob::EMPTY.end(), Some(0));
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

proptest! {
    #[test]
    fn a_blob_round_trips_through_its_twelve_bytes_wherever_it_points(
        start in props::interesting_u64(),
        len in props::interesting_u32(),
    ) {
        let blob = Blob::new(start, len);
        let mut buf = [0u8; <Blob as Record>::SIZE];
        blob.encode(&mut buf);

        prop_assert_eq!(Blob::decode(&buf), blob);
        prop_assert_eq!(blob.is_empty(), len == 0);
    }

    #[test]
    fn every_twelve_bytes_decode_as_a_blob_and_re_encode_unchanged(bytes: [u8; 12]) {
        let mut again = [0u8; <Blob as Record>::SIZE];
        Blob::decode(&bytes).encode(&mut again);
        prop_assert_eq!(again, bytes);
    }
}

#[test]
fn a_blob_whose_span_runs_off_the_end_of_the_offsets_has_no_end() {
    assert_eq!(Blob::new(u64::MAX, 1).end(), None);
    assert_eq!(Blob::new(u64::MAX, 0).end(), Some(u64::MAX));
    assert_eq!(Blob::new(u64::MAX - 4, 4).end(), Some(u64::MAX));
    assert_eq!(Blob::new(u64::MAX - 4, 5).end(), None);
}

proptest! {
    #[test]
    fn every_blob_reports_an_end_only_when_its_span_fits(
        start in props::interesting_u64(),
        len in props::interesting_u32(),
    ) {
        let blob = Blob::new(start, len);

        match blob.end() {
            Some(end) => {
                prop_assert_eq!(end - start, u64::from(len));
                prop_assert!(end >= start);
            }
            None => prop_assert!(u64::MAX - start < u64::from(len)),
        }
    }
}

#[test]
fn the_none_blob_is_not_the_empty_string_at_the_start_of_a_heap() {
    let mut heap = crate::storage::Arena::<u8>::with_chunk_rows(8);
    let first = heap.push_str("").unwrap();

    assert_eq!(first, Blob::EMPTY);
    assert_ne!(first, Blob::NONE);
    assert_eq!(
        <Option<Blob> as Record>::decode(&encoded(first)),
        Some(first)
    );
    assert_eq!(heap.text(first).unwrap(), "");
}

#[test]
fn no_heap_can_hold_the_bytes_the_none_blob_names() {
    let mut heap = crate::storage::Arena::<u8>::with_chunk_rows(8);
    heap.push_str("some bytes to make it non-empty").unwrap();

    let err = heap.bytes(Blob::NONE).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("runs past"), "{err}");
}

#[test]
fn a_none_blob_round_trips_as_none() {
    assert_eq!(
        <Option<Blob> as Record>::decode(&encoded_option(None)),
        None
    );
    assert_eq!(encoded_option(None), encoded(Blob::NONE));
}

fn encoded(blob: Blob) -> [u8; <Blob as Record>::SIZE] {
    let mut buf = [0u8; <Blob as Record>::SIZE];
    blob.encode(&mut buf);
    buf
}

fn encoded_option(blob: Option<Blob>) -> [u8; <Blob as Record>::SIZE] {
    let mut buf = [0u8; <Blob as Record>::SIZE];
    blob.encode(&mut buf);
    buf
}

proptest! {
    #[test]
    fn every_blob_but_one_round_trips_through_an_option(
        start in props::interesting_u64(),
        len in props::interesting_u32(),
    ) {
        let blob = Blob::new(start, len);
        prop_assume!(blob != Blob::NONE);

        prop_assert_eq!(<Option<Blob> as Record>::decode(&encoded_option(Some(blob))), Some(blob));
    }

    #[test]
    fn every_twelve_bytes_decode_as_an_option_and_re_encode_unchanged(bytes: [u8; 12]) {
        let mut again = [0u8; <Blob as Record>::SIZE];
        <Option<Blob> as Record>::decode(&bytes).encode(&mut again);
        prop_assert_eq!(again, bytes);
    }
}
