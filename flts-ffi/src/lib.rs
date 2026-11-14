//! The FLTS core for native shells: one command table over JSON, events pushed
//! back through the shell's callback.

use std::sync::{Arc, Mutex};

use app_core::{AppHandle, AppState, Host};
use serde_json::Value;

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("{message}")]
    Failed { message: String },
}

impl CoreError {
    fn from_value(value: Value) -> Self {
        let message = match value {
            Value::String(s) => s,
            other => other.to_string(),
        };
        CoreError::Failed { message }
    }
}

/// Implemented by the native app.
#[uniffi::export(with_foreign)]
pub trait Shell: Send + Sync {
    /// Same names and JSON payloads the Tauri frontend receives.
    fn on_event(&self, name: String, payload: String);
    /// Must call `task.run()` on the UI thread.
    fn run_on_main(&self, task: Arc<MainTask>);
}

/// Work the core needs done on the UI thread (AppKit/UIKit-only APIs).
#[derive(uniffi::Object)]
pub struct MainTask(Mutex<Option<Box<dyn FnOnce() + Send>>>);

#[uniffi::export]
impl MainTask {
    pub fn run(&self) {
        let task = self.0.lock().ok().and_then(|mut slot| slot.take());
        if let Some(task) = task {
            task();
        }
    }
}

struct ShellHost(Arc<dyn Shell>);

impl Host for ShellHost {
    fn emit(&self, event: &str, payload: Value) {
        self.0.on_event(event.to_string(), payload.to_string());
    }

    fn run_on_main_thread(&self, f: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        self.0.run_on_main(Arc::new(MainTask(Mutex::new(Some(f)))));
        Ok(())
    }
}

#[derive(uniffi::Object)]
pub struct FltsCore {
    state: AppState,
    owner: Mutex<Option<library::task::TaskOwner>>,
}

#[uniffi::export]
impl FltsCore {
    /// Paths come from the same resolution as the Tauri app (`FLTS_CONFIG_DIR`
    /// and friends included), so both shells open the same library.
    #[uniffi::constructor]
    pub fn start(shell: Arc<dyn Shell>) -> Result<Arc<Self>, CoreError> {
        let fd_limit = app_core::raise_open_file_limit();
        init_logging();
        log::info!("{fd_limit}");
        let core =
            app_core::start(AppHandle::new(ShellHost(shell))).map_err(|err| CoreError::Failed {
                message: format!("{err:#}"),
            })?;
        Ok(Arc::new(Self {
            state: core.state,
            owner: Mutex::new(Some(core.owner)),
        }))
    }

    /// `args` is a JSON object with camelCase keys; the result is JSON.
    pub async fn invoke(&self, cmd: String, args: String) -> Result<String, CoreError> {
        let args: Value = if args.is_empty() {
            Value::Object(Default::default())
        } else {
            serde_json::from_str(&args).map_err(|err| CoreError::Failed {
                message: format!("bad args JSON: {err}"),
            })?
        };
        let state = self.state.clone();
        let result = app_core::host::async_runtime::spawn(async move {
            app_core::dispatch::dispatch(&state, &cmd, args).await
        })
        .await
        .map_err(|err| CoreError::Failed {
            message: format!("command task failed: {err}"),
        })?;
        result
            .map(|value| value.to_string())
            .map_err(CoreError::from_value)
    }

    /// Flushes and stops the backend; call once before the process exits.
    pub async fn shutdown(&self) {
        let state = self.state.clone();
        let owner = self.owner.lock().ok().and_then(|mut o| o.take());
        let _ = app_core::host::async_runtime::spawn(async move {
            state.shutdown().await;
            drop(owner);
        })
        .await;
    }
}

fn init_logging() {
    #[cfg(target_vendor = "apple")]
    {
        let _ = oslog::OsLogger::new("com.TS.FLTS")
            .level_filter(log::LevelFilter::Info)
            .init();
    }
}
