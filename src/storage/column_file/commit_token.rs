//! What a commit says about itself, readable without touching the directory.

use super::writer_id::WriterId;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CommitToken {
    pub generation: u64,
    pub content_id: u64,
    pub writer_id: WriterId,
    pub file_uuid: u128,
}

impl CommitToken {
    pub fn is_echo_of(self, mine: WriterId, last_committed: u64) -> bool {
        self.writer_id == mine && self.generation <= last_committed
    }
}

#[cfg(test)]
#[path = "commit_token_test.rs"]
mod commit_token_test;
