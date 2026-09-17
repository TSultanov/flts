use super::*;
use proptest::prelude::*;

const _: () = assert!(Uuid::NIL.is_nil());
const _: () = assert!(Uuid::v7_at(0, 0, 0).version() == 7);
const _: () = assert!(Uuid::v7_at(0, 0, 0).variant() == 0b10);

#[test]
fn the_layout_puts_every_field_where_the_standard_says_it_goes() {
    let uuid = Uuid::v7_at(0x0199_c4f2_8a1c, 0xabc, 0x3edc_ba98_7654_3210);

    assert_eq!(uuid.unix_ms(), 0x0199_c4f2_8a1c);
    assert_eq!(uuid.version(), 7);
    assert_eq!(uuid.sequence(), 0xabc);
    assert_eq!(uuid.variant(), 0b10);
    assert_eq!(uuid.entropy(), 0x3edc_ba98_7654_3210);
}

#[test]
fn a_field_wider_than_its_place_does_not_reach_into_the_next() {
    let uuid = Uuid::v7_at(u64::MAX, u16::MAX, u64::MAX);

    assert_eq!(uuid.unix_ms(), MS_MASK);
    assert_eq!(uuid.version(), 7);
    assert_eq!(uuid.sequence(), SEQUENCE_MASK);
    assert_eq!(uuid.variant(), 0b10);
    assert_eq!(uuid.entropy(), ENTROPY_MASK);
}

#[test]
fn the_sequence_climbs_while_the_millisecond_stands_still() {
    assert_eq!(following((10, 0), 10), (10, 1));
    assert_eq!(following((10, 1), 10), (10, 2));
}

#[test]
fn the_sequence_starts_over_when_the_millisecond_moves_on() {
    assert_eq!(following((10, 7), 11), (11, 0));
}

#[test]
fn a_millisecond_that_runs_out_of_sequence_borrows_the_next_one() {
    assert_eq!(following((10, SEQUENCE_MASK), 10), (11, 0));
}

#[test]
fn a_clock_that_slipped_backwards_still_mints_upwards() {
    assert_eq!(following((10, 3), 4), (10, 4));
    assert_eq!(following((10, 3), 0), (10, 4));
}

#[test]
fn every_minted_uuid_is_a_version_seven_uuid_of_the_right_variant() {
    for _ in 0..64 {
        let uuid = Uuid::now_v7();
        assert_eq!(uuid.version(), 7, "{uuid}");
        assert_eq!(uuid.variant(), 0b10, "{uuid}");
        assert!(!uuid.is_nil(), "{uuid}");
    }
}

#[test]
fn every_uuid_minted_in_a_row_is_larger_than_the_one_before_it() {
    let minted: Vec<Uuid> = (0..512).map(|_| Uuid::now_v7()).collect();

    for pair in minted.windows(2) {
        assert!(pair[0] < pair[1], "{} then {}", pair[0], pair[1]);
    }
}

#[test]
fn a_minted_uuid_is_stamped_with_the_moment_it_was_minted() {
    let before = wall_clock_ms();
    let uuid = Uuid::now_v7();
    let after = wall_clock_ms();

    assert!(
        uuid.unix_ms() >= before,
        "{} is before {before}",
        uuid.unix_ms()
    );
    assert!(
        uuid.unix_ms() <= after + 1000,
        "{} is long after {after}",
        uuid.unix_ms()
    );
}

fn wall_clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[test]
fn the_nil_uuid_is_zero_and_prints_as_nothing_at_all() {
    assert!(Uuid::NIL.is_nil());
    assert_eq!(Uuid::NIL.to_u128(), 0);
    assert_eq!(
        Uuid::NIL.to_string(),
        "00000000-0000-0000-0000-000000000000"
    );
    assert!(!Uuid::v7_at(0, 0, 0).is_nil());
}

