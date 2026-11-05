//! The recorder as the app runs it: recording and instant replay, started and stopped by
//! the hotkeys, the tray or the window, with what they're doing shown in all three.
//!
//! Only one captures at a time: starting a recording ends the replay and what it held, and
//! a fresh one starts when the recording's done.

use std::collections::HashSet;
use std::fs;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use relight_recorder::{Options, Recorded, Recording, Replay, Sources};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::settings::{Settings, SettingsState};
use crate::{chime, tray, window};

#[derive(Default)]
pub struct Recorder {
    /// Held while a press is acted on, so presses close together take turns.
    turn: Mutex<()>,
    phase: Mutex<Phase>,
    /// Recordings stopped and still being saved.
    saving: AtomicUsize,
    /// Where the recordings being made or saved go, so a new one doesn't take the name.
    outputs: Mutex<HashSet<PathBuf>>,
    replay: Mutex<ReplayPhase>,
    /// Replays being saved.
    replays_saving: AtomicUsize,
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Starting,
    Recording(Recording, PathBuf),
}

#[derive(Default)]
enum ReplayPhase {
    #[default]
    Off,
    Starting,
    On(Replay),
    /// Waiting for a recording to finish, to start afresh.
    Paused,
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

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum ReplayStatus {
    Off,
    Starting,
    On {
        /// A replay is being saved.
        saving: bool,
        /// The screen capture has ended and is waiting to start again.
        interrupted: bool,
    },
    Paused,
}

fn status(recorder: &Recorder) -> Status {
    match &*recorder.phase.lock().unwrap() {
        Phase::Recording(recording, _) => Status::Recording { since: millis(recording.started()) },
        Phase::Starting => Status::Starting,
        Phase::Idle if recorder.saving.load(Ordering::Relaxed) > 0 => Status::Saving,
        Phase::Idle => Status::Idle,
    }
}

fn status_of_replay(recorder: &Recorder) -> ReplayStatus {
    match &*recorder.replay.lock().unwrap() {
        ReplayPhase::Off => ReplayStatus::Off,
        ReplayPhase::Starting => ReplayStatus::Starting,
        ReplayPhase::Paused => ReplayStatus::Paused,
        ReplayPhase::On(replay) => ReplayStatus::On {
            saving: recorder.replays_saving.load(Ordering::Relaxed) > 0,
            interrupted: replay.is_interrupted(),
        },
    }
}

fn millis(time: SystemTime) -> f64 {
    time.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64() * 1000.0
}

fn announce(app: &AppHandle) {
    let recorder = app.state::<Recorder>();
    let status = status(&recorder);
    let replay = status_of_replay(&recorder);
    tray::show_status(app, &status, &replay);
    let _ = app.emit("recorder", status);
    let _ = app.emit("replay", replay);
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
            resume_replay(&app);
            return;
        }
        *recorder.phase.lock().unwrap() = Phase::Starting;
        pause_replay(&recorder);
        announce(&app);
        let started = start(&app, &recorder);
        let failed = started.is_err();
        *recorder.phase.lock().unwrap() = match started {
            Ok((recording, output)) => Phase::Recording(recording, output),
            Err(e) => {
                let _ = app.emit("recorder-error", e);
                Phase::Idle
            }
        };
        if failed && matches!(*recorder.replay.lock().unwrap(), ReplayPhase::Paused) {
            begin_replay(&app, &recorder);
        }
        drop(turn);
        announce(&app);
    });
}

/// Stops any recording and replay, and waits for everything stopped to be saved: for quitting.
pub fn stop_and_wait(app: &AppHandle) {
    let recorder = app.state::<Recorder>();
    let _turn = recorder.turn.lock().unwrap();
    end_replay(&recorder);
    let phase = mem::take(&mut *recorder.phase.lock().unwrap());
    if let Phase::Recording(recording, output) = phase {
        finish(app, recording, output);
    }
    while recorder.saving.load(Ordering::Relaxed) > 0 || recorder.replays_saving.load(Ordering::Relaxed) > 0 {
        thread::sleep(Duration::from_millis(50));
    }
}

