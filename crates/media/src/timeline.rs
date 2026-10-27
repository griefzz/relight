//! Rendering a timeline (pieces cut from source clips, end to end) to one file.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt::Write;
use std::fs;
use std::path::Path;

use relight_ffmpeg::{self as ffmpeg, Job, Scratch, VideoInfo, args};
use serde::Deserialize;

/// What the rendered picture should be, where it differs from the first clip's. Neither
/// goes above the first clip's: a clip is never scaled up or given made-up frames.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Picture {
    /// The shorter side in pixels (720 for 1280×720 or for 720×1280).
    pub short_side: Option<u32>,
    pub fps: Option<f64>,
}

impl Picture {
    /// The size and frame rate to render at, shaped like the first clip.
    pub fn for_clip(&self, first: &VideoInfo) -> (u32, u32, f64) {
        let (w, h) = (first.width, first.height);
        let (w, h) = match self.short_side {
            Some(short) if short < w.min(h) && w >= h => (w * short / h, short),
            Some(short) if short < w.min(h) => (short, h * short / w),
            _ => (w, h),
        };
        let fps = self.fps.filter(|&fps| fps > 0.0 && fps < first.fps).unwrap_or(first.fps);
        // yuv420p needs even sizes.
        (w / 2 * 2, h / 2 * 2, fps)
    }

    /// Whether the first clip would render as it is.
    pub fn keeps(&self, first: &VideoInfo) -> bool {
        let (w, h, fps) = self.for_clip(first);
        (w, h, fps) == (first.width / 2 * 2, first.height / 2 * 2, first.fps)
    }
}

/// One piece of the timeline: `start` to `end` seconds of the clip at `path`.
#[derive(Debug, Clone, Deserialize)]
pub struct Segment {
    pub path: String,
    pub start: f64,
    pub end: f64,
}

impl Segment {
    pub fn length(&self) -> f64 {
        self.end - self.start
    }

    fn check(&self) -> Result<(), String> {
        if self.start.is_finite() && self.end.is_finite() && 0.0 <= self.start && self.start < self.end {
            return Ok(());
        }
        Err(format!("A piece of {} runs from {}s to {}s, which isn't a length.", self.path, self.start, self.end))
    }
}

/// How a render is encoded: its picture, its sound, and the file they go in.
pub struct Encoding {
    video: Vec<OsString>,
    audio: Vec<OsString>,
    container: Vec<OsString>,
}

/// For the finished file: visually lossless H.264 and AAC.
pub fn full_quality() -> Encoding {
    Encoding {
        video: args!["-c:v", "libx264", "-preset", "slow", "-crf", "16", "-pix_fmt", "yuv420p"],
        audio: args!["-c:a", "aac", "-b:a", "320k"],
        container: args!["-movflags", "+faststart", "-f", "mp4"],
    }
}

/// For a render that gets encoded again: quick, near lossless, and uncompressed sound so
/// the second encode starts from the original.
pub fn intermediate() -> Encoding {
    Encoding {
        video: args!["-c:v", "libx264", "-preset", "veryfast", "-crf", "12", "-pix_fmt", "yuv420p"],
        audio: args!["-c:a", "pcm_s16le"],
        container: args!["-f", "mov"],
    }
}

/// ffmpeg rounds a seek to microseconds, which can land just past a frame's timestamp
/// and drop it; seeking this much early keeps the first frame (and less than a frame,
/// so it doesn't pick up the one before).
const SEEK_MARGIN: f64 = 0.0005;

/// The most pieces one ffmpeg run renders. ffmpeg opens every input at once, each with a
/// decoder holding about 100 MB of frames, so a timeline with more is rendered a few
/// pieces at a time and the parts joined.
const PIECES_PER_RUN: usize = 8;

/// Decoder threads for each piece. Only one piece is decoded at a time, and more
/// threads would only hold more frames.
const DECODER_THREADS: &str = "2";

/// How much of the progress rendering the parts is; joining them is the rest.
const RENDERING: f64 = 0.97;

/// A piece as it's rendered: whether its clip has sound, and how long it lasts in whole
/// frames, so picture and sound end together at every join.
struct Piece<'a> {
    segment: &'a Segment,
    sound: bool,
    length: f64,
}

/// Renders `segments` end to end into `output`, encoded as `encoding` says. The picture
/// is shaped like the first clip, sized by `picture`; other clips are fitted inside it.
pub fn render(
    job: &Job,
    segments: &[Segment],
    picture: &Picture,
    output: &Path,
    encoding: &Encoding,
) -> Result<(), String> {
    render_in_runs(job, segments, picture, output, encoding, PIECES_PER_RUN)
}

