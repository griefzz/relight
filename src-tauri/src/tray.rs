//! The tray icon. With "keep running" on, closing the window leaves relight here, so the
//! hotkeys still work.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::recording::{self, ReplayStatus, Status};
use crate::settings::SettingsState;
use crate::window::{ask_to_quit, show_window};

const ID: &str = "relight";

struct Tray {
    record: MenuItem<Wry>,
    replay: MenuItem<Wry>,
    save_replay: MenuItem<Wry>,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open relight", true, None::<&str>)?;
    let record = MenuItem::with_id(app, "record", "Start recording", true, None::<&str>)?;
    let replay = MenuItem::with_id(app, "replay", "Start instant replay", true, None::<&str>)?;
    let save_replay = MenuItem::with_id(app, "save-replay", "Save replay", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit relight", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &record, &replay, &save_replay, &PredefinedMenuItem::separator(app)?, &quit])?;
    let mut tray = TrayIconBuilder::with_id(ID)
        .tooltip(idle_tooltip(app))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_window(app),
            "record" => recording::toggle(app),
            "replay" => recording::toggle_replay(app),
            "save-replay" => recording::save_replay(app),
            "quit" => ask_to_quit(app),
            _ => {}
        })
        .on_tray_icon_event(|icon, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_window(icon.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    app.manage(Tray { record, replay, save_replay });
    Ok(())
}

fn idle_tooltip(app: &AppHandle) -> String {
    match app.try_state::<SettingsState>() {
        Some(settings) => format!("relight ({} records the screen)", settings.get().hotkey),
        None => "relight".into(),
    }
}

fn replay_tooltip(app: &AppHandle, replay: &ReplayStatus) -> Option<String> {
    match replay {
        ReplayStatus::Starting => Some("relight: starting instant replay".into()),
        ReplayStatus::On { saving: true, .. } => Some("relight: saving a replay".into()),
        ReplayStatus::On { interrupted: true, .. } => Some("relight: instant replay is waiting for the screen".into()),
        ReplayStatus::On { .. } => {
            let settings = app.try_state::<SettingsState>()?;
            Some(format!("relight: instant replay on ({} saves it)", settings.get().replay_save_hotkey))
        }
        ReplayStatus::Off | ReplayStatus::Paused => None,
    }
}

pub fn show_status(app: &AppHandle, status: &Status, replay: &ReplayStatus) {
    let recording = matches!(status, Status::Recording { .. });
    if let Some(tray) = app.try_state::<Tray>() {
        let _ = tray.record.set_text(if recording { "Stop recording" } else { "Start recording" });
        let off = matches!(replay, ReplayStatus::Off);
        let _ = tray.replay.set_text(if off { "Start instant replay" } else { "Stop instant replay" });
        let _ = tray.save_replay.set_enabled(matches!(replay, ReplayStatus::On { interrupted: false, .. }));
    }
    if let Some(icon) = app.tray_by_id(ID) {
        let tooltip = match status {
            Status::Starting => "relight: starting to record".into(),
            Status::Recording { .. } => "relight: recording".into(),
            Status::Saving => "relight: saving the recording".into(),
            Status::Idle => replay_tooltip(app, replay).unwrap_or_else(|| idle_tooltip(app)),
        };
        let _ = icon.set_tooltip(Some(tooltip));
    }
}
