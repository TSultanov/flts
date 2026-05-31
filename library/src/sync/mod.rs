//! Native Syncthing-based device synchronization.
//!
//! - [`control`] — the REST control client (trait + HTTP impl + mock).
//! - [`engine`] — brings up the embedded engine via FFI and configures it
//!   (folder + discovery). Feature-gated behind `sync-engine` so the core
//!   library test loop stays Go-free.
//!
//! The merge/conflict half of "sync" already lives elsewhere: the persistence
//! layer detects and union-merges Syncthing `.sync-conflict-*` files (see
//! [`crate::library::library_card`], [`crate::card::Card::merge`]). This module
//! owns only the *transport*.

pub mod control;
pub mod reconcile;
pub mod roster;

#[cfg(feature = "sync-engine")]
pub mod engine;

/// Trace-harness scenarios that drive the roster mesh and emit NDJSON traces for
/// the `spec/roster/` TLA+ spec. Test-only, and gated on `tla_trace` (needs the
/// `engine` glue, so also `sync-engine`).
#[cfg(all(test, feature = "tla_trace", feature = "sync-engine"))]
mod trace_harness;
