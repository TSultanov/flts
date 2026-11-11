use super::*;
use proptest::test_runner::RngSeed;

#[test]
fn the_property_seed_is_pinned_unless_a_soak_run_says_otherwise() {
    if std::env::var_os("DBSYNC_PROPTEST_SOAK").is_some() {
        return;
    }
    let config = Config::default();
    assert!(
        matches!(config.rng_seed, RngSeed::Fixed(_)),
        "PROPTEST_RNG_SEED is not reaching this process, so every mutant would \
         be classified from a different random sequence"
    );
    assert!(config.failure_persistence.is_none());
}

#[test]
fn a_scaled_budget_divides_the_global_one_and_never_reaches_zero() {
    let whole = Config::default().cases;
    assert_eq!(scaled(1).cases, whole);
    assert_eq!(scaled(4).cases, whole / 4);
    assert_eq!(scaled(u32::MAX).cases, 1);
}
