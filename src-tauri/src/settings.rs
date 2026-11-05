//! The user's settings: the hotkeys and what gets recorded, how instant replay runs, where
//! recordings go, and whether closing the window keeps relight running. Saved as JSON in
//! the app's config folder.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use relight_ffmpeg::Partial;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use crate::recording;

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
    /// Starts instant replay, from anywhere.
    pub replay_start_hotkey: String,
    /// Stops instant replay, from anywhere.
    pub replay_stop_hotkey: String,
    /// Saves what instant replay holds, from anywhere.
    pub replay_save_hotkey: String,
    /// How much instant replay holds.
    pub replay_seconds: u32,
    /// Whether instant replay holds the microphone too. Separate from a recording's, as it's
    /// listening for as long as replay is on, not just while recording's asked for.
    pub replay_microphone: bool,
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
            replay_start_hotkey: "Alt+F6".into(),
            replay_stop_hotkey: "Alt+F7".into(),
            replay_save_hotkey: "Alt+F8".into(),
            replay_seconds: 30,
            replay_microphone: false,
            folder: None,
            keep_running: true,
        }
    }
}

impl Settings {
    /// Settings as they can be used, whatever the file said.
    fn usable(mut self) -> Self {
        self.fps = self.fps.clamp(1, 240);
        self.replay_seconds = self.replay_seconds.clamp(5, 600);
        self
    }

