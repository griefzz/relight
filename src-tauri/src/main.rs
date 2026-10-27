// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The relight app: the window's commands, and the tray, hotkey and settings around them.
//! The work itself is in the crates: `relight-media` (rendering, exporting, previews) and
//! `relight-recorder` (screen recording), both running ffmpeg through `relight-ffmpeg`.

mod export;
mod media;
mod process;
mod project;
mod recording;
mod settings;
mod tray;
mod window;

use tauri::Manager;
use tauri_plugin_global_shortcut::ShortcutState;

fn main() {
    process::end_children_on_exit();

    tauri::Builder::default()
        // First, so a second copy hands over before setting anything up.
        .plugin(tauri_plugin_single_instance::init(window::another_instance))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _, event| {
                    if event.state == ShortcutState::Pressed {
                        recording::toggle(app);
                    }
                })
                .build(),
        )
        .manage(window::InitialFile::from_command_line())
        .manage(export::Exports::default())
        .manage(recording::Recorder::default())
        .setup(|app| {
            let (settings, problem) = settings::load(app.handle());
            app.manage(settings);
            app.manage(window::Notice::new(problem));
            tray::create(app.handle())?;
            if let Ok(dir) = media::cache_dir(app.handle()) {
                std::thread::spawn(move || relight_media::previews::prune(&dir));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            window::initial_file,
            window::take_notice,
            window::quit,
            media::open_media,
            media::previews,
            media::preview_peaks,
            project::save_project,
            project::load_project,
            export::export,
            export::cancel_export,
            export::is_exporting,
            settings::get_settings,
            settings::set_settings,
            settings::pause_hotkey,
            settings::hotkey_working,
            recording::toggle_recording,
            recording::recorder_status,
            recording::recording_sources,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
