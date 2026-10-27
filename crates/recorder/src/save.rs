//! Putting a recording's picture and sound together, or keeping them when that fails.

use std::fs;
use std::path::{Path, PathBuf};

use relight_ffmpeg::{self as ffmpeg, Job, args};

use crate::sound;

/// Puts the picture and sound together in `output`, the sound starting `lead` seconds in,
/// where the picture does. With more than one source, the first track is them mixed (it's
/// what players and the editor play) and each follows on its own, to balance later.
pub fn combine(picture: &Path, lead: f64, tracks: &[sound::Recorded], output: &Path) -> Result<(), String> {
    let length = ffmpeg::probe(picture)?.duration;
    let mut args = args!["-i", picture];
    for track in tracks {
        args.extend(args![
            "-ss", format!("{lead:.4}"),
            "-f", "f32le", "-ar", track.rate.to_string(), "-ch_layout", layout(track.channels),
            "-i", &track.path,
        ]);
    }

    // Every track as 48 kHz stereo: a microphone is usually one channel, which goes to
    // both sides rather than the left.
    let mut graph = String::new();
    let mut maps = args!["-map", "0:v"];
    let mut titles = Vec::new();
    let mixed = tracks.len() > 1;
    for (i, track) in tracks.iter().enumerate() {
        let n = i + 1;
        let stereo = if track.channels == 1 { "pan=stereo|c0=c0|c1=c0" } else { "aformat=channel_layouts=stereo" };
        let outs = if mixed { format!("asplit=2[t{n}][m{n}]") } else { format!("anull[t{n}]") };
        graph.push_str(&format!("[{n}:a]aresample=48000,{stereo},{outs};"));
    }
    if mixed {
        let each: String = (1..=tracks.len()).map(|n| format!("[m{n}]")).collect();
        graph.push_str(&format!("{each}amix=inputs={}:duration=longest:normalize=0[mix];", tracks.len()));
        maps.extend(args!["-map", "[mix]"]);
        titles.push("Mix");
    }
    for (i, track) in tracks.iter().enumerate() {
        maps.extend(args!["-map", format!("[t{}]", i + 1)]);
        titles.push(track.title);
    }
    if !graph.is_empty() {
        graph.pop();
        args.extend(args!["-filter_complex", graph]);
    }
    args.extend(maps);
    args.extend(args!["-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-t", format!("{length:.3}")]);
    for (i, title) in titles.iter().enumerate() {
        args.extend(args![format!("-metadata:s:a:{i}"), format!("title={title}")]);
    }
    args.extend(args!["-movflags", "+faststart", "-f", "mp4", output]);
    ffmpeg::run(&Job::silent(), "saving the recording", length, args)
}

/// How ffmpeg names the usual Windows speaker layout for `channels`.
fn layout(channels: u16) -> String {
    match channels {
        1 => "mono".into(),
        2 => "stereo".into(),
        4 => "quad".into(),
        6 => "5.1".into(),
        8 => "7.1".into(),
        n => format!("{n}c"),
    }
}

/// When the pieces can't be put together, moves them next to where the recording would
/// have gone, so it isn't lost; says where, along with what went wrong.
pub fn keep_pieces(scratch: &Path, output: &Path, error: &str) -> String {
    let mut name = output.with_extension("").into_os_string();
    name.push(" (unsaved)");
    let folder = PathBuf::from(name);
    let moved = fs::create_dir_all(&folder).is_ok()
        && fs::read_dir(scratch).is_ok_and(|files| {
            files.flatten().all(|file| fs::rename(file.path(), folder.join(file.file_name())).is_ok())
        });
    if moved {
        format!(
            "The recording couldn't be saved as a video, but its picture and sound are in {} \
             (the sound as raw 32-bit float samples, at the rate and channels each file's name gives).\n{error}",
            folder.display()
        )
    } else {
        format!("The recording couldn't be saved.\n{error}")
    }
}
