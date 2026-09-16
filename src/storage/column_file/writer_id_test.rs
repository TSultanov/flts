use super::*;

#[test]
fn a_device_id_always_maps_to_the_same_writer() {
    let once = WriterId::from_device_id("KRZH3XY-7PH4NJK-2WQ5LMC");
    let again = WriterId::from_device_id("KRZH3XY-7PH4NJK-2WQ5LMC");
    assert_eq!(once, again, "must survive a process restart");
    assert_ne!(once, WriterId::from_device_id("OTHERID-7PH4NJK-2WQ5LMC"));
    assert!(!once.is_anonymous());
}

#[test]
fn anonymous_is_the_only_zero() {
    assert_eq!(WriterId::ANONYMOUS.raw(), 0);
    assert!(WriterId::ANONYMOUS.is_anonymous());
    assert!(!WriterId::from_device_id("").is_anonymous());
}

#[test]
fn raw_values_round_trip() {
    assert_eq!(WriterId::from_raw(42).raw(), 42);
    assert_eq!(WriterId::from_raw(42), WriterId::from_raw(42));
}

#[test]
fn debug_names_an_anonymous_writer() {
    assert_eq!(format!("{:?}", WriterId::ANONYMOUS), "WriterId(anonymous)");
    assert_eq!(
        format!("{:?}", WriterId::from_raw(0x1f)),
        "WriterId(0x000000000000001f)"
    );
}
