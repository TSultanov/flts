//! A version 7 UUID: the identity a row is minted with and keeps for its whole life.

use std::fmt;
use std::str::FromStr;
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use rand::Rng;

use crate::error::{Error, Result};

const VERSION: u128 = 7;
const VARIANT: u128 = 0b10;
const MS_SHIFT: u32 = 80;
const VERSION_SHIFT: u32 = 76;
const SEQUENCE_SHIFT: u32 = 64;
const VARIANT_SHIFT: u32 = 62;
const MS_MASK: u64 = (1 << 48) - 1;
const SEQUENCE_MASK: u16 = (1 << 12) - 1;
const ENTROPY_MASK: u64 = (1 << 62) - 1;
const TEXT_LEN: usize = 36;
const HYPHENS: [usize; 4] = [8, 13, 18, 23];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uuid(u128);

static MINTED: Mutex<(u64, u16)> = Mutex::new((0, 0));

impl Uuid {
    pub const NIL: Uuid = Uuid(0);

    pub fn now_v7() -> Self {
        let (unix_ms, sequence) = next_after(now_ms());
        Self::v7_at(unix_ms, sequence, rand::rng().random())
    }

    pub const fn from_u128(raw: u128) -> Self {
        Self(raw)
    }

    pub const fn to_u128(self) -> u128 {
        self.0
    }

    pub const fn is_nil(self) -> bool {
        self.0 == 0
    }

    pub const fn unix_ms(self) -> u64 {
        (self.0 >> MS_SHIFT) as u64 & MS_MASK
    }

    pub const fn version(self) -> u8 {
        ((self.0 >> VERSION_SHIFT) & 0xf) as u8
    }

    pub const fn sequence(self) -> u16 {
        ((self.0 >> SEQUENCE_SHIFT) as u16) & SEQUENCE_MASK
    }

    pub const fn variant(self) -> u8 {
        ((self.0 >> VARIANT_SHIFT) & 0b11) as u8
    }

    pub const fn entropy(self) -> u64 {
        self.0 as u64 & ENTROPY_MASK
    }

    pub(crate) const fn v7_at(unix_ms: u64, sequence: u16, entropy: u64) -> Self {
        Self(
            ((unix_ms & MS_MASK) as u128) << MS_SHIFT
                | VERSION << VERSION_SHIFT
                | ((sequence & SEQUENCE_MASK) as u128) << SEQUENCE_SHIFT
                | VARIANT << VARIANT_SHIFT
                | (entropy & ENTROPY_MASK) as u128,
        )
    }
}

const fn following(last: (u64, u16), now: u64) -> (u64, u16) {
    let (last_ms, last_sequence) = last;
    if now > last_ms {
        (now, 0)
    } else if last_sequence < SEQUENCE_MASK {
        (last_ms, last_sequence + 1)
    } else {
        (last_ms + 1, 0)
    }
}

fn next_after(now: u64) -> (u64, u16) {
    let mut minted = MINTED.lock().unwrap_or_else(PoisonError::into_inner);
    *minted = following(*minted, now);
    *minted
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            (self.0 >> 96) as u32,
            (self.0 >> 80) as u16,
            (self.0 >> 64) as u16,
            (self.0 >> 48) as u16,
            self.0 as u64 & 0xffff_ffff_ffff,
        )
    }
}

impl fmt::Debug for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for Uuid {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        if text.len() != TEXT_LEN {
            return Err(Error::invalid(format!(
                "a uuid is {TEXT_LEN} characters, {text:?} is {}",
                text.len()
            )));
        }

        let mut raw = 0u128;
        for (at, byte) in text.bytes().enumerate() {
            if HYPHENS.contains(&at) {
                if byte != b'-' {
                    return Err(Error::invalid(format!(
                        "a uuid has a hyphen at {at}, {text:?} has {:?}",
                        byte as char
                    )));
                }
                continue;
            }
            let digit = char::from(byte).to_digit(16).ok_or_else(|| {
                Error::invalid(format!(
                    "a uuid is hexadecimal, {text:?} has {:?} at {at}",
                    byte as char
                ))
            })?;
            raw = raw * 16 + digit as u128;
        }
        Ok(Self(raw))
    }
}

#[cfg(test)]
#[path = "uuid_test.rs"]
mod uuid_test;
