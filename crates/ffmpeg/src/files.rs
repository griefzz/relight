//! The files jobs write: temporary folders for what's in between, and finished files that
//! only appear once they're whole.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

/// Removes a file or folder ffmpeg was just using. Windows keeps a killed process's
/// files locked for a moment after it exits, so this tries again for up to a second.
pub fn remove_soon(path: &Path) {
    for _ in 0..20 {
        let removed = if path.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) };
        if removed.is_ok() || !path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// A temporary folder for a job's in-between files, removed when dropped.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new() -> Result<Self, String> {
        let dir = tempfile::Builder::new()
            .prefix("relight-")
            .tempdir()
            .map_err(|e| format!("Couldn't make a temporary folder: {e}"))?;
        Ok(Self(dir.keep()))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        remove_soon(&self.0);
    }
}

/// A file written beside where it goes ("name.mp4.part") and moved there once finished,
/// so a failed or cancelled job never leaves half a file where the whole one should be.
/// Dropped unfinished, the part is removed.
pub struct Partial {
    part: PathBuf,
    destination: PathBuf,
    finished: bool,
}

impl Partial {
    pub fn new(path: &Path) -> Self {
        let mut part = path.as_os_str().to_owned();
        part.push(".part");
        Self { part: part.into(), destination: path.to_owned(), finished: false }
    }

    /// Where to write it meanwhile.
    pub fn path(&self) -> &Path {
        &self.part
    }

    /// Puts the written file where it goes, replacing anything there.
    pub fn finish(mut self) -> Result<(), String> {
        fs::rename(&self.part, &self.destination)
            .map_err(|e| format!("Couldn't save {}: {e}", self.destination.display()))?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        if !self.finished && self.part.exists() {
            remove_soon(&self.part);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_file_appears_only_when_finished() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.mp4");

        let failed = Partial::new(&path);
        fs::write(failed.path(), "half").unwrap();
        drop(failed);
        assert!(!path.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0, "the part is cleaned up");

        fs::write(&path, "old").unwrap();
        let saved = Partial::new(&path);
        fs::write(saved.path(), "new").unwrap();
        saved.finish().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
