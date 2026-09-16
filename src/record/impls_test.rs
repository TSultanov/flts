use super::*;
use crate::id::Id;

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
    roundtrip(Id::<Chapter>::from_index(9));
}

#[test]
fn optional_join_costs_the_same_as_a_required_one() {
    assert_eq!(
        <Option<Id<Chapter>> as Record>::SIZE,
        <Id<Chapter> as Record>::SIZE
    );
    roundtrip(Some(Id::<Chapter>::from_index(4)));
    roundtrip(None::<Id<Chapter>>);
}

#[test]
fn null_decodes_as_none_not_as_an_id() {
    let mut buf = [0u8; 8];
    None::<Id<Chapter>>.encode(&mut buf);
    assert_eq!(buf, u64::MAX.to_le_bytes());
    assert_eq!(<Option<Id<Chapter>> as Record>::decode(&buf), None);
}
