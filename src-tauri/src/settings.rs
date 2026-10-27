//! The user's settings: the recording hotkey and what gets recorded, where recordings go,
//! and whether closing the window keeps relight running. Saved as JSON in the app's
//! config folder.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use relight_ffmpeg::Partial;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Starts and stops recording, from anywhere: "Ctrl+5", "Alt+Shift+R", "F9".
    pub hotkey: String,
    /// The screen to record, by its DXGI output number; none for the one the mouse is on.
    pub screen: Option<u32>,
    pub fps: u32,
    pub game_sound: bool,
    /// The output device whose sound is recorded; none for the default one.
    pub game_device: Option<String>,
    pub microphone: bool,
    /// The microphone recorded; none for the default one.
    pub mic_device: Option<String>,
    /// Where recordings are saved; none for a relight folder in Videos.
    pub folder: Option<String>,
    /// Closing the window hides it to the tray, so the hotkey keeps working.
    pub keep_running: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "Ctrl+5".into(),
            screen: None,
            fps: 60,
            game_sound: true,
            game_device: None,
            microphone: true,
            mic_device: None,
            folder: None,
            keep_running: true,
        }
    }
}

impl Settings {
    /// Settings as they can be used, whatever the file said.
    fn usable(mut self) -> Self {
        self.fps = self.fps.clamp(1, 240);
        self
    }
}

pub struct SettingsState(Mutex<Settings>);

impl SettingsState {
    pub fn get(&self) -> Settings {
        self.0.lock().unwrap().clone()
    }
}

fn file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_config_dir().map_err(|e| e.to_string())?.join("settings.json"))
}

/// The saved settings (the defaults if there are none yet), with their hotkey taken. A
/// hotkey another program has is reported rather than failing the start.
pub fn load(app: &AppHandle) -> (SettingsState, Option<String>) {
    let settings: Settings = file(app)
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Settings>(&text).ok())
        .unwrap_or_default()
        .usable();
    let problem = take_hotkey(app, &settings.hotkey).err();
    (SettingsState(Mutex::new(settings)), problem)
}

fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = file(app)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    // Written whole or not at all: half a file would read as no settings.
    let partial = Partial::new(&path);
    fs::write(partial.path(), json).map_err(|e| format!("Couldn't save the settings: {e}"))?;
    partial.finish()
}

fn shortcut(hotkey: &str) -> Result<Shortcut, String> {
    hotkey.parse().map_err(|_| format!("\"{hotkey}\" isn't a key combination relight can use."))
}

fn take_hotkey(app: &AppHandle, hotkey: &str) -> Result<(), String> {
    let shortcut = shortcut(hotkey)?;
    if app.global_shortcut().is_registered(shortcut) {
        return Ok(());
    }
    // Windows only says no: the usual reason is another program having it already.
    app.global_shortcut().register(shortcut).map_err(|_| {
        format!("Couldn't use {hotkey} as the recording hotkey: another program already has it. Pick another in Settings.")
    })
}

#[tauri::command]
pub fn get_settings(state: State<SettingsState>) -> Settings {
    state.get()
}

/// Saves `next`, moving the hotkey if it changed; if the new one can't be had, nothing
/// changes and the old one stays. An unchanged hotkey that another program had is tried
/// again, as that program may have let go of it.
#[tauri::command]
pub fn set_settings(app: AppHandle, state: State<SettingsState>, next: Settings) -> Result<Settings, String> {
    let next = next.usable();
    let mut current = state.0.lock().unwrap();
    if next.hotkey != current.hotkey {
        let _ = app.global_shortcut().unregister_all();
        if let Err(e) = take_hotkey(&app, &next.hotkey) {
            let _ = take_hotkey(&app, &current.hotkey);
            return Err(e);
        }
    } else {
        let _ = take_hotkey(&app, &next.hotkey);
    }
    save(&app, &next)?;
    *current = next.clone();
    Ok(next)
}

/// Lets go of the hotkey while a new one is being typed in, so pressing it there doesn't
/// start a recording; then takes it back.
#[tauri::command]
pub fn pause_hotkey(app: AppHandle, state: State<SettingsState>, paused: bool) -> Result<(), String> {
    if paused {
        app.global_shortcut().unregister_all().map_err(|e| e.to_string())
    } else {
        take_hotkey(&app, &state.get().hotkey)
    }
}

/// Whether the hotkey is relight's: not if another program had it first.
#[tauri::command]
pub fn hotkey_working(app: AppHandle, state: State<SettingsState>) -> bool {
    shortcut(&state.get().hotkey).is_ok_and(|shortcut| app.global_shortcut().is_registered(shortcut))
}
