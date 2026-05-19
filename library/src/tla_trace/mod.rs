//! TLA+ trace emission.
//!
//! With the `tla_trace` feature on, the real implementations in `trace`,
//! `interaction`, and `mutex` are wired up. Without the feature, only the
//! `tla_trace::*` (book/translation event emitters) and
//! `tla_trace::mutex::*` (TracedMutex/TracedLock) surfaces are kept as
//! zero-cost no-ops so production code can call them unconditionally.
//! The `interaction` submodule has no no-op — its consumers are test
//! harnesses gated on the feature themselves.

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
