//! The FLTS application core, independent of the UI shell.

use library::library::file_watcher::LibraryWatcher;
use log::{info, warn};

pub mod app;
pub mod dispatch;
pub mod host;

pub use app::AppState;
pub use host::{AppHandle, Host, PlatformDirs};

/// A started core. Dropping `owner` stops the backend tasks, so the shell keeps
/// it for the process lifetime.
pub struct Core {
    pub state: AppState,
    pub owner: library::task::TaskOwner,
}

/// Builds the backend and spawns its startup work on the core runtime.
pub fn start(host: AppHandle) -> anyhow::Result<Core> {
    let runtime = host::async_runtime::handle();
    let _enter = runtime.inner().enter();

    info!("Creating watcher");
    let mut watcher = LibraryWatcher::new()?;
    let mut recv = watcher
        .take_recv()
        .expect("LibraryWatcher receiver already taken");
    let (state, owner) = AppState::new(host.clone(), watcher)?;

    info!("Spawning watch->emit bridges");
    {
        let mut rx = state.subscribe_config();
        let host = host.clone();
        host::async_runtime::spawn(async move {
            while rx.changed().await.is_ok() {
                info!("Emitting \"config_updated\"");
                let _ = host.emit("config_updated", ());
            }
        });
    }
    {
        let mut rx = state.subscribe_library();
        let host = host.clone();
        host::async_runtime::spawn(async move {
            while rx.changed().await.is_ok() {
                info!("Emitting \"library_updated\"");
                let _ = host.emit("library_updated", ());
            }
        });
    }
    {
        let mut rx = state.subscribe_anki_sync_status();
        let host = host.clone();
        host::async_runtime::spawn(async move {
            while rx.changed().await.is_ok() {
                let _ = host.emit("anki_sync_status_changed", ());
            }
        });
    }
    {
        let mut rx = state.subscribe_sync_status();
        let host = host.clone();
        host::async_runtime::spawn(async move {
            while rx.changed().await.is_ok() {
                let _ = host.emit("sync_status_changed", ());
            }
        });
    }

    info!("Spawning async init");
    let app_state = state.clone();
    host::async_runtime::spawn(async move {
        // Publish first: commands gate on it, and the steps below must not be
        // able to withhold it.
        let outcome = app_state.eval_config().await.map_err(|err| {
            warn!("Failed to evaluate config at startup: {err}");
            err.to_string()
        });
        app_state.publish_ready(outcome);

        // A token must be in hand when start_spotify_watcher begins polling.
        let client_id = app_state.config_borrow_client_id();
        app_state.spotify_web.try_resume(client_id).await;

        while let Some(event) = recv.recv().await {
            app_state
                .handle_file_change_event(&event)
                .await
                .unwrap_or_else(|err| warn!("Failed to process event {event:?}: {err}"));
        }
        warn!("LibraryWatcher sender disconnected; file change loop exiting");
    });

    Ok(Core { state, owner })
}

/// Raises the open-file soft limit and returns a line for the caller to log once
/// the logger exists; this must run before it does.
///
/// iOS gives the whole process 256 descriptors, shared by WebKit, the embedded
/// Syncthing engine, and library I/O, and exhaustion surfaces as EMFILE far from
/// the culprit. The Go runtime raises it too, but from a c-archive constructor
/// that races `main`.
///
/// Targets descend because Darwin refuses a soft limit above `OPEN_MAX` /
/// `kern.maxfilesperproc` even under an unlimited hard limit; if none land, the
/// inherited limit stands.
#[cfg(unix)]
pub fn raise_open_file_limit() -> String {
    const TARGETS: [libc::rlim_t; 3] = [10_240, 4_096, 1_024];

    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 {
        return format!(
            "open-file limit: getrlimit failed ({})",
            std::io::Error::last_os_error()
        );
    }

    let ceiling = if lim.rlim_max == libc::RLIM_INFINITY {
        libc::rlim_t::MAX
    } else {
        lim.rlim_max
    };

    for target in TARGETS {
        if target <= lim.rlim_cur || target > ceiling {
            continue;
        }
        let want = libc::rlimit {
            rlim_cur: target,
            rlim_max: lim.rlim_max,
        };
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &want) } == 0 {
            return format!("open-file limit raised {} -> {target}", lim.rlim_cur);
        }
    }

    format!("open-file limit left at {}", lim.rlim_cur)
}

#[cfg(not(unix))]
pub fn raise_open_file_limit() -> String {
    "open-file limit: not adjusted (non-unix)".to_string()
}
