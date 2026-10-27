//! Reading and writing .relight project files. The window makes and reads their contents;
//! these only touch files of that kind.

use std::fs;
use std::path::Path;

use relight_ffmpeg::Partial;

fn check(path: &Path) -> Result<(), String> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("relight")) {
        Ok(())
    } else {
        Err("Projects are .relight files.".into())
    }
}

/// Writes a project file, replacing the old one only once the new one is all written.
#[tauri::command]
pub fn save_project(path: String, contents: String) -> Result<(), String> {
    let path = Path::new(&path);
    check(path)?;
    let partial = Partial::new(path);
    fs::write(partial.path(), contents).map_err(|e| format!("Couldn't save {}: {e}", path.display()))?;
    partial.finish()
}

#[tauri::command]
pub fn load_project(path: String) -> Result<String, String> {
    check(Path::new(&path))?;
    fs::read_to_string(&path).map_err(|e| format!("Couldn't open {path}: {e}"))
}
