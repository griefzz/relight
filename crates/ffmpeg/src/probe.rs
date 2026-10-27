//! Reading a clip's length and streams with ffprobe.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::{ffprobe_path, hide_window};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub path: String,
    pub duration: f64,
    pub size: u64,
    pub video: Option<VideoInfo>,
    pub audio: Option<AudioInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoInfo {
    pub codec: String,
    pub profile: Option<String>,
    pub pix_fmt: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInfo {
    pub codec: String,
    pub bit_rate: Option<u64>,
}

#[derive(Deserialize)]
struct Probed {
    format: ProbedFormat,
    #[serde(default)]
    streams: Vec<ProbedStream>,
}

#[derive(Deserialize)]
struct ProbedFormat {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct ProbedStream {
    codec_type: String,
    codec_name: Option<String>,
    profile: Option<String>,
    pix_fmt: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    bit_rate: Option<String>,
    avg_frame_rate: Option<String>,
    side_data_list: Option<Vec<ProbedSideData>>,
    disposition: Option<ProbedDisposition>,
}

#[derive(Deserialize)]
struct ProbedSideData {
    rotation: Option<f64>,
}

#[derive(Deserialize)]
struct ProbedDisposition {
    #[serde(default)]
    attached_pic: u8,
}

/// Codecs ffmpeg reads text files with, drawing them as pictures: not video.
const TEXT_CODECS: [&str; 4] = ["ansi", "bintext", "xbin", "idf"];

impl ProbedStream {
    /// A moving picture: not a music file's cover art, or a text file drawn as one.
    fn is_picture(&self) -> bool {
        self.codec_type == "video"
            && self.disposition.as_ref().is_none_or(|d| d.attached_pic == 0)
            && !self.codec_name.as_deref().is_some_and(|codec| TEXT_CODECS.contains(&codec))
            && self.width.unwrap_or(0) > 0
            && self.height.unwrap_or(0) > 0
    }

    /// Phones store portrait video as landscape frames plus a rotation, which ffmpeg
    /// and the player both apply.
    fn is_turned(&self) -> bool {
        self.side_data_list
            .iter()
            .flatten()
            .filter_map(|d| d.rotation)
            .any(|r| (r.abs() - 90.0).abs() < 1.0 || (r.abs() - 270.0).abs() < 1.0)
    }
}

/// The clip's length, size as shown, and first video and audio streams.
pub fn probe(path: &Path) -> Result<MediaInfo, String> {
    let mut command = Command::new(ffprobe_path());
    command
        .args(["-v", "error", "-show_entries"])
        .arg(concat!(
            "format=duration",
            ":stream=codec_type,codec_name,profile,pix_fmt,width,height,bit_rate,avg_frame_rate",
            ":stream_side_data=rotation:stream_disposition=attached_pic"
        ))
        .args(["-of", "json"])
        .arg(path);
    let out = hide_window(&mut command)
        .output()
        .map_err(|e| format!("Couldn't run ffprobe ({e}). Is ffmpeg installed and on your PATH?"))?;
    if !out.status.success() {
        return Err(format!(
            "ffprobe couldn't read {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let probed: Probed = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("Couldn't make sense of what ffprobe said: {e}"))?;

    let duration = probed
        .format
        .duration
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0)
        .ok_or_else(|| format!("ffprobe found no length in {}", path.display()))?;
    let video = probed.streams.iter().find(|s| s.is_picture()).map(|s| {
        let (width, height) = (s.width.unwrap_or(0), s.height.unwrap_or(0));
        let (width, height) = if s.is_turned() { (height, width) } else { (width, height) };
        VideoInfo {
            codec: s.codec_name.clone().unwrap_or_default(),
            profile: s.profile.clone(),
            pix_fmt: s.pix_fmt.clone(),
            width,
            height,
            fps: s.avg_frame_rate.as_deref().map_or(30.0, parse_rate),
        }
    });
    let audio = probed.streams.iter().find(|s| s.codec_type == "audio").map(|s| AudioInfo {
        codec: s.codec_name.clone().unwrap_or_default(),
        bit_rate: s.bit_rate.as_deref().and_then(|b| b.parse().ok()),
    });
    let size = fs::metadata(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?.len();

    Ok(MediaInfo { path: path.to_string_lossy().into_owned(), duration, size, video, audio })
}

/// A frame rate as ffprobe writes it ("30000/1001"); 30 when it doesn't say.
fn parse_rate(rate: &str) -> f64 {
    let (num, den) = rate.split_once('/').unwrap_or((rate, "1"));
    match (num.parse::<f64>(), den.parse::<f64>()) {
        (Ok(num), Ok(den)) if den != 0.0 && num != 0.0 => num / den,
        _ => 30.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rates_read_as_ffprobe_writes_them() {
        assert!((parse_rate("30000/1001") - 29.97).abs() < 0.01);
        assert_eq!(parse_rate("60/1"), 60.0);
        assert_eq!(parse_rate("0/0"), 30.0);
        assert_eq!(parse_rate("25"), 25.0);
    }

    #[test]
    #[ignore = "runs ffmpeg"]
    fn turned_phone_video_reads_as_portrait() {
        let dir = tempfile::tempdir().unwrap();
        let (flat, turned) = (dir.path().join("flat.mp4"), dir.path().join("turned.mp4"));
        let ffmpeg = |args: &[&str], out: &Path| {
            let status = Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(args)
                .arg(out)
                .status()
                .unwrap();
            assert!(status.success());
        };
        ffmpeg(&["-f", "lavfi", "-i", "testsrc=size=1280x720:rate=30:duration=1", "-c:v", "libx264"], &flat);
        ffmpeg(&["-display_rotation", "90", "-i", flat.to_str().unwrap(), "-c", "copy"], &turned);
        let video = probe(&turned).unwrap().video.unwrap();
        assert_eq!((video.width, video.height), (720, 1280));
    }

    #[test]
    #[ignore = "runs ffmpeg"]
    fn text_files_are_not_video() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("notes.txt");
        fs::write(&text, "a password, say\n".repeat(50)).unwrap();
        // ffmpeg reads it, as a picture of the text, but it isn't a clip.
        assert!(probe(&text).unwrap().video.is_none());
    }
}
