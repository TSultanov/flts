use super::*;
use crate::id::{Id, Uuid};
use crate::test_utils::props;
use proptest::prelude::*;

struct Chapter;

fn roundtrip<T: Record + PartialEq + std::fmt::Debug>(value: T) {
    let mut buf = vec![0u8; T::SIZE];
    value.encode(&mut buf);
    assert_eq!(T::decode(&buf), value);
}

#[test]
fn primitives_roundtrip_at_their_declared_size() {
    roundtrip(0u8);
    roundtrip(u32::MAX);
    roundtrip(-1i64);
    roundtrip(1.5f64);
    roundtrip(true);
    roundtrip(false);
    assert_eq!(<u32 as Record>::SIZE, 4);
    assert_eq!(<bool as Record>::SIZE, 1);
}

#[test]
fn encoding_is_little_endian_regardless_of_host() {
    let mut buf = [0u8; 4];
    0x0102_0304u32.encode(&mut buf);
    assert_eq!(buf, [0x04, 0x03, 0x02, 0x01]);
}

#[test]
fn ids_roundtrip() {
    roundtrip(Id::<Chapter>::mint());
}

#[test]
fn an_id_is_sixteen_bytes_of_the_uuid_it_names() {
    let id = Id::<Chapter>::from_uuid(Uuid::from_u128(0x0199_c4f2_8a1c_7f3b_9d0e_5a6c_1b2d_3e4f));
    let mut buf = [0u8; 16];
    id.encode(&mut buf);

    assert_eq!(<Id<Chapter> as Record>::SIZE, 16);
    assert_eq!(
        buf,
        0x0199_c4f2_8a1c_7f3b_9d0e_5a6c_1b2d_3e4fu128.to_le_bytes()
    );
}

#[test]
fn optional_join_costs_the_same_as_a_required_one() {
    assert_eq!(
        <Option<Id<Chapter>> as Record>::SIZE,
        <Id<Chapter> as Record>::SIZE
    );
    roundtrip(Some(Id::<Chapter>::mint()));
    roundtrip(None::<Id<Chapter>>);
}

#[test]
fn the_nil_id_decodes_as_none_not_as_an_id() {
    let mut buf = [0u8; 16];
    None::<Id<Chapter>>.encode(&mut buf);

    assert_eq!(buf, [0u8; 16]);
    assert_eq!(<Option<Id<Chapter>> as Record>::decode(&buf), None);
}

proptest! {
    #[test]
    fn every_byte_pattern_decodes_without_panicking(raw in props::interesting_u64()) {
        let bytes = raw.to_le_bytes();
        <u8 as Record>::decode(&bytes[..1]);
        <bool as Record>::decode(&bytes[..1]);
        <u16 as Record>::decode(&bytes[..2]);
        <u32 as Record>::decode(&bytes[..4]);
        <u64 as Record>::decode(&bytes);
        <i8 as Record>::decode(&bytes[..1]);
        <i16 as Record>::decode(&bytes[..2]);
        <i32 as Record>::decode(&bytes[..4]);
        <i64 as Record>::decode(&bytes);
        <f32 as Record>::decode(&bytes[..4]);
        <f64 as Record>::decode(&bytes);

        let wide = (raw as u128) << 64 | raw as u128;
        <Id<Chapter> as Record>::decode(&wide.to_le_bytes());
        <Option<Id<Chapter>> as Record>::decode(&wide.to_le_bytes());
    }

    #[test]
    fn every_integer_record_round_trips_through_its_own_encoding(
        a: u8, b: u16, c: u32, d: u64, e: i8, f: i16, g: i32, h: i64, bit: bool,
    ) {
        roundtrip(a);
        roundtrip(b);
        roundtrip(c);
        roundtrip(d);
        roundtrip(e);
        roundtrip(f);
        roundtrip(g);
        roundtrip(h);
        roundtrip(bit);
    }

    #[test]
    fn floats_round_trip_bit_for_bit_including_nan(narrow: u32, wide: u64) {
        let mut four = [0u8; 4];
        f32::from_bits(narrow).encode(&mut four);
        prop_assert_eq!(<f32 as Record>::decode(&four).to_bits(), narrow);

        let mut eight = [0u8; 8];
        f64::from_bits(wide).encode(&mut eight);
        prop_assert_eq!(<f64 as Record>::decode(&eight).to_bits(), wide);
    }

    #[test]
    fn a_re_encoded_record_reproduces_the_bytes_it_came_from(raw in props::interesting_u64()) {
        let bytes = raw.to_le_bytes();
        let mut again = [0u8; 8];

        <u64 as Record>::decode(&bytes).encode(&mut again);
        prop_assert_eq!(again, bytes);

        <i64 as Record>::decode(&bytes).encode(&mut again);
        prop_assert_eq!(again, bytes);

    }

    #[test]
    fn a_re_encoded_id_reproduces_the_sixteen_bytes_it_came_from(raw in any::<u128>()) {
        let bytes = raw.to_le_bytes();
        let mut again = [0u8; 16];

        <Id<Chapter> as Record>::decode(&bytes).encode(&mut again);
        prop_assert_eq!(again, bytes);

        if raw != 0 {
            <Option<Id<Chapter>> as Record>::decode(&bytes).encode(&mut again);
            prop_assert_eq!(again, bytes);
        }
    }

    #[test]
    fn a_bool_reads_as_false_only_for_a_zero_byte_and_writes_back_canonically(byte: u8) {
        let value = <bool as Record>::decode(&[byte]);
        prop_assert_eq!(value, byte != 0);

        let mut out = [0u8; 1];
        value.encode(&mut out);
        prop_assert_eq!(out[0], u8::from(byte != 0));
    }

    #[test]
    fn only_the_nil_id_round_trips_as_none(raw in any::<u128>()) {
        let bytes = raw.to_le_bytes();
        let decoded = <Option<Id<Chapter>> as Record>::decode(&bytes);

        if raw == 0 {
            prop_assert_eq!(decoded, None);
        } else {
            prop_assert_eq!(decoded.map(|id| id.uuid().to_u128()), Some(raw));
        }
    }
}