fn render_in_runs(
    job: &Job,
    segments: &[Segment],
    picture: &Picture,
    output: &Path,
    encoding: &Encoding,
    per_run: usize,
) -> Result<(), String> {
    let first = segments.first().ok_or("The timeline is empty.")?;
    let mut sources = HashMap::new();
    for segment in segments {
        segment.check()?;
        if !sources.contains_key(segment.path.as_str()) {
            let info = ffmpeg::probe(Path::new(&segment.path))?;
            if info.video.is_none() {
                return Err(format!("{} has no video in it.", segment.path));
            }
            sources.insert(segment.path.as_str(), info);
        }
    }
    let frame = picture.for_clip(sources[first.path.as_str()].video.as_ref().expect("checked above"));
    let fps = frame.2;
    let pieces: Vec<Piece> = segments
        .iter()
        .map(|segment| Piece {
            segment,
            sound: sources[segment.path.as_str()].audio.is_some(),
            length: (segment.length() * fps).round().max(1.0) / fps,
        })
        .collect();
    let total: f64 = pieces.iter().map(|piece| piece.length).sum();

    if pieces.len() <= per_run {
        let mut args = pieces_args(&pieces, frame);
        args.extend(encoding.video.iter().cloned());
        args.extend(encoding.audio.iter().cloned());
        args.extend(encoding.container.iter().cloned());
        args.push(output.into());
        return ffmpeg::run(job, "rendering the timeline", total, args);
    }

    // In parts, joined without encoding the picture again: each part's picture is encoded
    // as the finished file's is, and its sound left uncompressed for the join to encode
    // once, with no gaps at the joins.
    let scratch = Scratch::new()?;
    let mut list = String::new();
    let mut done = 0.0;
    for (n, part) in pieces.chunks(per_run).enumerate() {
        let length: f64 = part.iter().map(|piece| piece.length).sum();
        let name = format!("part{n:04}.mov");
        let mut args = pieces_args(part, frame);
        args.extend(encoding.video.iter().cloned());
        args.extend(args!["-c:a", "pcm_f32le", "-f", "mov", scratch.path().join(&name)]);
        let share = job.part(done / total * RENDERING, (done + length) / total * RENDERING);
        ffmpeg::run(&share, "rendering the timeline", length, args)?;
        done += length;
        let _ = writeln!(list, "file '{name}'");
    }
    let list_path = scratch.path().join("parts.txt");
    fs::write(&list_path, list).map_err(|e| format!("Couldn't write the list of parts: {e}"))?;

    let mut args = args!["-f", "concat", "-i", &list_path, "-map", "0:v", "-map", "0:a", "-c:v", "copy"];
    args.extend(encoding.audio.iter().cloned());
    args.extend(encoding.container.iter().cloned());
    args.push(output.into());
    ffmpeg::run(&job.part(RENDERING, 1.0), "joining the timeline's parts", total, args)
}

