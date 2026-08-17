//! TLA+ trace emission.
//!
//! The `tla_trace` feature wires up `trace`, `interaction`, and `mutex`.
//! Without it the event emitters and `mutex` surfaces remain as zero-cost
//! no-ops, so production code calls them unconditionally; `interaction` needs
//! none, its consumers being feature-gated harnesses.

#[cfg(feature = "tla_trace")]
mod trace;
#[cfg(feature = "tla_trace")]
pub use trace::*;

#[cfg(feature = "tla_trace")]
pub mod interaction;

#[cfg(feature = "tla_trace")]
pub mod mutex;

#[cfg(not(feature = "tla_trace"))]
mod noop;
#[cfg(not(feature = "tla_trace"))]
pub use noop::trace::*;
#[cfg(not(feature = "tla_trace"))]
pub mod mutex {
    pub use super::noop::mutex::*;
}
