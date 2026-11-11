//! What went wrong, named by what the caller should do about it.

use std::fmt;
use std::io;

use crate::storage::CommitToken;

pub type Result<T> = std::result::Result<T, Error>;

const FORKED: &str = "a dot identifies different revisions";

#[derive(Debug)]
pub enum Error {
    SchemaMismatch {
        expected: Box<crate::schema::SchemaMetadata>,
        actual: Box<crate::schema::SchemaMetadata>,
    },
    CompactionUncertain {
        expected: CommitToken,
        attempted: CommitToken,
        source: io::Error,
    },

    CommitUncertain {
        attempt: Box<crate::storage::CommitAttempt>,
        source: io::Error,
    },
    Corrupt {
        record: &'static str,
        detail: String,
    },
    Unsupported(String),
    Conflict {
        expected: CommitToken,
        found: CommitToken,
    },
    Invalid(String),
    Io(io::Error),
}

impl Error {
    pub fn corrupt(record: &'static str, detail: impl Into<String>) -> Self {
        Self::Corrupt {
            record,
            detail: detail.into(),
        }
    }

    pub fn unsupported(detail: impl Into<String>) -> Self {
        Self::Unsupported(detail.into())
    }

    pub fn invalid(detail: impl Into<String>) -> Self {
        Self::Invalid(detail.into())
    }

    pub fn is_corrupt(&self) -> bool {
        matches!(self, Self::Corrupt { .. })
    }

    /// Two revisions under one dot: two writers have used one identity.
    pub(crate) fn forked() -> Self {
        Self::corrupt("revision", FORKED)
    }

    pub fn is_fork(&self) -> bool {
        matches!(self, Self::Corrupt { record: "revision", detail } if detail == FORKED)
    }

    pub fn is_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported(_) | Self::SchemaMismatch { .. })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SchemaMismatch { expected, actual } => write!(
                f,
                "schema mismatch: expected {} version {}, found {} version {}{}",
                expected.name,
                expected.version,
                actual.name,
                actual.version,
                if expected.descriptor != actual.descriptor {
                    " (descriptor differs)"
                } else {
                    ""
                }
            ),
            Self::CompactionUncertain { source, .. } => write!(
                f,
                "compaction was replaced but durability is uncertain: {source}"
            ),
            Self::CommitUncertain { attempt, source } => write!(
                f,
                "publication of generation {} is uncertain: {source}",
                attempt.attempted.generation
            ),
            Self::Corrupt { record, detail } => write!(f, "corrupt {record}: {detail}"),
            Self::Unsupported(detail) => write!(f, "unsupported: {detail}"),
            Self::Conflict { expected, found } => write!(
                f,
                "column file moved to generation {} ({:#018x}) while holding generation {} ({:#018x})",
                found.generation, found.content_id, expected.generation, expected.content_id
            ),
            Self::Invalid(detail) => write!(f, "{detail}"),
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CommitUncertain { source, .. } => Some(source),
            Self::Io(err) => Some(err),
            Self::CompactionUncertain { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

#[cfg(test)]
#[path = "error_test.rs"]
mod error_test;