/// The inputs and filters for rendering `pieces` end to end as `[v]` and `[a]`, at the
/// size and frame rate in `frame`.
fn pieces_args(pieces: &[Piece], (w, h, fps): (u32, u32, f64)) -> Vec<OsString> {
    let mut inputs = Vec::new();
    let mut graph = String::new();
    let mut joined = String::new();
    for (i, piece) in pieces.iter().enumerate() {
        let segment = piece.segment;
        let length = piece.length;
        inputs.extend(args![
            "-threads", DECODER_THREADS,
            "-ss", format!("{:.6}", (segment.start - SEEK_MARGIN).max(0.0)),
            "-t", format!("{:.6}", segment.length()),
            "-i", &segment.path,
        ]);
        // A clip whose picture stops before its sound holds its last frame to the end.
        let _ = write!(
            graph,
            "[{i}:v:0]fps={fps},scale={w}:{h}:force_original_aspect_ratio=decrease,\
             pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,format=yuv420p,\
             tpad=stop_mode=clone:stop_duration={length:.6},trim=duration={length:.6},setpts=PTS-STARTPTS[v{i}];"
        );
        // Every piece gets exactly its length of 48 kHz stereo, silent if the clip has
        // no sound, so the sound can't drift from the picture across cuts.
        if piece.sound {
            let _ = write!(
                graph,
                "[{i}:a:0]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,\
                 apad=whole_dur={length:.6},atrim=duration={length:.6},asetpts=PTS-STARTPTS[a{i}];"
            );
        } else {
            let _ = write!(graph, "anullsrc=channel_layout=stereo:sample_rate=48000,atrim=duration={length:.6}[a{i}];");
        }
        let _ = write!(joined, "[v{i}][a{i}]");
    }
    // concat forgets the frame rate, and the encoder would then assume 25 fps and drop
    // frames to fit.
    let _ = write!(graph, "{joined}concat=n={}:v=1:a=1[joined][a];[joined]fps={fps}[v]", pieces.len());

    let mut args = inputs;
    args.extend(args!["-filter_complex", graph, "-map", "[v]", "-map", "[a]"]);
    args
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::testing::{decodes_cleanly, frames, make, stream_length};

    /// Clips to cut: 4s of 720p30 with sound, and 3s of 480p60 without.
    fn clips(dir: &Path) -> (String, String) {
        let a = make(dir, "a.mp4", &[
            "-f", "lavfi", "-i", "testsrc=size=1280x720:rate=30:duration=4",
            "-f", "lavfi", "-i", "sine=duration=4", "-c:v", "libx264", "-c:a", "aac",
        ]);
        let b = make(dir, "b.mp4", &[
            "-f", "lavfi", "-i", "testsrc2=size=640x480:rate=60:duration=3", "-c:v", "libx264",
        ]);
        let path = |p: PathBuf| p.to_string_lossy().into_owned();
        (path(a), path(b))
    }

    fn timeline(a: &str, b: &str) -> [Segment; 3] {
        [
            Segment { path: a.into(), start: 1.0, end: 2.5 },
            Segment { path: b.into(), start: 0.0, end: 3.0 },
            Segment { path: a.into(), start: 3.0, end: 4.0 },
        ]
    }

    #[test]
    #[ignore = "runs ffmpeg"]
    fn renders_pieces_end_to_end_to_the_frame() {
        let scratch = Scratch::new().unwrap();
        let dir = scratch.path();
        let (a, b) = clips(dir);
        let out = dir.join("out.mp4");
        render(&Job::silent(), &timeline(&a, &b), &Picture::default(), &out, &full_quality()).unwrap();

        let info = ffmpeg::probe(&out).unwrap();
        let video = info.video.unwrap();
        assert_eq!((video.width, video.height), (1280, 720));
        assert!(info.audio.is_some());
        // 1.5s + 3s + 1s at 30 fps.
        assert_eq!(frames(&out), 165);
        assert!((video.fps - 30.0).abs() < 0.01, "{} fps", video.fps);
    }

    #[test]
    #[ignore = "runs ffmpeg"]
    fn renders_in_parts_the_same_as_in_one_go() {
        let scratch = Scratch::new().unwrap();
        let dir = scratch.path();
        let (a, b) = clips(dir);
        let out = dir.join("out.mp4");
        // Two parts: the first two pieces, then the last.
        render_in_runs(&Job::silent(), &timeline(&a, &b), &Picture::default(), &out, &full_quality(), 2).unwrap();

        assert_eq!(frames(&out), 165);
        assert!(decodes_cleanly(&out));
        let (video, sound) = (stream_length(&out, "v:0"), stream_length(&out, "a:0"));
        assert!((video - 5.5).abs() < 0.001, "picture lasts {video}s");
        // AAC's last packet can run a little over.
        assert!((sound - 5.5).abs() < 0.025, "sound lasts {sound}s");
    }

    #[test]
    #[ignore = "runs ffmpeg"]
    fn a_picture_shorter_than_its_sound_holds_its_last_frame() {
        let scratch = Scratch::new().unwrap();
        let dir = scratch.path();
        let short = make(dir, "short.mp4", &[
            "-f", "lavfi", "-i", "testsrc=size=640x360:rate=30:duration=2",
            "-f", "lavfi", "-i", "sine=duration=3", "-c:v", "libx264", "-c:a", "aac",
        ]);
        let short = short.to_string_lossy().into_owned();
        let segments = [
            Segment { path: short.clone(), start: 0.0, end: 3.0 },
            Segment { path: short, start: 0.0, end: 1.0 },
        ];
        let out = dir.join("out.mov");
        // Each piece its own part, so the held frames have to carry across a join.
        render_in_runs(&Job::silent(), &segments, &Picture::default(), &out, &intermediate(), 1).unwrap();
        assert_eq!(frames(&out), 120);
        assert!((stream_length(&out, "a:0") - 4.0).abs() < 0.001);
    }

    #[test]
    fn pieces_that_are_not_a_length_are_refused() {
        let piece = |start, end| Segment { path: "clip.mp4".into(), start, end };
        assert!(piece(0.0, 1.0).check().is_ok());
        assert!(piece(1.0, 1.0).check().is_err());
        assert!(piece(-1.0, 1.0).check().is_err());
        assert!(piece(0.0, f64::INFINITY).check().is_err());
        let refused = render(&Job::silent(), &[piece(2.0, 1.0)], &Picture::default(), Path::new("out.mp4"), &full_quality());
        assert!(refused.unwrap_err().contains("isn't a length"));
    }

    fn video(width: u32, height: u32, fps: f64) -> VideoInfo {
        VideoInfo { codec: "h264".into(), profile: None, pix_fmt: None, width, height, fps }
    }

    #[test]
    fn picture_scales_down_by_the_short_side_and_never_up() {
        let smaller = Picture { short_side: Some(720), fps: Some(30.0) };
        assert_eq!(smaller.for_clip(&video(1920, 1080, 60.0)), (1280, 720, 30.0));
        assert_eq!(smaller.for_clip(&video(1080, 1920, 60.0)), (720, 1280, 30.0));
        assert_eq!(smaller.for_clip(&video(640, 480, 24.0)), (640, 480, 24.0));
        assert!(!smaller.keeps(&video(1920, 1080, 60.0)));
        assert!(smaller.keeps(&video(640, 480, 24.0)));
        // Sizes come out even, as yuv420p needs.
        assert_eq!(Picture::default().for_clip(&video(853, 481, 30.0)), (852, 480, 30.0));
        // A frame rate of nothing means the clip's own.
        assert_eq!(Picture { short_side: None, fps: Some(0.0) }.for_clip(&video(640, 480, 24.0)).2, 24.0);
    }
}
