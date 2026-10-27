//! The tray icon. With "keep running" on, closing the window leaves relight here, so the
//! recording hotkey still works.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::recording::{self, Status};
use crate::settings::SettingsState;
use crate::window::show_window;

const ID: &str = "relight";

struct Tray {
    record: MenuItem<Wry>,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open relight", true, None::<&str>)?;
    let record = MenuItem::with_id(app, "record", "Start recording", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit relight", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &record, &PredefinedMenuItem::separator(app)?, &quit])?;
    let mut tray = TrayIconBuilder::with_id(ID)
        .tooltip(idle_tooltip(app))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_window(app),
            "record" => recording::toggle(app),
            "quit" => {
                // The window decides, as it may have unsaved changes to ask about.
                show_window(app);
                let _ = app.emit("quit-requested", ());
            }
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
    app.manage(Tray { record });
    Ok(())
}

fn idle_tooltip(app: &AppHandle) -> String {
    match app.try_state::<SettingsState>() {
        Some(settings) => format!("relight ({} records the screen)", settings.get().hotkey),
        None => "relight".into(),
    }
}

pub fn show_status(app: &AppHandle, status: &Status) {
    let recording = matches!(status, Status::Recording { .. });
    if let Some(tray) = app.try_state::<Tray>() {
        let _ = tray.record.set_text(if recording { "Stop recording" } else { "Start recording" });
    }
    if let Some(icon) = app.tray_by_id(ID) {
        let tooltip = match status {
            Status::Starting => "relight: starting to record".into(),
            Status::Recording { .. } => "relight: recording".into(),
            Status::Saving => "relight: saving the recording".into(),
            Status::Idle => idle_tooltip(app),
        };
        let _ = icon.set_tooltip(Some(tooltip));
    }
}
