use super::*;

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
