//! Property-test configuration and the generators every module shares.

use proptest::prelude::{Just, Strategy, any, prop_oneof};
use proptest::test_runner::Config;

pub fn scaled(divisor: u32) -> Config {
    let base = Config::default();
    Config {
        cases: (base.cases / divisor).max(1),
        ..base
    }
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

pub fn interesting_u32() -> impl Strategy<Value = u32> {
    prop_oneof![
        6 => any::<u32>(),
        1 => Just(0),
        1 => Just(u32::MAX),
        1 => Just(u32::MAX - 1),
        1 => (0..32u32).prop_map(|shift| 1u32 << shift),
    ]
}

#[cfg(test)]
#[path = "props_test.rs"]
mod props_test;
