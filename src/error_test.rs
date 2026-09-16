use super::*;

use crate::storage::WriterId;

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
