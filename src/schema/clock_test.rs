use proptest::prelude::*;

use super::*;

fn wrote(raw: u64, counter: u64) -> Dot {
    Dot::new(WriterId::from_raw(raw), counter)
}

fn seen(dots: &[Dot]) -> VersionVector {
    VersionVector::from_dots(dots)
}

fn minted() -> impl Strategy<Value = Dot> {
    (
        prop_oneof![
            (0u64..3).prop_map(WriterId::from_raw),
            Just(WriterId::ANONYMOUS)
        ],
        prop_oneof![1u64..6, any::<u64>()],
    )
        .prop_map(|(writer, counter)| Dot::new(writer, counter))
}

fn ticked() -> impl Strategy<Value = VersionVector> {
    prop::collection::vec(minted(), 0..8).prop_map(|dots| seen(&dots))
}

#[test]
fn a_clock_that_has_seen_nothing_has_seen_nothing_from_anyone() {
    let clock = VersionVector::default();

    assert!(clock.is_empty());
    assert_eq!(clock.writers(), 0);
    assert_eq!(clock.counter_of(WriterId::from_raw(1)), 0);
    assert!(!clock.dominates(wrote(1, 1)));
}

#[test]
fn a_clock_covers_every_count_up_to_the_one_it_saw_and_nothing_past_it() {
    let clock = seen(&[wrote(1, 5)]);

    assert!(!clock.is_empty());
    assert_eq!(clock.writers(), 1);
    assert!(clock.dominates(wrote(1, 5)));
    assert!(clock.dominates(wrote(1, 1)));
    assert!(!clock.dominates(wrote(1, 6)));
    assert!(
        !clock.dominates(wrote(2, 5)),
        "one writer counting to five says nothing about another"
    );
}

#[test]
fn observing_an_older_dot_never_lowers_what_a_clock_has_already_seen() {
    let mut clock = seen(&[wrote(1, 9)]);

    clock.observe(wrote(1, 2));

    assert_eq!(clock.counter_of(WriterId::from_raw(1)), 9);
    assert_eq!(
        clock.writers(),
        1,
        "the same writer twice is still one writer"
    );
}

#[test]
fn a_tick_hands_back_a_dot_the_clock_did_not_cover_and_then_does() {
    let mut clock = VersionVector::default();
    let writer = WriterId::from_device_id("the reading room");

    let dot = clock.tick(writer).unwrap();

    assert_eq!(dot.writer(), writer);
    assert!(clock.dominates(dot));
    assert!(
        !clock.dominates(Dot::new(writer, dot.counter() + 1)),
        "a tick covers what it minted and not a count past it"
    );
}

#[test]
fn two_ticks_of_one_writer_never_name_the_same_write() {
    let mut clock = VersionVector::default();
    let writer = WriterId::from_raw(7);

    let first = clock.tick(writer).unwrap();
    let second = clock.tick(writer).unwrap();

    assert!(
        first < second,
        "two writes in the same millisecond still happened in an order: {first:?} then {second:?}"
    );
}

#[test]
fn a_tick_outruns_a_count_the_writer_itself_already_reached() {
    let mut clock = seen(&[wrote(7, u64::MAX / 2)]);

    let dot = clock.tick(WriterId::from_raw(7)).unwrap();

    assert!(
        dot.counter() > u64::MAX / 2,
        "a clock read from a file may be ahead of this machine's wall clock, \
         and a dot that repeats a count is two writes wearing one name"
    );
}

#[test]
fn a_tick_outruns_every_count_the_clock_has_seen_and_not_only_its_own() {
    let mut clock = seen(&[wrote(1, u64::MAX / 2), wrote(2, 3)]);

    let dot = clock.tick(WriterId::from_raw(4)).unwrap();

    assert!(
        dot.counter() > u64::MAX / 2,
        "a write made after seeing someone else's must carry the higher dot, \
         or newest-wins says the opposite of causally-later whenever a clock runs fast; \
         a quiet writer in the same clock must not be the one that sets the pace"
    );
}

#[test]
fn a_clock_that_has_counted_as_far_as_it_can_is_refused_rather_than_made_to_repeat() {
    let mut clock = seen(&[wrote(7, u64::MAX)]);

    let err = clock
        .tick(WriterId::from_raw(3))
        .expect_err("a counter that wrapped would mint a dot that was already spent");

    assert!(err.to_string().contains("no counter left"), "{err}");
}

