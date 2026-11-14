use log::info;
use serde_json::Value;
use tauri::{Builder, Emitter, Manager, RunEvent};

#[cfg(feature = "e2e-bridge")]
pub mod bridge;

/// Delivers core events and main-thread work through the Tauri app.
struct TauriHost(tauri::AppHandle);

impl app_core::Host for TauriHost {
    fn emit(&self, event: &str, payload: Value) {
        if let Err(err) = self.0.emit(event, payload) {
            log::warn!("Failed to emit {event}: {err}");
        }
    }

    fn run_on_main_thread(&self, f: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        self.0.run_on_main_thread(f).map_err(|err| err.to_string())
    }

    #[cfg(target_os = "android")]
    fn platform_dirs(&self) -> Option<app_core::PlatformDirs> {
        let path = self.0.path();
        Some(app_core::PlatformDirs {
            config: path.app_config_dir().ok()?,
            cache: path.app_cache_dir().ok()?,
            data: path.app_data_dir().ok()?,
        })
    }
}

/// Every frontend command goes through the core's command table.
fn invoke(invoke: tauri::ipc::Invoke) -> bool {
    let cmd = invoke.message.command().to_string();
    let args = match invoke.message.payload() {
        tauri::ipc::InvokeBody::Json(value) => value.clone(),
        tauri::ipc::InvokeBody::Raw(_) => Value::Object(Default::default()),
    };
    let state = invoke
        .message
        .webview()
        .state::<app_core::AppState>()
        .inner()
        .clone();
    invoke.resolver.respond_async(async move {
        app_core::dispatch::dispatch(&state, &cmd, args)
            .await
            .map_err(tauri::ipc::InvokeError)
    });
    true
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Before anything else opens a file — including the webview.
    let fd_limit = app_core::raise_open_file_limit();
    // One runtime for the core and Tauri.
    tauri::async_runtime::set(app_core::host::async_runtime::handle().inner().clone());

    #[allow(unused_mut)]
    let mut context = tauri::generate_context!();
    // Headless: the WS bridge stands in for the webview's IPC channel.
    // Unless a harness wants the real webview, to time the frontend in it;
    // it then says where that frontend is served.
    #[cfg(feature = "e2e-bridge")]
    let e2e_window = std::env::var("FLTS_E2E_WINDOW_URL")
        .ok()
        .and_then(|url| url.parse().ok());
    #[cfg(feature = "e2e-bridge")]
    if std::env::var("FLTS_E2E_BRIDGE_PORT").is_ok() {
        match &e2e_window {
            Some(url) => context.config_mut().build.dev_url = Some(tauri::Url::clone(url)),
            None => context.config_mut().app.windows.clear(),
        }
    }
    // The saved window frame lives outside FLTS_CONFIG_DIR: a harness
    // window must neither read nor write the user's.
    #[cfg(feature = "e2e-bridge")]
    let keep_window_state = e2e_window.is_none();
    #[cfg(not(feature = "e2e-bridge"))]
    let keep_window_state = true;

    #[allow(unused_mut)]
    let mut builder = Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init());

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    if keep_window_state {
        builder = builder.plugin(tauri_plugin_window_state::Builder::new().build());
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let _ = keep_window_state;

    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    }

    builder
        .setup(move |app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .targets([
                            tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                            tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                                file_name: Some("logs".to_string()),
                            }),
                        ])
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            info!("{fd_limit}");

            let core = app_core::start(app_core::AppHandle::new(TauriHost(app.handle().clone())))?;
            app.manage(core.owner);
            app.manage(core.state);

            #[cfg(feature = "e2e-bridge")]
            if let Ok(port) = std::env::var("FLTS_E2E_BRIDGE_PORT") {
                crate::bridge::spawn(app.handle().clone(), port.parse()?);
            }

            Ok(())
        })
        .invoke_handler(invoke)
        .build(context)
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let RunEvent::Exit = event {
                let app_state = app_handle.state::<app_core::AppState>();
                let app_state = app_state.inner().clone();
                tauri::async_runtime::block_on(async move {
                    app_state.shutdown().await;
                });
                info!("Forcing process exit");
                app_handle.cleanup_before_exit();
                std::process::exit(0);
            }
        });
}
