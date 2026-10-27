//! Re-encode a video to fit under a Discord upload limit, as well as it can look there.
//!
//! H.264 + AAC in MP4 by default: the one combination every Discord client plays
//! inline (desktop, web, Android, iOS). A clip that already fits and plays
//! everywhere is copied as it is. AV1 looks better on long clips (on short ones
//! the gain is small, and its worst moments are worse), but only plays where the
//! device decodes AV1 (desktop and web do; older iPhones and some Android phones
//! show a thumbnail or nothing).

use std::fs;
use std::path::Path;

use relight_ffmpeg::{self as ffmpeg, Job, MediaInfo, NULL_OUTPUT, Scratch, args};
use serde::Deserialize;

/// The hard max on a video upload; decimal MB, so it's under either way.
pub const TARGET_MB: f64 = 20.0;
/// Most the audio gets; less when the whole budget is tight.
const AUDIO_KBPS: u32 = 128;
/// Aim this far under the limit, for the MP4's own overhead.
const MARGIN: f64 = 0.985;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// Size to fit under, in MB.
    pub target_mb: f64,
    /// AV1: better on long clips (little gain on short ones), but older phones can't play it.
    pub av1: bool,
    pub tune: Tune,
}

impl Default for Options {
    fn default() -> Self {
        Self { target_mb: TARGET_MB, av1: false, tune: Tune::Animation }
    }
}

/// What x264 tunes for.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tune {
    /// Suits flat colour and ink lines, like the game's.
    Animation,
    /// Camera footage.
    Film,
}

impl Tune {
    fn as_str(self) -> &'static str {
        match self {
            Tune::Animation => "animation",
            Tune::Film => "film",
        }
    }
}

fn plays_everywhere(info: &MediaInfo) -> bool {
    let Some(video) = &info.video else { return false };
    video.codec == "h264"
        && video.pix_fmt.as_deref() == Some("yuv420p")
        && matches!(
            video.profile.as_deref(),
            Some("Constrained Baseline" | "Baseline" | "Main" | "High")
        )
        && info.audio.as_ref().is_none_or(|a| a.codec == "aac")
}

/// The audio's share: all of AUDIO_KBPS unless that would starve the picture.
fn audio_kbps(total_kbps: f64) -> u32 {
    ((total_kbps * 0.12) as u32 / 16 * 16).clamp(64, AUDIO_KBPS)
}

/// The tallest of the usual heights that still gets enough bits per pixel.
///
/// Too few bits per pixel and the encoder smears detail and blocks; fewer, cleaner
/// pixels look better, and Discord's inline player shows the video small anyway.
fn pick_height(width: u32, height: u32, fps: f64, video_kbps: f64, av1: bool) -> u32 {
    let short = width.min(height);
    let long = width.max(height);
    let enough = if av1 { 0.02 } else { 0.05 }; // bits per pixel per frame
    for h in [2160, 1440, 1080, 900, 720, 540] {
        if h > short {
            continue;
        }
        let pixels = (h * h) as f64 * long as f64 / short as f64;
        if video_kbps * 1000.0 / (pixels * fps) >= enough {
            return h;
        }
    }
    short.min(540)
}

fn file_bits(path: &Path) -> Result<f64, String> {
    Ok(fs::metadata(path).map_err(|e| e.to_string())?.len() as f64 * 8.0)
}

