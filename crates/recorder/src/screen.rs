//! The picture side of a recording: which screens there are, and ffmpeg capturing one.

use std::path::Path;
use std::process::ChildStdout;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use ffmpeg_sidecar::child::FfmpegChild;
use ffmpeg_sidecar::event::{FfmpegEvent, LogLevel};
use serde::Serialize;

use relight_ffmpeg::{args, command, last_lines, start_failed};

use crate::chunks::{Clock, KEYFRAME_SECONDS};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Screen {
    /// Its DXGI output number, which is how ffmpeg's ddagrab picks it.
    pub index: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// The screens ddagrab can record: those on the first graphics adapter, which is where
/// its capture device opens.
#[cfg(windows)]
pub fn screens() -> Vec<Screen> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

    let mut found = Vec::new();
    // SAFETY: DXGI calls on objects made here, used only within this block.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return found };
        let Ok(adapter) = factory.EnumAdapters1(0) else { return found };
        let mut index = 0;
        while let Ok(output) = adapter.EnumOutputs(index) {
            if let Ok(desc) = output.GetDesc() {
                let r = desc.DesktopCoordinates;
                found.push(Screen {
                    index,
                    x: r.left,
                    y: r.top,
                    width: (r.right - r.left) as u32,
                    height: (r.bottom - r.top) as u32,
                    // The main screen is the one at the desktop's origin.
                    primary: r.left == 0 && r.top == 0,
                });
            }
            index += 1;
        }
    }
    found
}

#[cfg(not(windows))]
pub fn screens() -> Vec<Screen> {
    Vec::new()
}

/// Which of `screens` the mouse is on.
#[cfg(windows)]
pub fn under_mouse(screens: &[Screen]) -> Option<u32> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let mut at = POINT::default();
    // SAFETY: writes the cursor position into `at`.
    unsafe { GetCursorPos(&mut at).ok()? };
    screens
        .iter()
        .find(|s| at.x >= s.x && at.x < s.x + s.width as i32 && at.y >= s.y && at.y < s.y + s.height as i32)
        .map(|s| s.index)
}

#[cfg(not(windows))]
pub fn under_mouse(_screens: &[Screen]) -> Option<u32> {
    None
}

/// Prints the wall-clock time (microseconds since 1970) as the first frame goes by, and
/// changes no timestamps: it's what the separately captured sound is lined up against.
const FIRST_FRAME_TIME: &str = r"setpts='PTS+0*if(eq(N\,0)\,print(RTCTIME)\,0)'";

/// Prints when the first frame was by the wall clock, going by the frame at hand, every
/// `every` frames. For a replay, which can run for hours: the picture's timestamps slowly
/// drift from the wall clock the sound is placed by.
fn origin_time(every: u32) -> String {
    format!(r"setpts='PTS+0*if(eq(mod(N\,{every})\,0)\,print(RTCTIME-(T-STARTT)*1000000)\,0)'")
}

/// Reads ffmpeg's log on its own thread: gives the wall-clock times printed to `on_time`, and
/// the first of them to the receiver returned too, once it's really going. The first frame
/// is printed before the encoder is opened, so that alone doesn't say it can encode: only
/// its output being opened does. The thread ends with the errors it saw.
fn read_log(
    child: &mut FfmpegChild,
    on_time: impl Fn(f64) + Send + 'static,
) -> Result<(thread::JoinHandle<Vec<String>>, mpsc::Receiver<f64>), String> {
    const ERRORS_KEPT: usize = 64;
    let events = child.iter().map_err(|e| e.to_string())?;
    let (first_tx, first_rx) = mpsc::channel();
    let log = thread::spawn(move || {
        let mut errors = Vec::new();
        let mut first_time = None;
        let mut output_open = false;
        let mut first = Some(first_tx);
        for event in events {
            match event {
                FfmpegEvent::Log(level, line) => match wall_clock(&line) {
                    Some(time) => {
                        on_time(time);
                        first_time.get_or_insert(time);
                    }
                    None if matches!(level, LogLevel::Error | LogLevel::Fatal) => errors.push(line),
                    None => {}
                },
                FfmpegEvent::ParsedOutput(_) => output_open = true,
                FfmpegEvent::Error(e) => errors.push(e),
                _ => {}
            }
            if output_open
                && let Some(time) = first_time
                && let Some(tx) = first.take()
            {
                let _ = tx.send(time);
            }
            // A replay's ffmpeg may be at it for hours.
            if errors.len() > ERRORS_KEPT {
                errors.drain(..ERRORS_KEPT / 2);
            }
        }
        errors
    });
    Ok((log, first_rx))
}

