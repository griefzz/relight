//! Making and measuring clips for the tests that run ffmpeg.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Runs ffmpeg with `args`, writing `dir/name`.
pub fn make(dir: &Path, name: &str, args: &[&str]) -> PathBuf {
    let path = dir.join(name);
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg couldn't make {name}");
    path
}

/// How many frames the first video stream really has.
pub fn frames(path: &Path) -> u32 {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-count_frames", "-select_streams", "v:0"])
        .args(["-show_entries", "stream=nb_read_frames", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

/// How long a stream (`v:0`, `a:0`) of the file lasts, in seconds, by its packets.
pub fn stream_length(path: &Path, stream: &str) -> f64 {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", stream])
        .args(["-show_entries", "packet=pts_time,duration_time", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let (pts, duration) = line.split_once(',')?;
            Some(pts.parse::<f64>().ok()? + duration.trim_end_matches(',').parse::<f64>().ok()?)
        })
        .fold(0.0, f64::max)
}

/// Whether ffmpeg decodes the whole file without complaint.
pub fn decodes_cleanly(path: &Path) -> bool {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-v", "error", "-i"])
        .arg(path)
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    out.status.success() && out.stderr.is_empty()
}
