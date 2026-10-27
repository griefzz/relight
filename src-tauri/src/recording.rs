//! The recorder as the app runs it: started and stopped by the hotkey, the tray or the
//! window, with what it's doing shown in all three.

use std::collections::HashSet;
use std::fs;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use relight_recorder::{Options, Recording, Sources};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::settings::{Settings, SettingsState};
use crate::{tray, window};

#[derive(Default)]
pub struct Recorder {
    /// Held while a press is acted on, so presses close together take turns.
    turn: Mutex<()>,
    phase: Mutex<Phase>,
    /// Recordings stopped and still being saved.
    saving: AtomicUsize,
    /// Where the recordings being made or saved go, so a new one doesn't take the name.
    outputs: Mutex<HashSet<PathBuf>>,
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Starting,
    Recording(Recording, PathBuf),
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    Idle,
    Starting,
    /// Since when, in milliseconds since 1970.
    Recording { since: f64 },
    Saving,
}

fn status(recorder: &Recorder) -> Status {
    match &*recorder.phase.lock().unwrap() {
        Phase::Recording(recording, _) => Status::Recording { since: millis(recording.started()) },
        Phase::Starting => Status::Starting,
        Phase::Idle if recorder.saving.load(Ordering::Relaxed) > 0 => Status::Saving,
        Phase::Idle => Status::Idle,
    }
}

fn millis(time: SystemTime) -> f64 {
    time.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64() * 1000.0
}

fn announce(app: &AppHandle) {
    let status = status(&app.state::<Recorder>());
    tray::show_status(app, &status);
    let _ = app.emit("recorder", status);
}

/// Starts recording, or stops the recording going. Off the calling thread: starting takes
/// a moment, and the hotkey arrives on the window's.
pub fn toggle(app: &AppHandle) {
    let app = app.clone();
    thread::spawn(move || {
        let recorder = app.state::<Recorder>();
        let turn = recorder.turn.lock().unwrap();
        // Its own statement, so the lock is let go before `finish` needs it again.
        let phase = mem::take(&mut *recorder.phase.lock().unwrap());
        if let Phase::Recording(recording, output) = phase {
            drop(turn);
            finish(&app, recording, output);
            return;
        }
        *recorder.phase.lock().unwrap() = Phase::Starting;
        announce(&app);
        let started = start(&app, &recorder);
        *recorder.phase.lock().unwrap() = match started {
            Ok((recording, output)) => Phase::Recording(recording, output),
            Err(e) => {
                let _ = app.emit("recorder-error", e);
                Phase::Idle
            }
        };
        drop(turn);
        announce(&app);
    });
}

/// Stops any recording, and waits for every recording stopped to be saved: for quitting.
pub fn stop_and_wait(app: &AppHandle) {
    let recorder = app.state::<Recorder>();
    let _turn = recorder.turn.lock().unwrap();
    let phase = mem::take(&mut *recorder.phase.lock().unwrap());
    if let Phase::Recording(recording, output) = phase {
        finish(app, recording, output);
    }
    while recorder.saving.load(Ordering::Relaxed) > 0 {
        thread::sleep(Duration::from_millis(50));
    }
}

fn start(app: &AppHandle, recorder: &Recorder) -> Result<(Recording, PathBuf), String> {
    let settings = app.state::<SettingsState>().get();
    let folder = match &settings.folder {
        Some(folder) => PathBuf::from(folder),
        None => app.path().video_dir().map_err(|e| e.to_string())?.join("relight"),
    };
    fs::create_dir_all(&folder).map_err(|e| format!("Couldn't make {}: {e}", folder.display()))?;
    let output = reserve(recorder, &folder);
    match Recording::start(options(&settings), output.clone()) {
        Ok(recording) => Ok((recording, output)),
        Err(e) => {
            recorder.outputs.lock().unwrap().remove(&output);
            Err(e)
        }
    }
}

/// A name for a new recording in `folder`, from when it starts: "relight 2026-10-27 21.05.33.mp4",
/// with a number on the end if another recording has that already.
fn reserve(recorder: &Recorder, folder: &Path) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y-%m-%d %H.%M.%S");
    let mut taken = recorder.outputs.lock().unwrap();
    let mut output = folder.join(format!("relight {stamp}.mp4"));
    let mut n = 2;
    while output.exists() || taken.contains(&output) {
        output = folder.join(format!("relight {stamp} ({n}).mp4"));
        n += 1;
    }
    taken.insert(output.clone());
    output
}

fn options(settings: &Settings) -> Options {
    Options {
        screen: settings.screen,
        fps: settings.fps,
        game_sound: settings.game_sound,
        game_device: settings.game_device.clone(),
        microphone: settings.microphone,
        mic_device: settings.mic_device.clone(),
    }
}

fn finish(app: &AppHandle, recording: Recording, output: PathBuf) {
    let recorder = app.state::<Recorder>();
    recorder.saving.fetch_add(1, Ordering::Relaxed);
    announce(app);
    let result = recording.stop();
    recorder.outputs.lock().unwrap().remove(&output);
    recorder.saving.fetch_sub(1, Ordering::Relaxed);
    announce(app);
    window::show_window(app);
    let _ = match result {
        Ok(recorded) => app.emit("recorded", recorded),
        Err(e) => app.emit("recorder-error", e),
    };
}

#[tauri::command]
pub fn toggle_recording(app: AppHandle) {
    toggle(&app);
}

#[tauri::command]
pub fn recorder_status(recorder: State<Recorder>) -> Status {
    status(&recorder)
}

/// What there is to record, for the settings.
#[tauri::command]
pub async fn recording_sources() -> Result<Sources, String> {
    tauri::async_runtime::spawn_blocking(relight_recorder::sources).await.map_err(|e| e.to_string())
}
