use super::*;
use proptest::prelude::*;

#[test]
fn matches_known_vectors() {
    assert_eq!(fnv1a(b""), OFFSET_BASIS);
    assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a(b"foobar"), 0x85944171f73967e8);
}

#[test]
fn detects_single_bit_flips() {
    let clean = fnv1a(b"the quick brown fox");
    let dirty = fnv1a(b"the quick brown fox\x00");
    assert_ne!(clean, dirty);
    assert_ne!(fnv1a(b"ab"), fnv1a(b"ba"));
}

fn fnv1a_from_the_specification(bytes: &[u8]) -> u64 {
    let mut hash = 14_695_981_039_346_656_037u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash
}

proptest! {
    #[test]
    fn fnv1a_matches_its_definition_byte_for_byte(bytes: Vec<u8>) {
        prop_assert_eq!(fnv1a(&bytes), fnv1a_from_the_specification(&bytes));
    }

    #[test]
    fn changing_any_single_byte_changes_the_checksum(
        bytes in prop::collection::vec(any::<u8>(), 1..64),
        at: prop::sample::Index,
        mask in 1u8..=u8::MAX,
    ) {
        let at = at.index(bytes.len());
        let mut changed = bytes.clone();
        changed[at] ^= mask;
        prop_assert_ne!(fnv1a(&bytes), fnv1a(&changed));
    }

}
