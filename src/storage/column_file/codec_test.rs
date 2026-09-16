use super::*;

#[test]
fn known_codecs_round_trip_through_their_byte() {
    assert_eq!(Codec::from_u8(Codec::Raw as u8).unwrap(), Codec::Raw);
    assert_eq!(Codec::Raw as u8, 0);
}

#[test]
fn an_unknown_codec_is_rejected() {
    let err = Codec::from_u8(7).unwrap_err();
    assert!(err.is_unsupported(), "{err}");
    assert!(err.to_string().contains("unknown codec 7"), "{err}");
}