/// Waits for the first frame; without one in good time, ffmpeg has given up (its log thread
/// ended) or never got going, so it's ended, and the error says what it said.
fn wait_for_first_frame(
    child: &mut FfmpegChild,
    log: thread::JoinHandle<Vec<String>>,
    first: &mpsc::Receiver<f64>,
) -> Result<(f64, thread::JoinHandle<Vec<String>>), String> {
    match first.recv_timeout(Duration::from_secs(10)) {
        Ok(time) => Ok((time, log)),
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            let errors = log.join().unwrap_or_default();
            Err(format!("Couldn't record the screen:\n{}", last_lines(&errors, 8)))
        }
    }
}

/// ffmpeg recording a screen into a fragmented MP4, which stays playable even if relight
/// stops without finishing it.
pub struct Capture {
    child: FfmpegChild,
    log: thread::JoinHandle<Vec<String>>,
    /// When the first frame was taken, in microseconds since 1970.
    pub first_frame: f64,
}

impl Capture {
    /// Starts recording screen `screen`; returns once the first frame is in.
    pub fn start(screen: u32, fps: u32, out: &Path) -> Result<Self, String> {
        // NVENC encodes on an NVIDIA card's own encoder; without one, the frames come off
        // the GPU for x264.
        Self::start_with(screen, fps, out, true).or_else(|_| Self::start_with(screen, fps, out, false))
    }

    fn start_with(screen: u32, fps: u32, out: &Path, nvenc: bool) -> Result<Self, String> {
        let grab = format!("ddagrab=output_idx={screen}:framerate={fps}:draw_mouse=1");
        // A keyframe every two seconds keeps seeking quick in the editor.
        let keyframes = (fps * 2).to_string();
        let (graph, codec) = if nvenc {
            (format!("{grab},{FIRST_FRAME_TIME}"), args![
                "-c:v", "h264_nvenc", "-preset", "p4", "-tune", "hq",
                "-rc", "vbr", "-cq", "19", "-b:v", "0", "-g", &keyframes,
            ])
        } else {
            (format!("{grab},hwdownload,format=bgra,{FIRST_FRAME_TIME}"), args![
                "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-g", &keyframes,
            ])
        };
        // ddagrab stamps each frame with when it took it, which wobbles; a steady frame
        // rate (a late frame repeating the one before) is what editing wants. Fragmented
        // MP4 keeps exact timestamps and, written as it goes, survives relight stopping.
        let mut child = command()
            .hide_banner()
            .overwrite()
            .args(args!["-f", "lavfi", "-i", graph])
            .args(args!["-fps_mode", "cfr", "-r", fps.to_string()])
            .args(codec)
            .args(args!["-f", "mp4", "-movflags", "+frag_keyframe+empty_moov+default_base_moof", out])
            .spawn()
            .map_err(start_failed)?;
        let (log, first) = read_log(&mut child, |_| {})?;
        let (first_frame, log) = wait_for_first_frame(&mut child, log, &first)?;
        Ok(Self { child, log, first_frame })
    }

    /// Asks ffmpeg to finish the file and waits for it, giving up on it after a while.
    pub fn stop(mut self) -> Result<(), String> {
        let _ = self.child.quit();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match self.child.as_inner_mut().try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
                _ => {
                    let _ = self.child.kill();
                    break;
                }
            }
        }
        let status = self.child.wait().map_err(|e| e.to_string())?;
        let errors = self.log.join().unwrap_or_default();
        if status.success() {
            return Ok(());
        }
        Err(format!("The screen recording didn't finish cleanly:\n{}", last_lines(&errors, 8)))
    }
}

/// Where a replay's picture comes from.
#[derive(Debug, Clone, Copy)]
pub enum Source {
    /// A screen, by its DXGI output number.
    Screen(u32),
    /// A test pattern in real time, lasting this many seconds.
    #[cfg(test)]
    Pattern { lasting: f64 },
    /// A black picture in real time, flashing white for a moment 0.7 s into every 3 s: not
    /// in step with the chunks, so being a chunk out shows.
    #[cfg(test)]
    Flashes,
    /// A test pattern on a PC where the NVIDIA encoder won't open.
    #[cfg(test)]
    NoNvidia,
}

