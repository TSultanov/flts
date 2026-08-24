//! Spotify integration.
//!
//! - `applescript` (macOS-only): polls the local Spotify.app via AppleScript
//!   to track the currently-playing track.
//! - `cdp`: fetches first-party lyrics by evaluating a request inside the
//!   running Spotify desktop app's webview over its DevTools bridge — no
//!   remote-API credentials, no ban surface.
//! - `web`: optional Spotify Web API layer that adds a queue lookahead and
//!   feeds the lyrics view's "Up next" UI.

#[cfg(target_os = "macos")]
pub mod applescript;
pub mod cdp;
pub mod web;
