use super::*;
use crate::storage::WriterId;

fn wrote(raw: u64, counter: u64) -> Dot {
    Dot::new(WriterId::from_raw(raw), counter)
}

#[test]
fn a_report_of_nothing_says_nothing_happened() {
    let quiet = MergeReport::default();

    assert!(quiet.is_quiet());
    assert!(!quiet.changed_anything());
    assert_eq!((quiet.taken, quiet.known), (0, 0));
}

#[test]
fn a_report_that_took_a_row_says_so_even_when_nobody_disagreed() {
    let told = MergeReport {
        taken: 3,
        known: 7,
        ..MergeReport::default()
    };

    assert!(told.is_quiet(), "nobody disagreed");
    assert!(told.changed_anything(), "but three rows are new here");
}

#[test]
fn a_report_is_not_quiet_when_two_copies_wrote_the_same_row_apart() {
    let said = MergeReport {
        concurrent: vec![Concurrent {
            entity: "book",
            id: crate::id::Uuid::from_u128(1),
            kept: wrote(2, 9),
            dropped: wrote(1, 9),
        }],
        ..MergeReport::default()
    };

    assert!(!said.is_quiet());
    assert!(
        !said.changed_anything(),
        "a row nobody took is a row that did not change"
    );
}

#[test]
fn a_report_includes_combined_source_revisions() {
    let report = MergeReport {
        combined: vec![Combined {
            entity: "book",
            id: crate::Uuid::from_u128(1),
            sources: vec![wrote(1, 1), wrote(2, 1)],
        }],
        ..MergeReport::default()
    };
    assert!(!report.is_quiet());
}
