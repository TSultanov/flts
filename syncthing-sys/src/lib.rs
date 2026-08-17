//! FFI bindings to the embedded Syncthing engine (`syncthing-core`, v1.30.0).
//!
//! Keep the surface at `start`/`stop`/`ping`; everything else is driven from
//! higher layers over the engine's localhost REST API.

use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::path::Path;

extern "C" {
    fn flts_st_ping() -> c_int;
    fn flts_st_start(
        home: *const c_char,
        gui_addr: *const c_char,
        api_key: *const c_char,
        hermetic: c_int,
    ) -> c_int;
    fn flts_st_stop() -> c_int;
}

/// Returns the Go archive's sentinel (`4711`), proving the FFI link is live.
pub fn ping() -> i32 {
    // SAFETY: no args, plain int return, no shared state.
    unsafe { flts_st_ping() }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    /// The Go wrapper returned a non-zero step code (see `wrapper.go`).
    Start(i32),
    /// `flts_st_stop` returned non-zero.
    Stop(i32),
    /// A passed string contained an interior NUL byte.
    NulInArg,
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Start(code) => write!(f, "syncthing engine start failed (step {code})"),
            EngineError::Stop(code) => write!(f, "syncthing engine stop failed (code {code})"),
            EngineError::NulInArg => write!(f, "argument contained an interior NUL byte"),
        }
    }
}

impl std::error::Error for EngineError {}

/// Starts the engine: state under `home`, REST/GUI on `gui_addr` keyed by
/// `api_key`. `hermetic` disables discovery/relays/NAT (tests and the Docker
/// harness); production passes `false` and configures discovery over REST.
///
/// Returns once REST is listening. Idempotent; one engine per process.
pub fn start(home: &Path, gui_addr: &str, api_key: &str, hermetic: bool) -> Result<(), EngineError> {
    let home = CString::new(home.to_string_lossy().as_bytes()).map_err(|_| EngineError::NulInArg)?;
    let addr = CString::new(gui_addr).map_err(|_| EngineError::NulInArg)?;
    let key = CString::new(api_key).map_err(|_| EngineError::NulInArg)?;
    // SAFETY: all three pointers are NUL-terminated and outlive the call.
    let rc = unsafe {
        flts_st_start(home.as_ptr(), addr.as_ptr(), key.as_ptr(), c_int::from(hermetic))
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(EngineError::Start(rc))
    }
}

/// Stops the engine cleanly. Idempotent: a no-op success when nothing runs.
pub fn stop() -> Result<(), EngineError> {
    // SAFETY: the Go side guards its state under a mutex.
    let rc = unsafe { flts_st_stop() };
    if rc == 0 {
        Ok(())
    } else {
        Err(EngineError::Stop(rc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    #[test]
    fn ping_round_trips_the_sentinel() {
        assert_eq!(ping(), 4711);
    }

    #[test]
    fn engine_starts_reports_myid_and_stops() {
        let home = unique_temp_dir();
        std::fs::create_dir_all(&home).expect("create temp home");
        let port = free_port();
        let addr = format!("127.0.0.1:{port}");
        let api_key = "flts-phase0-test-key";

        start(&home, &addr, api_key, true).expect("engine starts");

        let mut my_id: Option<String> = None;
        for _ in 0..100 {
            if let Some(body) = http_get(&addr, "/rest/system/status", api_key) {
                if let Some(id) = extract_field(&body, "myID") {
                    if !id.is_empty() {
                        my_id = Some(id);
                        break;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        // Stop before asserting so a failure can't leak a running engine.
        let stop_result = stop();
        let _ = std::fs::remove_dir_all(&home);

        let id = my_id.expect("engine reported myID over REST within timeout");
        assert!(
            id.len() >= 50 && id.contains('-'),
            "expected a Syncthing device ID, got {id:?}"
        );
        assert_eq!(stop_result, Ok(()), "engine stops cleanly");
    }

    fn unique_temp_dir() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("flts-st-test-{}-{}", std::process::id(), nanos))
    }

    /// Reserves an ephemeral port and releases it; TOCTOU-racy but localhost-only.
    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .expect("bind ephemeral port")
            .local_addr()
            .unwrap()
            .port()
    }

    /// Plain HTTP/1.1 GET (the GUI is bound with TLS disabled); body on `200`.
    fn http_get(addr: &str, path: &str, api_key: &str) -> Option<String> {
        let mut stream = TcpStream::connect(addr).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        stream.set_write_timeout(Some(Duration::from_secs(5))).ok()?;
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {addr}\r\nX-API-Key: {api_key}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).ok()?;
        let mut raw = String::new();
        stream.read_to_string(&mut raw).ok()?;
        let (head, body) = raw.split_once("\r\n\r\n")?;
        if !head.lines().next()?.contains("200") {
            return None;
        }
        Some(body.to_string())
    }

    /// Extracts a flat `"name":"value"` field without pulling in a JSON parser.
    fn extract_field(json: &str, name: &str) -> Option<String> {
        let needle = format!("\"{name}\"");
        let start = json.find(&needle)? + needle.len();
        let rest = &json[start..];
        let colon = rest.find(':')? + 1;
        let after = rest[colon..].trim_start();
        let after = after.strip_prefix('"')?;
        let end = after.find('"')?;
        Some(after[..end].to_string())
    }
}