fn folder_for(app: &AppHandle, settings: &Settings) -> Result<PathBuf, String> {
    let folder = match &settings.folder {
        Some(folder) => PathBuf::from(folder),
        None => app.path().video_dir().map_err(|e| e.to_string())?.join("relight"),
    };
    fs::create_dir_all(&folder).map_err(|e| format!("Couldn't make {}: {e}", folder.display()))?;
    Ok(folder)
}

fn start(app: &AppHandle, recorder: &Recorder) -> Result<(Recording, PathBuf), String> {
    let settings = app.state::<SettingsState>().get();
    let folder = folder_for(app, &settings)?;
    let output = reserve(recorder, &folder, "relight");
    match Recording::start(options(&settings), output.clone()) {
        Ok(recording) => Ok((recording, output)),
        Err(e) => {
            recorder.outputs.lock().unwrap().remove(&output);
            Err(e)
        }
    }
}

/// A name for a new file in `folder`, from now: "relight 2026-10-27 21.05.33.mp4", with a
/// number on the end if another has that already.
fn reserve(recorder: &Recorder, folder: &Path, name: &str) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y-%m-%d %H.%M.%S");
    let mut taken = recorder.outputs.lock().unwrap();
    let mut output = folder.join(format!("{name} {stamp}.mp4"));
    let mut n = 2;
    while output.exists() || taken.contains(&output) {
        output = folder.join(format!("{name} {stamp} ({n}).mp4"));
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

/// Starts instant replay, or if a recording's going, has it start when that's done. Off
/// the calling thread, like `toggle`.
pub fn start_replay(app: &AppHandle) {
    let app = app.clone();
    thread::spawn(move || {
        let recorder = app.state::<Recorder>();
        let _turn = recorder.turn.lock().unwrap();
        if !matches!(*recorder.replay.lock().unwrap(), ReplayPhase::Off) {
            return;
        }
        if matches!(*recorder.phase.lock().unwrap(), Phase::Idle) {
            begin_replay(&app, &recorder);
        } else {
            *recorder.replay.lock().unwrap() = ReplayPhase::Paused;
            announce(&app);
        }
    });
}

/// Stops instant replay, and lets go of what it held.
pub fn stop_replay(app: &AppHandle) {
    let app = app.clone();
    thread::spawn(move || {
        let recorder = app.state::<Recorder>();
        let _turn = recorder.turn.lock().unwrap();
        end_replay(&recorder);
        announce(&app);
    });
}

/// Starts instant replay over if it's on, for new settings to take effect.
pub fn restart_replay(app: &AppHandle) {
    let app = app.clone();
    thread::spawn(move || {
        let recorder = app.state::<Recorder>();
        let _turn = recorder.turn.lock().unwrap();
        if matches!(*recorder.replay.lock().unwrap(), ReplayPhase::On(_)) {
            end_replay(&recorder);
            begin_replay(&app, &recorder);
        }
    });
}

/// Starts instant replay if it's off, and stops it otherwise.
pub fn toggle_replay(app: &AppHandle) {
    if matches!(*app.state::<Recorder>().replay.lock().unwrap(), ReplayPhase::Off) {
        start_replay(app);
    } else {
        stop_replay(app);
    }
}

/// Saves what instant replay holds, up to right now. From a game, the window can't be
/// brought up: this says how it went with a sound, and leaves the rest for later.
pub fn save_replay(app: &AppHandle) {
    let pressed = SystemTime::now();
    let recorder = app.state::<Recorder>();
    let saver = match &*recorder.replay.lock().unwrap() {
        ReplayPhase::On(replay) => Ok(replay.saver()),
        ReplayPhase::Starting => Err("Instant replay is still starting, so there's nothing to save yet."),
        ReplayPhase::Paused => Err("Instant replay is off while recording, so there's nothing to save."),
        ReplayPhase::Off => Err("Instant replay isn't on, so there's nothing to save."),
    };
    let saver = match saver {
        Ok(saver) => saver,
        Err(why) => {
            chime::failed();
            let _ = app.emit("recorder-error", why);
            return;
        }
    };
    chime::saved();
    recorder.replays_saving.fetch_add(1, Ordering::Relaxed);
    announce(app);
    let app = app.clone();
    thread::spawn(move || {
        let recorder = app.state::<Recorder>();
        let settings = app.state::<SettingsState>().get();
        let result: Result<Recorded, String> = folder_for(&app, &settings).and_then(|folder| {
            let output = reserve(&recorder, &folder, "relight replay");
            let saved = saver.save(pressed, &output);
            recorder.outputs.lock().unwrap().remove(&output);
            saved
        });
        recorder.replays_saving.fetch_sub(1, Ordering::Relaxed);
        announce(&app);
        let _ = match result {
            Ok(recorded) => app.emit("replay-saved", recorded),
            Err(e) => {
                chime::failed();
                app.emit("recorder-error", e)
            }
        };
    });
}

/// Starts instant replay with the settings as they are now. With the turn held.
fn begin_replay(app: &AppHandle, recorder: &Recorder) {
    *recorder.replay.lock().unwrap() = ReplayPhase::Starting;
    announce(app);
    let settings = app.state::<SettingsState>().get();
    let options = Options { microphone: settings.replay_microphone, ..options(&settings) };
    let changes = app.clone();
    let started = Replay::start(options, settings.replay_seconds, move || announce(&changes));
    *recorder.replay.lock().unwrap() = match started {
        Ok((replay, warnings)) => {
            if !warnings.is_empty() {
                let _ = app.emit("recorder-error", warnings.join("\n"));
            }
            ReplayPhase::On(replay)
        }
        Err(e) => {
            let _ = app.emit("recorder-error", e);
            ReplayPhase::Off
        }
    };
    announce(app);
}

/// Stops instant replay for good. With the turn held. It's taken out of the lock first:
/// stopping waits for its thread, which may be saying something's changed.
fn end_replay(recorder: &Recorder) {
    let phase = mem::take(&mut *recorder.replay.lock().unwrap());
    if let ReplayPhase::On(replay) = phase {
        replay.stop();
    }
}

/// Stops a replay that's on until the recording about to start is done. With the turn held.
fn pause_replay(recorder: &Recorder) {
    let mut phase = recorder.replay.lock().unwrap();
    if matches!(*phase, ReplayPhase::On(_)) {
        let going = mem::replace(&mut *phase, ReplayPhase::Paused);
        drop(phase);
        if let ReplayPhase::On(replay) = going {
            replay.stop();
        }
    }
}

/// A recording's done: starts a fresh replay if one was paused for it, unless another
/// recording has begun since.
fn resume_replay(app: &AppHandle) {
    let recorder = app.state::<Recorder>();
    let _turn = recorder.turn.lock().unwrap();
    let idle = matches!(*recorder.phase.lock().unwrap(), Phase::Idle);
    if idle && matches!(*recorder.replay.lock().unwrap(), ReplayPhase::Paused) {
        begin_replay(app, &recorder);
    }
}

#[tauri::command]
pub fn toggle_recording(app: AppHandle) {
    toggle(&app);
}

#[tauri::command]
pub fn recorder_status(recorder: State<Recorder>) -> Status {
    status(&recorder)
}

#[tauri::command]
pub fn replay_start(app: AppHandle) {
    start_replay(&app);
}

#[tauri::command]
pub fn replay_stop(app: AppHandle) {
    stop_replay(&app);
}

#[tauri::command]
pub fn replay_save(app: AppHandle) {
    save_replay(&app);
}

#[tauri::command]
pub fn replay_status(recorder: State<Recorder>) -> ReplayStatus {
    status_of_replay(&recorder)
}

/// What there is to record, for the settings.
#[tauri::command]
pub async fn recording_sources() -> Result<Sources, String> {
    tauri::async_runtime::spawn_blocking(relight_recorder::sources).await.map_err(|e| e.to_string())
}