/// Encodes `input` to an MP4 at `output` that fits under `opts.target_mb`.
pub fn encode(job: &Job, input: &Path, output: &Path, opts: &Options) -> Result<(), String> {
    let info = ffmpeg::probe(input)?;
    let video = info.video.as_ref().ok_or("The clip has no video in it.")?;
    let (duration, width, height, fps) = (info.duration, video.width, video.height, video.fps);
    let target_mb = opts.target_mb;
    if !(target_mb.is_finite() && target_mb > 0.0) {
        return Err(format!("{target_mb} MB isn't a size to fit under."));
    }
    let target_bytes = target_mb * 1000.0 * 1000.0;
    let total_kbps = target_bytes * 8.0 * MARGIN / duration / 1000.0;

    if info.size as f64 <= target_bytes * MARGIN && plays_everywhere(&info) {
        job.note("Already fits and plays everywhere: copying it as it is.");
        job.stage("Copying");
        return ffmpeg::run(job, "the copy", duration, args![
            "-i", input, "-map", "0:v:0", "-map", "0:a:0?", "-c", "copy",
            "-movflags", "+faststart", "-f", "mp4", output,
        ]);
    }

    let scratch = Scratch::new()?;
    let tmp = scratch.path();

    // The sound first, so the picture gets exactly what's left.
    let mut audio_file = None;
    if let Some(audio) = &info.audio {
        let akbps = audio_kbps(total_kbps);
        let path = tmp.join("audio.m4a");
        let source_kbps = audio.bit_rate.unwrap_or(0) as f64 / 1000.0;
        job.stage("Audio");
        if audio.codec == "aac" && 0.0 < source_kbps && source_kbps <= akbps as f64 * 1.05 {
            ffmpeg::run(job, "audio", duration, args![
                "-i", input, "-map", "0:a:0", "-c:a", "copy", &path,
            ])?;
        } else {
            ffmpeg::run(job, "audio", duration, args![
                "-i", input, "-map", "0:a:0", "-c:a", "aac", "-b:a", format!("{akbps}k"), &path,
            ])?;
        }
        audio_file = Some(path);
    }
    let audio_bits = match &audio_file {
        Some(path) => file_bits(path)?,
        None => 0.0,
    };
    let mut video_kbps = (target_bytes * 8.0 * MARGIN - audio_bits) / duration / 1000.0;
    if video_kbps < 100.0 {
        return Err(format!("{duration:.0}s is too long to fit in {target_mb} MB."));
    }

    let short = pick_height(width, height, fps, video_kbps, opts.av1);
    let scale = if short < width.min(height) {
        let size = if width >= height { format!("scale=-2:{short}") } else { format!("scale={short}:-2") };
        args!["-vf", format!("{size}:flags=lanczos")]
    } else {
        vec![]
    };

    job.note(format!(
        "Input: {:.1} MB, {duration:.1}s, {width}x{height} @ {fps:.0} fps",
        info.size as f64 / 1000.0 / 1000.0
    ));
    job.note(format!(
        "Target: {target_mb} MB ({} kbps video at {short}p + {:.0} kbps audio)",
        video_kbps as u64,
        audio_bits / duration / 1000.0
    ));

    // Rate control only gets near the size asked for, so measure each try, keep the
    // biggest that fits, and aim again if it's over or well under.
    let aim = target_bytes * 8.0 * MARGIN;
    if opts.av1 {
        video_kbps *= 0.92; // SVT-AV1's one pass comes out about 8% big
    }
    let video_file = tmp.join("video.mp4");
    let best_file = tmp.join("best.mp4");
    let mut best_size = 0.0;
    let log = tmp.join("pass");
    for attempt in 0..3 {
        let bitrate = args!["-b:v", format!("{}k", video_kbps as u64)];
        if opts.av1 {
            // SVT-AV1 has no second pass through ffmpeg: one pass, measured below.
            job.stage(if attempt == 0 { "Encoding (AV1)" } else { "Encoding again, re-aimed" });
            // Preset 6: about real time at 1080p; 4 is a point better and three times slower.
            let mut av1 = args!["-i", input, "-map", "0:v:0"];
            av1.extend(scale.iter().cloned());
            av1.extend(args![
                "-fps_mode", "cfr", "-c:v", "libsvtav1", "-preset", "6",
                "-pix_fmt", "yuv420p10le", "-g", format!("{}", (fps * 10.0).round() as u64),
            ]);
            av1.extend(bitrate);
            av1.push((&video_file).into());
            ffmpeg::run(job, "the AV1 encode", duration, av1)?;
        } else {
            // A steady frame rate, or pass 2 can count more frames than pass 1 did
            // (jittery recording timestamps get frames duplicated going into MP4).
            let mut x264 = args!["-i", input, "-map", "0:v:0"];
            x264.extend(scale.iter().cloned());
            x264.extend(args![
                "-fps_mode", "cfr", "-c:v", "libx264", "-preset", "veryslow",
                "-tune", opts.tune.as_str(), "-x264-params", "aq-mode=3",
                "-profile:v", "high", "-pix_fmt", "yuv420p",
            ]);
            x264.extend(bitrate);
            x264.extend(args!["-passlogfile", &log]);
            if attempt == 0 {
                job.stage("Pass 1/2");
                let pass1 = [x264.clone(), args!["-pass", "1", "-an", "-f", "null", NULL_OUTPUT]].concat();
                ffmpeg::run(job, "pass 1", duration, pass1)?;
            }
            job.stage(if attempt == 0 { "Pass 2/2" } else { "Pass 2 again, re-aimed" });
            ffmpeg::run(job, "pass 2", duration, [x264, args!["-pass", "2", &video_file]].concat())?;
        }

        let size = file_bits(&video_file)? + audio_bits;
        if best_size < size && size <= aim * 1.005 {
            fs::rename(&video_file, &best_file).map_err(|e| e.to_string())?;
            best_size = size;
            if size >= aim * 0.97 {
                break;
            }
        }
        video_kbps *= (aim - audio_bits) / (size - audio_bits).max(1.0) * if size > aim { 0.99 } else { 1.0 };
    }
    if best_size == 0.0 {
        return Err(format!("Couldn't get it under {target_mb} MB."));
    }

    job.stage("Finishing");
    let mut mux = args!["-i", &best_file];
    if let Some(audio_file) = &audio_file {
        mux.extend(args!["-i", audio_file, "-map", "0:v", "-map", "1:a"]);
    } else {
        mux.extend(args!["-map", "0:v"]);
    }
    mux.extend(args!["-c", "copy", "-movflags", "+faststart", "-f", "mp4", output]);
    ffmpeg::run(job, "muxing", duration, mux)?;

    let output_mb = fs::metadata(output).map_err(|e| e.to_string())?.len() as f64 / 1000.0 / 1000.0;
    if output_mb > target_mb {
        return Err(format!("It came out at {output_mb:.2} MB, over the {target_mb} MB limit."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_gets_its_share_within_bounds() {
        assert_eq!(audio_kbps(10_000.0), 128);
        assert_eq!(audio_kbps(800.0), 96);
        assert_eq!(audio_kbps(100.0), 64);
    }

    #[test]
    fn height_drops_when_bits_are_short() {
        // 20 MB over 30s of 1080p60: about 5.2 Mbps, enough for 900p but not 1080p.
        assert_eq!(pick_height(1920, 1080, 60.0, 5200.0, false), 900);
        assert_eq!(pick_height(1920, 1080, 60.0, 5200.0, true), 1080);
        // Portrait clips go by their short side too.
        assert_eq!(pick_height(1080, 1920, 30.0, 50_000.0, false), 1080);
        // Never taller than the clip.
        assert_eq!(pick_height(640, 360, 30.0, 50_000.0, false), 360);
    }
}
