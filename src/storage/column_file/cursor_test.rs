use super::*;
use proptest::prelude::*;

#[test]
fn reads_little_endian_values_in_sequence() {
    let bytes = [9u8, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0];
    let mut cursor = Cursor::new(&bytes);
    assert_eq!(cursor.u8().unwrap(), 9);
    assert_eq!(cursor.u32().unwrap(), 1);
    assert_eq!(cursor.u64().unwrap(), 2);
    assert!(cursor.u8().is_err());
}

#[test]
fn take_yields_exactly_the_requested_bytes_then_advances() {
    let bytes = *b"abcdef";
    let mut cursor = Cursor::new(&bytes);
    assert_eq!(cursor.take(2).unwrap(), b"ab");
    assert_eq!(cursor.take(4).unwrap(), b"cdef");
    assert_eq!(cursor.take(0).unwrap(), b"");
}

#[test]
fn running_past_the_end_is_an_error_not_a_panic() {
    let bytes = [1u8, 2, 3];
    let mut cursor = Cursor::new(&bytes);
    let err = cursor.take(4).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("ends mid-record"), "{err}");

    let err = Cursor::new(&bytes).take(usize::MAX).unwrap_err();
    assert!(err.to_string().contains("ends mid-record"), "{err}");
}

#[test]
fn a_length_that_overflows_the_position_is_an_error() {
    let bytes = [1u8, 2, 3];
    let mut cursor = Cursor::new(&bytes);
    cursor.take(2).unwrap();
    let err = cursor.take(usize::MAX).unwrap_err();
    assert!(err.to_string().contains("offset overflow"), "{err}");
}

proptest! {
    #[test]
    fn a_cursor_never_reads_past_the_slice_it_was_given(
        bytes in prop::collection::vec(any::<u8>(), 0..64),
        script in prop::collection::vec(0usize..4, 0..24),
    ) {
        let mut cursor = Cursor::new(&bytes);
        let mut at = 0usize;

        for step in script {
            let (wanted, read) = match step {
                0 => (1, cursor.u8().map(|value| vec![value])),
                1 => (4, cursor.u32().map(|value| value.to_le_bytes().to_vec())),
                2 => (8, cursor.u64().map(|value| value.to_le_bytes().to_vec())),
                _ => (3, cursor.take(3).map(<[u8]>::to_vec)),
            };

            match read {
                Ok(got) => {
                    prop_assert_eq!(got.as_slice(), &bytes[at..at + wanted]);
                    at += wanted;
                }
                Err(err) => prop_assert!(err.is_corrupt()),
            }
        }
    }
}
