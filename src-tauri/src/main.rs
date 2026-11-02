// RoomLab desktop entry point. Hides the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use std::sync::Arc;
use std::time::Duration;

use roomlab_core::settings::DataPaths;
use roomlab_core::Engine;
use tauri::{Emitter, Manager};

pub struct AppState {
    pub engine: Arc<Engine>,
    pub wake: Arc<tokio::sync::Notify>,
    pub last_health: std::sync::Mutex<Option<roomlab_core::contracts::ProviderHealth>>,
}

#[cfg(windows)]
fn fatal_box(msg: &str) {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: *mut core::ffi::c_void, text: *const u16, caption: *const u16, utype: u32) -> i32;
    }
    let w = |s: &str| std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let (t, c) = (w(msg), w("RoomLab"));
    unsafe {
        MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), 0x10);
    }
}
#[cfg(not(windows))]
fn fatal_box(msg: &str) {
    eprintln!("{msg}");
}

fn main() {
    // T63: clear failure route when the WebView2 runtime is missing.
    if let Err(e) = tauri::webview_version() {
        fatal_box(&format!(
            "RoomLab needs the Microsoft Edge WebView2 Runtime, which was not found ({e}).\n\nInstall the Evergreen WebView2 Runtime from Microsoft (https://developer.microsoft.com/microsoft-edge/webview2/) and start RoomLab again."
        ));
        std::process::exit(2);
    }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| std::path::PathBuf::from("."));
    let override_dir = std::env::var_os("ROOMLAB_DATA_DIR").map(std::path::PathBuf::from);
    let paths = match DataPaths::resolve(&exe_dir, override_dir) {
        Ok(p) => p,
        Err(e) => {
            fatal_box(&format!("RoomLab could not create a writable data folder: {e}\n\nTry moving the RoomLab folder somewhere you can write to (e.g. Documents)."));
            std::process::exit(3);
        }
    };
    let engine = match Engine::open(paths.clone()) {
        Ok(e) => Arc::new(e),
        Err(e) => {
            fatal_box(&format!(
                "RoomLab could not open its database at {}:\n{e}\n\nIf the file is damaged, move it aside (a backup is kept next to it during upgrades) and start again.",
                paths.db_file.display()
            ));
            std::process::exit(4);
        }
    };
    roomlab_core::redact::log_line(&paths.logs, &[], &format!("RoomLab {} starting; data={}", roomlab_core::contracts::APP_VERSION, paths.data.display()));
    let state = AppState { engine: engine.clone(), wake: Arc::new(tokio::sync::Notify::new()), last_health: Default::default() };
    let wake = state.wake.clone();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .setup(move |app| {
            let handle = app.handle().clone();
            let engine = engine.clone();
            let wake = wake.clone();
            // Simulation runner: off the UI thread; one generation at a time (concurrency 1).
            tauri::async_runtime::spawn(async move {
                loop {
                    let run = engine.settings().active_run_id.clone();
                    let Some(run) = run else {
                        let _ = tokio::time::timeout(Duration::from_millis(500), wake.notified()).await;
                        continue;
                    };
                    let h2 = handle.clone();
                    let mut sink = move |d: roomlab_core::runner::DraftUpdate| {
                        let _ = h2.emit("roomlab://draft", d);
                    };
                    match engine.process_next(&run, &mut sink).await {
                        Ok(Some(report)) => {
                            let _ = handle.emit("roomlab://changed", serde_json::json!({"run_id": run, "reason": "job", "report": report}));
                            if report.model_id.is_none() {
                                // Pace scripted turns so humans can follow the chat.
                                let d = engine.settings().demo_turn_delay_ms.min(5000);
                                tokio::time::sleep(Duration::from_millis(d)).await;
                            }
                        }
                        Ok(None) => {
                            let _ = tokio::time::timeout(Duration::from_millis(400), wake.notified()).await;
                        }
                        Err(e) => {
                            roomlab_core::redact::log_line(&engine.paths.logs, &engine.secret_values(), &format!("runner error: {e}"));
                            let _ = handle.emit("roomlab://error", e.to_string());
                            tokio::time::sleep(Duration::from_millis(1000)).await;
                        }
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(commands::handler())
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            fatal_box(&format!("RoomLab failed to start its window: {e}"));
            std::process::exit(5);
        });

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            // Kill only the llama-server this app started (if any). External servers are untouched.
            let st = handle.state::<AppState>();
            let engine = st.engine.clone();
            tauri::async_runtime::block_on(async move {
                if let Some(mut m) = engine.managed.lock().await.take() {
                    m.stop().await;
                }
            });
            roomlab_core::redact::log_line(&st.engine.paths.logs, &[], "RoomLab exited cleanly");
        }
    });
}
