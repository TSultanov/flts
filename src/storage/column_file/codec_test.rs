use super::*;

#[test]
fn known_codecs_round_trip_through_their_byte() {
    assert_eq!(Codec::from_u8(Codec::Zstd as u8).unwrap(), Codec::Zstd);
    assert_eq!(Codec::Zstd as u8, 1);
}

#[test]
fn an_unknown_codec_is_rejected() {
    let err = Codec::from_u8(7).unwrap_err();
    assert!(err.is_unsupported(), "{err}");
    assert!(err.to_string().contains("unknown codec 7"), "{err}");
}

#[test]
fn raw_codec_is_rejected() {
    assert!(Codec::from_u8(0).unwrap_err().is_unsupported());
}
