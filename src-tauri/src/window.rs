//! The window: what it's told when it starts, bringing it back, and quitting.

use std::path::Path;
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::recording;

/// The clip or project named on the command line (`relight clip.mp4`), if any.
pub struct InitialFile(Mutex<Option<String>>);

impl InitialFile {
    pub fn from_command_line() -> Self {
        let path = std::env::args_os()
            .nth(1)
            .and_then(|arg| std::path::absolute(arg).ok())
            .and_then(|path| path.into_os_string().into_string().ok());
        Self(Mutex::new(path))
    }
}

/// The command line's file, given out once, so the window reloading doesn't open it again.
#[tauri::command]
pub fn initial_file(state: State<InitialFile>) -> Option<String> {
    state.0.lock().unwrap().take()
}

/// Something that went wrong starting up (the hotkey being taken), told to the window once.
pub struct Notice(Mutex<Option<String>>);

impl Notice {
    pub fn new(notice: Option<String>) -> Self {
        Self(Mutex::new(notice))
    }
}

#[tauri::command]
pub fn take_notice(state: State<Notice>) -> Option<String> {
    state.0.lock().unwrap().take()
}

/// Whether this copy updates itself. Only the release workflow's builds do: another copy
/// (`npm run install-cli`, a development build) would install a second relight beside itself.
#[tauri::command]
pub fn updates_on() -> bool {
    option_env!("RELIGHT_UPDATES").is_some()
}

/// Brings the window back from the tray, the taskbar or behind other windows.
pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Quits, once any recording has been saved. Off the main thread, so the window keeps
/// drawing while it waits.
#[tauri::command]
pub async fn quit(app: AppHandle) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        recording::stop_and_wait(&app);
        app.exit(0);
    })
    .await;
}

/// `relight clip.mp4` typed with relight already running: the running one shows itself and
/// opens it, rather than a second copy starting and fighting it for the hotkey.
pub fn another_instance(app: &AppHandle, args: Vec<String>, cwd: String) {
    show_window(app);
    if let Some(file) = args.get(1) {
        let path = Path::new(&cwd).join(file);
        let _ = app.emit("open-file", path.to_string_lossy());
    }
}
