//! Property-test configuration and the generators every module shares.

use proptest::prelude::{Just, Strategy, any, prop_oneof};
use proptest::sample::select;

use crate::id::Uuid;
use proptest::test_runner::Config;

pub fn scaled(divisor: u32) -> Config {
    let base = Config::default();
    Config {
        cases: (base.cases / divisor).max(1),
        ..base
    }
}

pub fn chunk_rows() -> impl Strategy<Value = usize> {
    prop_oneof![
        3 => 1usize..=4,
        3 => 5usize..=16,
        1 => Just(64usize),
    ]
}

pub fn interesting_u64() -> impl Strategy<Value = u64> {
    prop_oneof![
        6 => any::<u64>(),
        1 => Just(0),
        1 => Just(u64::MAX),
        1 => Just(u64::MAX - 1),
        1 => (0..64u32).prop_map(|shift| 1u64 << shift),
    ]
}

pub fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        1 => Just(String::new()),
        6 => "[a-z ]{0,12}",
        2 => "[αβγδεζηθ]{1,4}",
        1 => "[a-z]{200,260}",
    ]
}

pub fn optional_text() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        1 => Just(None),
        3 => text().prop_map(Some),
    ]
}

pub fn row_index() -> impl Strategy<Value = usize> {
    0usize..64
}

pub fn interesting_u32() -> impl Strategy<Value = u32> {
    prop_oneof![
        6 => any::<u32>(),
        1 => Just(0),
        1 => Just(u32::MAX),
        1 => Just(u32::MAX - 1),
        1 => (0..32u32).prop_map(|shift| 1u32 << shift),
    ]
}

pub fn uuid() -> impl Strategy<Value = Uuid> {
    prop_oneof![
        6 => any::<u128>().prop_map(Uuid::from_u128),
        1 => Just(Uuid::NIL),
        1 => Just(Uuid::from_u128(u128::MAX)),
        1 => (0..128u32).prop_map(|shift| Uuid::from_u128(1u128 << shift)),
    ]
}

const SHARED_LONG: [&str; 2] = [
    "a title far too long to be carried inline",
    "another title too long to be carried inline",
];

pub fn indexable_text() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => select(vec!["", "a", "quiet", "the same title twice"]).prop_map(str::to_owned),
        3 => "[a-z]{17,24}",
        2 => "[αβγδεζηθ]{9,12}",
        2 => select(SHARED_LONG.to_vec()).prop_map(str::to_owned),
        1 => "[a-z]{200,260}",
    ]
}

pub fn indexable_optional_text() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        1 => Just(None),
        3 => indexable_text().prop_map(Some),
    ]
}

#[cfg(test)]
#[path = "props_test.rs"]
mod props_test;
