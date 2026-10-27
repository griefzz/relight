//! Opening media for the window: reading what's in a file, letting the player stream it,
//! and the thumbnails and waveforms the timeline shows.

use std::path::{Path, PathBuf};

use relight_ffmpeg::{self as ffmpeg, MediaInfo};
use relight_media::previews::{self, Previews};
use tauri::ipc::Response;
use tauri::{AppHandle, Manager};

/// What the player can play; anything else can still be edited and exported, just not
/// watched, so it's not handed to the player at all.
const PLAYABLE: [&str; 5] = ["mp4", "m4v", "mov", "webm", "mkv"];

pub fn cache_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_cache_dir().map_err(|e| e.to_string())?.join("previews"))
}

/// Reads a clip's streams, and lets the player stream it.
#[tauri::command]
pub async fn open_media(app: AppHandle, path: String) -> Result<MediaInfo, String> {
    let probed = path.clone();
    let info = tauri::async_runtime::spawn_blocking(move || ffmpeg::probe(Path::new(&probed)))
        .await
        .map_err(|e| e.to_string())??;
    if info.video.is_none() {
        return Err(format!("{path} has no video in it."));
    }
    let playable = Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| PLAYABLE.iter().any(|p| p.eq_ignore_ascii_case(e)));
    if playable {
        app.asset_protocol_scope().allow_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(info)
}

/// The previews of the clip at `path`, making them if they aren't cached yet.
#[tauri::command]
pub async fn previews(app: AppHandle, path: String) -> Result<Previews, String> {
    let dir = cache_dir(&app)?;
    let previews = tauri::async_runtime::spawn_blocking(move || previews::make(&dir, Path::new(&path)))
        .await
        .map_err(|e| e.to_string())??;
    if let Some(strip) = &previews.strip {
        app.asset_protocol_scope().allow_file(strip).map_err(|e| e.to_string())?;
    }
    Ok(previews)
}

/// The peaks `previews` made for the clip at `path`: one byte each, 0 to 255.
#[tauri::command]
pub async fn preview_peaks(app: AppHandle, path: String) -> Result<Response, String> {
    let dir = cache_dir(&app)?;
    let peaks = tauri::async_runtime::spawn_blocking(move || previews::peaks(&dir, Path::new(&path)))
        .await
        .map_err(|e| e.to_string())??;
    Ok(Response::new(peaks))
}