#[test]
fn absorbing_a_clock_takes_the_later_count_of_every_writer_either_one_saw() {
    let mut mine = seen(&[wrote(1, 9), wrote(2, 1)]);

    mine.absorb(&seen(&[wrote(2, 4), wrote(3, 6)]));

    assert_eq!(mine.counter_of(WriterId::from_raw(1)), 9);
    assert_eq!(mine.counter_of(WriterId::from_raw(2)), 4);
    assert_eq!(mine.counter_of(WriterId::from_raw(3)), 6);
    assert_eq!(mine.writers(), 3);
}

#[test]
fn a_tick_counts_from_the_wall_clock_in_milliseconds() {
    let mut clock = VersionVector::default();

    let dot = clock.tick(WriterId::from_raw(1)).unwrap();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let minted = dot.counter() >> LOGICAL_BITS;
    assert!(
        now.abs_diff(minted) < 1000,
        "a dot minted now reads as millisecond {minted}, and it is {now}"
    );
}

#[test]
fn a_millisecond_has_room_for_many_writes_before_one_borrows_the_next() {
    let ahead = 1u64 << 45;
    let mut clock = seen(&[wrote(1, ticks_of(ahead))]);

    for _ in 0..1000 {
        clock.tick(WriterId::from_raw(1)).unwrap();
    }

    assert_eq!(
        clock.frontier() >> LOGICAL_BITS,
        ahead,
        "a thousand writes inside one millisecond must not spend a thousand milliseconds"
    );
}

#[test]
fn every_millisecond_in_range_counts_higher_than_the_one_before_it() {
    for ms in [0u64, 1, 1 << 40, (1 << 48) - 2] {
        assert!(
            ticks_of(ms) < ticks_of(ms + 1),
            "millisecond {ms} must count lower than the one after it"
        );
    }
}

proptest! {
    #[test]
    fn a_clock_keeps_one_count_per_writer_and_keeps_them_in_writer_order(
        dots in prop::collection::vec(minted(), 0..16),
    ) {
        let clock = seen(&dots);

        let mut writers: Vec<WriterId> = clock.dots().iter().map(|dot| dot.writer()).collect();
        let held = writers.len();
        writers.dedup();
        prop_assert_eq!(writers.len(), held, "a writer is named once");
        prop_assert!(clock.dots().windows(2).all(|two| two[0].writer() < two[1].writer()));

        for dot in &dots {
            prop_assert!(clock.dominates(*dot), "the clock saw {:?}", dot);
        }
    }

    #[test]
    fn a_clock_covers_a_dot_exactly_when_some_dot_it_saw_reached_that_far(
        dots in prop::collection::vec(minted(), 0..16),
        asked in minted(),
    ) {
        let reached = dots
            .iter()
            .filter(|dot| dot.writer() == asked.writer())
            .any(|dot| dot.counter() >= asked.counter());

        prop_assert_eq!(seen(&dots).dominates(asked), reached);
    }

    #[test]
    fn absorbing_two_clocks_says_the_same_thing_in_either_order(
        mine in ticked(),
        theirs in ticked(),
    ) {
        let mut ours = mine.clone();
        ours.absorb(&theirs);

        let mut backwards = theirs.clone();
        backwards.absorb(&mine);

        prop_assert_eq!(&ours, &backwards);
    }

    #[test]
    fn absorbing_a_clock_twice_says_no_more_than_absorbing_it_once(
        mine in ticked(),
        theirs in ticked(),
    ) {
        let mut once = mine.clone();
        once.absorb(&theirs);

        let mut twice = once.clone();
        twice.absorb(&theirs);

        prop_assert_eq!(&once, &twice);
    }

    #[test]
    fn a_clock_comes_back_from_the_dots_it_hands_out(clock in ticked()) {
        prop_assert_eq!(VersionVector::from_dots(clock.dots()), clock);
    }

    #[test]
    fn every_tick_counts_past_every_count_the_clock_had_already_seen(
        dots in prop::collection::vec(minted(), 0..8),
        raw in 0u64..4,
    ) {
        let mut clock = seen(&dots);
        let reached = clock.frontier();

        match clock.tick(WriterId::from_raw(raw)) {
            Ok(dot) => {
                prop_assert!(
                    dot.counter() > reached,
                    "{} is no later than the {reached} the clock had already seen",
                    dot.counter()
                );
                prop_assert!(clock.dominates(dot));
                for held in &dots {
                    prop_assert!(clock.dominates(*held), "the tick lost {:?}", held);
                }
            }
            Err(err) => {
                prop_assert_eq!(reached, u64::MAX, "only a spent clock refuses: {}", err);
            }
        }
    }
}
