//! TLA+ trace emission.
//!
//! The `tla_trace` feature enables roster and interaction events; without it
//! the roster emitter is a no-op.

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
