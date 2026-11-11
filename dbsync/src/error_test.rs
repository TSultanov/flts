use super::*;

use crate::storage::WriterId;
use proptest::prelude::*;
use std::error::Error as _;

fn token(generation: u64, content_id: u64) -> CommitToken {
    CommitToken {
        generation,
        content_id,
        writer_id: WriterId::ANONYMOUS,
        file_uuid: 1,
    }
}

#[test]
fn each_variant_names_a_different_recovery() {
    let corrupt = Error::corrupt("chunk", "failed checksum");
    let unsupported = Error::unsupported("unknown codec 7");

    assert!(corrupt.is_corrupt());
    assert!(!corrupt.is_unsupported());
    assert!(unsupported.is_unsupported());
    assert!(
        !unsupported.is_corrupt(),
        "overwriting a file we are merely too old to read would destroy it"
    );
}

#[test]
fn display_keeps_the_detail_a_human_needs() {
    assert_eq!(
        Error::corrupt("directory", "failed checksum").to_string(),
        "corrupt directory: failed checksum"
    );
    assert_eq!(
        Error::unsupported("unknown codec 7").to_string(),
        "unsupported: unknown codec 7"
    );
    assert_eq!(
        Error::invalid("no column named \"x\"").to_string(),
        "no column named \"x\""
    );

    let conflict = Error::Conflict {
        expected: token(2, 0xaa),
        found: token(2, 0xbb),
    };
    let message = conflict.to_string();
    assert!(message.contains("moved to generation 2"), "{message}");
    assert!(message.contains("0x00000000000000bb"), "{message}");
}

#[test]
fn io_errors_keep_their_kind_and_source() {
    let err: Error = io::Error::new(io::ErrorKind::PermissionDenied, "nope").into();
    assert!(matches!(&err, Error::Io(io) if io.kind() == io::ErrorKind::PermissionDenied));
    assert!(std::error::Error::source(&err).is_some());
    assert!(std::error::Error::source(&Error::invalid("x")).is_none());
}

proptest! {
    #[test]
    fn every_error_prints_something_that_names_what_went_wrong(detail in "[a-z ]{1,40}") {
        let corrupt = Error::corrupt("chunk", detail.clone());
        prop_assert!(corrupt.to_string().contains("chunk"));
        prop_assert!(corrupt.to_string().contains(&detail));
        prop_assert!(corrupt.is_corrupt());
        prop_assert!(!corrupt.is_unsupported());
        prop_assert!(corrupt.source().is_none());

        let unsupported = Error::unsupported(detail.clone());
        prop_assert!(unsupported.to_string().contains(&detail));
        prop_assert!(unsupported.is_unsupported());
        prop_assert!(!unsupported.is_corrupt());

        let invalid = Error::invalid(detail.clone());
        prop_assert!(invalid.to_string().contains(&detail));
        prop_assert!(!invalid.is_corrupt());
        prop_assert!(!invalid.is_unsupported());

        let io = Error::from(std::io::Error::other(detail.clone()));
        prop_assert!(io.to_string().contains(&detail));
        prop_assert!(io.source().is_some());
    }

    #[test]
    fn a_conflict_names_both_generations_it_could_not_reconcile(
        held in 1u64..1_000_000,
        found in 1u64..1_000_000,
    ) {
        let err = Error::Conflict {
            expected: token(held, 0xaa),
            found: token(found, 0xbb),
        };
        let shown = err.to_string();
        prop_assert!(shown.contains(&held.to_string()));
        prop_assert!(shown.contains(&found.to_string()));
        prop_assert!(err.source().is_none());
    }
}