/// The most a replay's picture uses, in bits a second and the window that's held to over.
/// Without a limit, what it holds in memory would have no limit either.
const STREAM_MAX_RATE: &str = "50M";
const STREAM_RATE_WINDOW: &str = "100M";

/// ffmpeg capturing a picture and sending it down a pipe, as a replay holds it: encoded
/// more cheaply than a recording as it's always going, and with no B-frames, so a chunk's
/// timestamp is when its first frame was.
pub struct Stream {
    child: FfmpegChild,
    log: thread::JoinHandle<Vec<String>>,
}

impl Stream {
    /// Starts it on NVENC or x264; returns once the first frame is in, with the stream to
    /// read. What ffmpeg prints about the time goes into `clock`.
    pub fn start(source: Source, fps: u32, nvenc: bool, clock: Arc<Clock>) -> Result<(Self, ChildStdout), String> {
        let keyframes = (fps * KEYFRAME_SECONDS).to_string();
        let origin = origin_time(fps * KEYFRAME_SECONDS);
        let picture = match source {
            Source::Screen(screen) if nvenc => format!("ddagrab=output_idx={screen}:framerate={fps}:draw_mouse=1,{origin}"),
            Source::Screen(screen) => {
                format!("ddagrab=output_idx={screen}:framerate={fps}:draw_mouse=1,hwdownload,format=bgra,{origin}")
            }
            #[cfg(test)]
            Source::Pattern { lasting } => format!("testsrc2=size=320x180:rate={fps}:duration={lasting},realtime,{origin}"),
            #[cfg(test)]
            Source::Flashes => format!(
                r"color=black:size=320x180:rate={fps}:duration=3600,drawbox=t=fill:c=white:enable='between(mod(t\,3)\,0.7\,0.9)',realtime,{origin}"
            ),
            #[cfg(test)]
            Source::NoNvidia => format!("testsrc2=size=320x180:rate={fps}:duration=3600,realtime,{origin}"),
        };
        let codec = if nvenc {
            args!["-c:v", "h264_nvenc", "-preset", "p2", "-tune", "ll", "-rc", "vbr", "-cq", "19", "-b:v", "0", "-bf", "0"]
        } else {
            args!["-c:v", "libx264", "-preset", "superfast", "-tune", "zerolatency", "-crf", "20", "-pix_fmt", "yuv420p"]
        };
        // An encoder that takes the first frame and then fails to open, as NVENC does without a card for it.
        #[cfg(test)]
        let codec = if nvenc && matches!(source, Source::NoNvidia) { args!["-c:v", "libx264", "-preset", "nonsense"] } else { codec };
        let mut child = command()
            .hide_banner()
            .overwrite()
            .args(args!["-f", "lavfi", "-i", picture])
            .args(args!["-fps_mode", "cfr", "-r", fps.to_string()])
            .args(codec)
            .args(args!["-maxrate", STREAM_MAX_RATE, "-bufsize", STREAM_RATE_WINDOW, "-g", keyframes])
            .args(args!["-f", "mp4", "-movflags", "+frag_keyframe+empty_moov+default_base_moof", "pipe:1"])
            .spawn()
            .map_err(start_failed)?;
        // Taken before the log's read, which would otherwise take it as a stream of its own.
        let stdout = child.take_stdout().ok_or("Couldn't read what ffmpeg captured.")?;
        let (log, first) = read_log(&mut child, move |time| clock.add(time))?;
        let (_, log) = wait_for_first_frame(&mut child, log, &first)?;
        Ok((Self { child, log }, stdout))
    }

    /// Ends it at once: nothing's being written that wants finishing.
    pub fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.log.join();
    }
}

/// The time `FIRST_FRAME_TIME` printed, if this log line is it.
fn wall_clock(line: &str) -> Option<f64> {
    let time: f64 = line.split_whitespace().last()?.parse().ok()?;
    // Microseconds since 1970: 16 digits for the next couple of centuries.
    (1e15..1e17).contains(&time).then_some(time)
}