    /// Each hotkey and what it's for, as said in an error.
    fn hotkeys(&self) -> [(&str, &'static str); 4] {
        [
            (&self.hotkey, "the recording hotkey"),
            (&self.replay_start_hotkey, "the hotkey that starts instant replay"),
            (&self.replay_stop_hotkey, "the hotkey that stops instant replay"),
            (&self.replay_save_hotkey, "the hotkey that saves a replay"),
        ]
    }

    /// Whether going from this to `next` means starting instant replay over, if it's on.
    fn replay_differs(&self, next: &Self) -> bool {
        self.screen != next.screen
            || self.fps != next.fps
            || self.game_sound != next.game_sound
            || self.game_device != next.game_device
            || self.replay_microphone != next.replay_microphone
            || self.mic_device != next.mic_device
            || self.replay_seconds != next.replay_seconds
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

/// The saved settings (the defaults if there are none yet), with their hotkeys taken. A
/// hotkey another program has is reported rather than failing the start.
pub fn load(app: &AppHandle) -> (SettingsState, Option<String>) {
    let settings: Settings = file(app)
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Settings>(&text).ok())
        .unwrap_or_default()
        .usable();
    let problems: Vec<String> = settings.hotkeys().into_iter().filter_map(|(keys, what)| take_hotkey(app, keys, what).err()).collect();
    (SettingsState(Mutex::new(settings)), (!problems.is_empty()).then(|| problems.join("\n")))
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

fn take_hotkey(app: &AppHandle, hotkey: &str, what: &str) -> Result<(), String> {
    let shortcut = shortcut(hotkey)?;
    if app.global_shortcut().is_registered(shortcut) {
        return Ok(());
    }
    // Windows only says no: the usual reason is another program having it already.
    app.global_shortcut().register(shortcut).map_err(|_| {
        format!("Couldn't use {hotkey} as {what}: another program already has it. Pick another in Settings.")
    })
}

/// Whether two of the hotkeys are the same keys, and says which if so.
fn clash(settings: &Settings) -> Option<String> {
    let hotkeys = settings.hotkeys();
    for (i, (keys, what)) in hotkeys.iter().enumerate() {
        let Ok(parsed) = shortcut(keys) else { continue };
        if let Some((_, other)) = hotkeys[..i].iter().find(|(earlier, _)| shortcut(earlier).is_ok_and(|s| s == parsed)) {
            return Some(format!("{keys} can't be {what}: it's already {other}."));
        }
    }
    None
}

/// Does what the hotkey that was just pressed is for.
pub fn hotkey_pressed(app: &AppHandle, pressed: &Shortcut) {
    let Some(state) = app.try_state::<SettingsState>() else { return };
    let settings = state.get();
    let is = |hotkey: &str| shortcut(hotkey).is_ok_and(|hotkey| hotkey == *pressed);
    if is(&settings.hotkey) {
        recording::toggle(app);
    } else if is(&settings.replay_start_hotkey) {
        recording::start_replay(app);
    } else if is(&settings.replay_stop_hotkey) {
        recording::stop_replay(app);
    } else if is(&settings.replay_save_hotkey) {
        recording::save_replay(app);
    }
}

#[tauri::command]
pub fn get_settings(state: State<SettingsState>) -> Settings {
    state.get()
}

/// Saves `next`, moving the hotkeys if they changed; if a new one can't be had, nothing
/// changes and the old ones stay. An unchanged hotkey that another program had is tried
/// again, as that program may have let go of it. A replay that's on starts over with the
/// new settings.
#[tauri::command]
pub fn set_settings(app: AppHandle, state: State<SettingsState>, next: Settings) -> Result<Settings, String> {
    let next = next.usable();
    if let Some(problem) = clash(&next) {
        return Err(problem);
    }
    let mut current = state.0.lock().unwrap();
    if next.hotkeys().map(|(keys, _)| keys) != current.hotkeys().map(|(keys, _)| keys) {
        let _ = app.global_shortcut().unregister_all();
        for (keys, what) in next.hotkeys() {
            let unchanged = current.hotkeys().iter().any(|(old, _)| *old == keys);
            if let Err(e) = take_hotkey(&app, keys, what)
                && !unchanged
            {
                let _ = app.global_shortcut().unregister_all();
                for (keys, what) in current.hotkeys() {
                    let _ = take_hotkey(&app, keys, what);
                }
                return Err(e);
            }
        }
    } else {
        for (keys, what) in next.hotkeys() {
            let _ = take_hotkey(&app, keys, what);
        }
    }
    save(&app, &next)?;
    let restart = current.replay_differs(&next);
    *current = next.clone();
    drop(current);
    if restart {
        recording::restart_replay(&app);
    }
    Ok(next)
}

/// Lets go of the hotkeys while a new one is being typed in, so pressing one there doesn't
/// start something; then takes them back.
#[tauri::command]
pub fn pause_hotkey(app: AppHandle, state: State<SettingsState>, paused: bool) -> Result<(), String> {
    if paused {
        return app.global_shortcut().unregister_all().map_err(|e| e.to_string());
    }
    let settings = state.get();
    let results: Vec<_> = settings.hotkeys().into_iter().map(|(keys, what)| take_hotkey(&app, keys, what)).collect();
    results.into_iter().collect()
}

/// Whether this hotkey is relight's: not if another program had it first.
#[tauri::command]
pub fn hotkey_working(app: AppHandle, hotkey: String) -> bool {
    shortcut(&hotkey).is_ok_and(|shortcut| app.global_shortcut().is_registered(shortcut))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_saved_before_replay_get_its_defaults() {
        let settings: Settings = serde_json::from_str(r#"{ "hotkey": "F9", "fps": 30 }"#).unwrap();
        assert_eq!(settings.hotkey, "F9");
        assert_eq!(settings.fps, 30);
        assert_eq!(settings.replay_seconds, 30);
        assert!(!settings.replay_microphone, "the microphone stays off unless asked for");
        assert_eq!(settings.replay_save_hotkey, "Alt+F8");
    }

    #[test]
    fn the_default_hotkeys_can_be_used_and_dont_clash() {
        let settings = Settings::default();
        for (keys, _) in settings.hotkeys() {
            shortcut(keys).unwrap();
        }
        assert_eq!(clash(&settings), None);
    }

    #[test]
    fn the_same_keys_for_two_things_is_a_clash_however_theyre_written() {
        let settings = Settings { replay_save_hotkey: "ctrl+5".into(), ..Settings::default() };
        let problem = clash(&settings).unwrap();
        assert!(problem.contains("saves a replay") && problem.contains("recording hotkey"), "{problem}");
    }

    #[test]
    fn replay_starts_over_only_for_what_it_uses() {
        let settings = Settings::default();
        assert!(!settings.replay_differs(&settings.clone()));
        assert!(!settings.replay_differs(&Settings { microphone: false, ..settings.clone() }));
        assert!(!settings.replay_differs(&Settings { hotkey: "F9".into(), ..settings.clone() }));
        assert!(settings.replay_differs(&Settings { replay_seconds: 60, ..settings.clone() }));
        assert!(settings.replay_differs(&Settings { replay_microphone: true, ..settings.clone() }));
        assert!(settings.replay_differs(&Settings { fps: 30, ..settings.clone() }));
    }

    #[test]
    fn a_replay_is_kept_to_a_sensible_length() {
        assert_eq!(Settings { replay_seconds: 0, ..Settings::default() }.usable().replay_seconds, 5);
        assert_eq!(Settings { replay_seconds: 86400, ..Settings::default() }.usable().replay_seconds, 600);
    }
}