#[test]
fn a_uuid_prints_in_the_form_every_other_tool_reads() {
    let uuid = Uuid::from_u128(0x0199_c4f2_8a1c_7f3b_9d0e_5a6c_1b2d_3e4f);

    assert_eq!(uuid.to_string(), "0199c4f2-8a1c-7f3b-9d0e-5a6c1b2d3e4f");
    assert_eq!(format!("{uuid:?}"), "0199c4f2-8a1c-7f3b-9d0e-5a6c1b2d3e4f");
    assert_eq!(uuid.to_u128(), 0x0199_c4f2_8a1c_7f3b_9d0e_5a6c_1b2d_3e4f);
}

#[test]
fn a_text_of_the_wrong_length_is_not_a_uuid() {
    let short = "0199c4f2-8a1c-7f3b-9d0e-5a6c1b2d3e"
        .parse::<Uuid>()
        .unwrap_err();
    assert!(
        short.to_string().contains("a uuid is 36 characters"),
        "{short}"
    );
    assert!(short.to_string().contains("is 34"), "{short}");

    let long = "0199c4f2-8a1c-7f3b-9d0e-5a6c1b2d3e4f0"
        .parse::<Uuid>()
        .unwrap_err();
    assert!(long.to_string().contains("is 37"), "{long}");
}

#[test]
fn a_hyphen_out_of_place_is_refused_by_position() {
    let err = "0199c4f28-a1c-7f3b-9d0e-5a6c1b2d3e4f"
        .parse::<Uuid>()
        .unwrap_err();

    assert!(
        err.to_string().contains("a uuid has a hyphen at 8"),
        "{err}"
    );
    assert!(err.to_string().contains("'8'"), "{err}");
}

#[test]
fn a_character_that_is_not_hexadecimal_is_refused_where_it_stands() {
    let err = "0199c4f2-8a1c-7f3b-9d0e-5a6c1b2d3exf"
        .parse::<Uuid>()
        .unwrap_err();

    assert!(err.to_string().contains("a uuid is hexadecimal"), "{err}");
    assert!(err.to_string().contains("'x' at 34"), "{err}");
}

proptest! {
    #[test]
    fn every_uuid_finds_its_way_back_from_its_own_text(raw in any::<u128>()) {
        let uuid = Uuid::from_u128(raw);

        prop_assert_eq!(uuid.to_u128(), raw);
        prop_assert_eq!(uuid.to_string().len(), 36);
        prop_assert_eq!(uuid.to_string().parse::<Uuid>().unwrap(), uuid);
    }

    #[test]
    fn a_uuid_built_from_three_fields_hands_all_three_back(
        unix_ms in 0u64..=MS_MASK,
        sequence in 0u16..=SEQUENCE_MASK,
        entropy in 0u64..=ENTROPY_MASK,
    ) {
        let uuid = Uuid::v7_at(unix_ms, sequence, entropy);

        prop_assert_eq!(uuid.unix_ms(), unix_ms);
        prop_assert_eq!(uuid.sequence(), sequence);
        prop_assert_eq!(uuid.entropy(), entropy);
        prop_assert_eq!(uuid.version(), 7);
        prop_assert_eq!(uuid.variant(), 0b10);
    }

    #[test]
    fn a_later_millisecond_always_makes_a_larger_uuid(
        earlier in 0u64..MS_MASK,
        step in 1u64..=1_000_000,
        first in any::<u64>(),
        second in any::<u64>(),
    ) {
        let later = earlier.saturating_add(step).min(MS_MASK);

        prop_assert!(
            Uuid::v7_at(earlier, SEQUENCE_MASK, first) < Uuid::v7_at(later, 0, second)
        );
    }

    #[test]
    fn the_sequence_never_repeats_a_millisecond_it_has_already_left(
        last_ms in 0u64..=1_000_000,
        last_sequence in 0u16..=SEQUENCE_MASK,
        now in 0u64..=1_000_000,
    ) {
        let next = following((last_ms, last_sequence), now);

        prop_assert!(next > (last_ms, last_sequence));
        prop_assert!(next.1 <= SEQUENCE_MASK);
    }
}
