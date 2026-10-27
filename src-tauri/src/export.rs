//! The window's export commands: running one, following it, and cancelling it.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use relight_ffmpeg::Job;
use relight_media::export::{self, Exported, Preset};
use relight_media::timeline::{Picture, Segment};
use tauri::{AppHandle, Emitter, Manager, State};

/// The running export's cancel flag, if one is running.
#[derive(Default)]
pub struct Exports(Mutex<Option<Arc<AtomicBool>>>);

/// Exports the timeline to `output`, reporting on the "export" event as it goes.
#[tauri::command]
pub async fn export(
    app: AppHandle,
    segments: Vec<Segment>,
    picture: Picture,
    output: String,
    preset: Preset,
) -> Result<Exported, String> {
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let exports = app.state::<Exports>();
        let mut running = exports.0.lock().unwrap();
        if running.is_some() {
            return Err("An export is already running.".into());
        }
        *running = Some(cancelled.clone());
    }

    let events = app.clone();
    let job = Job::new(cancelled, move |report| {
        let _ = events.emit("export", report);
    });
    let result = tauri::async_runtime::spawn_blocking(move || {
        export::run(&job, &segments, &picture, Path::new(&output), &preset)
    })
    .await;

    *app.state::<Exports>().0.lock().unwrap() = None;
    result.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn cancel_export(exports: State<Exports>) {
    if let Some(cancelled) = &*exports.0.lock().unwrap() {
        cancelled.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub fn is_exporting(exports: State<Exports>) -> bool {
    exports.0.lock().unwrap().is_some()
}
