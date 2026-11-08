//! TLA+ trace emission.
//!
//! The `tla_trace` feature enables persistence and interaction events.
//! Production builds retain zero-cost persistence event emitters.

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
