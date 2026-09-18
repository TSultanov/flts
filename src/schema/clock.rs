//! What a store has seen from every writer, and the next dot it may mint.

use std::time::{SystemTime, UNIX_EPOCH};

use super::version::Dot;
use crate::error::{Error, Result};
use crate::storage::WriterId;

const LOGICAL_BITS: u32 = 16;

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct VersionVector {
    seen: Vec<Dot>,
}

impl VersionVector {
    pub fn from_dots(dots: &[Dot]) -> Self {
        let mut clock = Self::default();
        for dot in dots {
            clock.observe(*dot);
        }
        clock
    }

    pub fn dots(&self) -> &[Dot] {
        &self.seen
    }

    pub fn writers(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    pub fn counter_of(&self, writer: WriterId) -> u64 {
        match self.at(writer) {
            Ok(at) => self.seen[at].counter(),
            Err(_) => 0,
        }
    }

    pub fn frontier(&self) -> u64 {
        self.seen.iter().map(|dot| dot.counter()).max().unwrap_or(0)
    }

    pub fn dominates(&self, dot: Dot) -> bool {
        dot.counter() <= self.counter_of(dot.writer())
    }

    pub fn observe(&mut self, dot: Dot) {
        match self.at(dot.writer()) {
            Ok(at) if self.seen[at].counter() < dot.counter() => self.seen[at] = dot,
            Ok(_) => {}
            Err(at) => self.seen.insert(at, dot),
        }
    }

    pub fn absorb(&mut self, other: &Self) {
        for dot in &other.seen {
            self.observe(*dot);
        }
    }

    pub fn tick(&mut self, writer: WriterId) -> Result<Dot> {
        let frontier = self.frontier();
        let now = now_ticks();
        let counter = match now > frontier {
            true => now,
            false => frontier
                .checked_add(1)
                .ok_or_else(|| Error::invalid("this clock has no counter left to mint with"))?,
        };

        let dot = Dot::new(writer, counter);
        self.observe(dot);
        Ok(dot)
    }

    fn at(&self, writer: WriterId) -> std::result::Result<usize, usize> {
        self.seen.binary_search_by_key(&writer, |dot| dot.writer())
    }
}

const fn ticks_of(unix_ms: u64) -> u64 {
    unix_ms << LOGICAL_BITS
}

fn now_ticks() -> u64 {
    ticks_of(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64),
    )
}

#[cfg(test)]
#[path = "clock_test.rs"]
mod clock_test;
