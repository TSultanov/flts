//! TLA+ trace emission.
//!
//! The `tla_trace` feature wires up `trace` and `interaction`. Without it the
//! event emitters remain as zero-cost no-ops, so production code calls them
//! unconditionally; `interaction` needs none, its consumers being
//! feature-gated harnesses.

#[cfg(feature = "tla_trace")]
mod trace;
#[cfg(feature = "tla_trace")]
pub use trace::*;

#[cfg(feature = "tla_trace")]
pub mod interaction;

#[cfg(not(feature = "tla_trace"))]
mod noop;
#[cfg(not(feature = "tla_trace"))]
pub use noop::trace::*;
