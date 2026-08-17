//! Native Syncthing-based device synchronization.
//!
//! - [`control`] — the REST control client (trait + HTTP impl + mock).
//! - [`engine`] — brings up the embedded engine via FFI and configures it.
//!   Behind `sync-engine` so the core library test loop stays Go-free.
//!
//! Transport only: conflict merging lives in the persistence layer (see
//! [`crate::library::library_card`], [`crate::card::Card::merge`]).

pub mod control;
pub mod reconcile;
pub mod roster;

#[cfg(feature = "sync-engine")]
pub mod engine;

/// Trace-harness scenarios driving the roster mesh, emitting NDJSON for the
/// `spec/roster/` TLA+ spec.
#[cfg(all(test, feature = "tla_trace", feature = "sync-engine"))]
mod trace_harness;
