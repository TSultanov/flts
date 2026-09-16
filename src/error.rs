//! What went wrong, named by what the caller should do about it.

use std::fmt;
use std::io;

use crate::storage::CommitToken;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
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

    pub fn is_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported(_))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
            Self::Io(err) => Some(err),
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
