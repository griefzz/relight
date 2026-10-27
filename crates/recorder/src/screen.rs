//! The picture side of a recording: which screens there are, and ffmpeg capturing one.

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use ffmpeg_sidecar::child::FfmpegChild;
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::{FfmpegEvent, LogLevel};
use serde::Serialize;

use relight_ffmpeg::{args, last_lines, start_failed};

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
        let mut child = FfmpegCommand::new()
            .hide_banner()
            .overwrite()
            .args(args!["-f", "lavfi", "-i", graph])
            .args(args!["-fps_mode", "cfr", "-r", fps.to_string()])
            .args(codec)
            .args(args!["-f", "mp4", "-movflags", "+frag_keyframe+empty_moov+default_base_moof", out])
            .spawn()
            .map_err(start_failed)?;
        let events = child.iter().map_err(|e| e.to_string())?;

        let (first_tx, first_rx) = mpsc::channel();
        let log = thread::spawn(move || {
            let mut errors = Vec::new();
            let mut first = Some(first_tx);
            for event in events {
                match event {
                    FfmpegEvent::Log(level, line) => match wall_clock(&line) {
                        Some(time) => {
                            if let Some(tx) = first.take() {
                                let _ = tx.send(time);
                            }
                        }
                        None if matches!(level, LogLevel::Error | LogLevel::Fatal) => errors.push(line),
                        None => {}
                    },
                    FfmpegEvent::Error(e) => errors.push(e),
                    _ => {}
                }
            }
            errors
        });

        // No first frame means ffmpeg gave up (its log thread ended) or never got going.
        match first_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(first_frame) => Ok(Self { child, log, first_frame }),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let errors = log.join().unwrap_or_default();
                Err(format!("Couldn't record the screen:\n{}", last_lines(&errors, 8)))
            }
        }
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

/// The time `FIRST_FRAME_TIME` printed, if this log line is it.
fn wall_clock(line: &str) -> Option<f64> {
    let time: f64 = line.split_whitespace().last()?.parse().ok()?;
    // Microseconds since 1970: 16 digits for the next couple of centuries.
    (1e15..1e17).contains(&time).then_some(time)
}
