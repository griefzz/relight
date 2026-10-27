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

/// The loudest moment of the first sound track, in dB (0 is full scale).
pub fn loudest(path: &Path) -> f64 {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-af", "volumedetect", "-f", "null", "-"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&out.stderr);
    let line = log.lines().find(|line| line.contains("max_volume:")).expect("volumedetect said nothing");
    line.split("max_volume:").nth(1).unwrap().trim().trim_end_matches(" dB").parse().unwrap()
}

/// A recording as relight makes them: picture, then sound tracks titled Mix, Game and
/// Microphone. The game is a tone; the microphone is silent.
pub fn recording(dir: &Path) -> PathBuf {
    make(dir, "recording.mp4", &[
        "-f", "lavfi", "-i", "testsrc=size=320x240:rate=30:duration=2",
        "-f", "lavfi", "-i", "sine=frequency=440:duration=2,volume=0.5",
        "-f", "lavfi", "-i", "anullsrc=channel_layout=stereo:sample_rate=48000",
        "-filter_complex", "[1:a]aformat=channel_layouts=stereo,asplit=2[g][m1];[2:a]atrim=duration=2,asplit=2[mic][m2];[m1][m2]amix=inputs=2:normalize=0[mix]",
        "-map", "0:v", "-map", "[mix]", "-map", "[g]", "-map", "[mic]",
        "-metadata:s:a:0", "title=Mix", "-metadata:s:a:1", "title=Game", "-metadata:s:a:2", "title=Microphone",
        "-c:v", "libx264", "-c:a", "aac", "-b:a", "192k",
    ])
}
