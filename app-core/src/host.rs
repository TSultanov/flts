//! What the core needs from the shell it runs in (Tauri or the native app).

use std::{fmt, path::PathBuf, sync::Arc};

/// Implemented by each shell. Events carry the same names and JSON payloads
/// whichever shell delivers them.
pub trait Host: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: serde_json::Value);

    fn run_on_main_thread(&self, f: Box<dyn FnOnce() + Send>) -> Result<(), String>;

    /// Where the platform keeps app files, for platforms without HOME (Android).
    fn platform_dirs(&self) -> Option<PlatformDirs> {
        None
    }
}

#[derive(Clone, Debug)]
pub struct PlatformDirs {
    pub config: PathBuf,
    pub cache: PathBuf,
    pub data: PathBuf,
}

#[derive(Clone)]
pub struct AppHandle(Arc<dyn Host>);

#[derive(Debug)]
pub struct EmitError(serde_json::Error);

impl fmt::Display for EmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "event payload did not serialize: {}", self.0)
    }
}

impl std::error::Error for EmitError {}

impl AppHandle {
    pub fn new(host: impl Host) -> Self {
        Self(Arc::new(host))
    }

    pub fn emit<S: serde::Serialize>(&self, event: &str, payload: S) -> Result<(), EmitError> {
        let value = serde_json::to_value(payload).map_err(EmitError)?;
        self.0.emit(event, value);
        Ok(())
    }

    pub fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<(), String> {
        self.0.run_on_main_thread(Box::new(f))
    }

    pub fn platform_dirs(&self) -> anyhow::Result<PlatformDirs> {
        self.0
            .platform_dirs()
            .ok_or_else(|| anyhow::anyhow!("host provides no platform dirs"))
    }
}

/// Stand-in for `tauri::State`: a borrowed handle to managed state.
pub struct State<'a, T>(pub &'a T);

impl<'a, T> State<'a, T> {
    pub fn inner(&self) -> &'a T {
        self.0
    }
}

impl<T> std::ops::Deref for State<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0
    }
}

impl<T> Clone for State<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for State<'_, T> {}

/// Sync-roster default name source; the shell may not know a better one.
pub fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

/// One process-wide tokio runtime. Shells that bring their own (Tauri) are
/// handed this one instead, so tasks never straddle two runtimes.
pub mod async_runtime {
    use std::{future::Future, sync::OnceLock};

    pub use tokio::task::JoinHandle;

    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

    pub struct RuntimeHandle(&'static tokio::runtime::Runtime);

    impl RuntimeHandle {
        pub fn inner(&self) -> &tokio::runtime::Handle {
            self.0.handle()
        }
    }

    pub fn handle() -> RuntimeHandle {
        RuntimeHandle(RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("flts-core")
                .build()
                .expect("failed to build the tokio runtime")
        }))
    }

    pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        handle().inner().spawn(future)
    }

    pub fn block_on<F: Future>(future: F) -> F::Output {
        handle().inner().block_on(future)
    }
}
