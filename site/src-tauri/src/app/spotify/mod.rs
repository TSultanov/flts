//! Spotify integration.
//!
//! - `applescript` (macOS-only): polls the local Spotify.app via AppleScript
//!   to track the currently-playing track.
//! - `web`: optional Spotify Web API layer that adds a queue lookahead and
//!   feeds the lyrics view's "Up next" UI.

#[cfg(target_os = "macos")]
pub mod applescript;
pub mod web;
