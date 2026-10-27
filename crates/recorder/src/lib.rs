//! Recording the screen and its sound into an MP4.
//!
//! The picture comes from ffmpeg's ddagrab (the GPU's own copy of the screen) encoded by
//! NVENC, so recording costs a game very little. The sound is captured here: what an
//! output device plays, and a microphone. Each is written against the wall clock from the
//! moment recording starts, and ffmpeg reports the wall-clock time of its first frame, so
//! when it stops the sound is trimmed to start with the picture.

mod save;
mod screen;
mod sound;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use relight_ffmpeg::{Partial, Scratch};
use serde::Serialize;

pub use screen::Screen;
pub use sound::Devices;
use screen::Capture;
use sound::{Kind, Track};

/// What to record.
#[derive(Debug, Clone)]
pub struct Options {
    /// The screen, by its DXGI output number; none for the one the mouse is on.
    pub screen: Option<u32>,
    pub fps: u32,
    pub game_sound: bool,
    /// The output device whose sound is recorded; none for the default one.
    pub game_device: Option<String>,
    pub microphone: bool,
    /// The microphone recorded; none for the default one.
    pub mic_device: Option<String>,
}

/// A finished recording.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recorded {
    pub path: PathBuf,
    /// Things that didn't go as asked, the recording going ahead without them.
    pub warnings: Vec<String>,
}

/// A recording going on, on its own thread.
pub struct Recording {
    started: SystemTime,
    stop: mpsc::Sender<()>,
    finished: mpsc::Receiver<Result<Recorded, String>>,
}

impl Recording {
    /// Starts recording into `output`; returns once the first frame is in.
    pub fn start(options: Options, output: PathBuf) -> Result<Self, String> {
        let (stop_tx, stop_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let started = SystemTime::now();
        // The sound streams can't move between threads, so one thread has the recording
        // from start to finish.
        thread::spawn(move || {
            let result = record(&options, started, &output, &stop_rx, &ready_tx);
            let _ = done_tx.send(result);
        });
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { started, stop: stop_tx, finished: done_rx }),
            Ok(Err(e)) => Err(e),
            // It failed before it could say: its result says why.
            Err(_) => Err(done_rx.recv().ok().and_then(Result::err).unwrap_or_else(|| "The recording didn't start.".into())),
        }
    }

    /// When it was asked to start.
    pub fn started(&self) -> SystemTime {
        self.started
    }

    /// Stops it, and waits for it to be saved.
    pub fn stop(self) -> Result<Recorded, String> {
        let _ = self.stop.send(());
        self.finished.recv().unwrap_or_else(|_| Err("The recording stopped unexpectedly.".into()))
    }
}

/// What there is to record.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sources {
    pub screens: Vec<Screen>,
    pub sound: Devices,
}

pub fn sources() -> Sources {
    Sources { screens: screen::screens(), sound: sound::devices() }
}

/// One recording, start to finish: says on `ready` once it's going (or why it isn't),
/// then records until told to `stop`.
fn record(
    options: &Options,
    started: SystemTime,
    output: &Path,
    stop: &mpsc::Receiver<()>,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<Recorded, String> {
    let scratch = Scratch::new()?;
    let picture = scratch.path().join("screen.mp4");
    let mut warnings = Vec::new();

    let mut setup = || -> Result<(Vec<Track>, Capture), String> {
        // The sound first, so none is missed while the screen capture gets going. A source
        // that can't be had is left out rather than costing the recording.
        let mut tracks = Vec::new();
        let sources = [
            (options.game_sound, "Game", Kind::Plays, options.game_device.as_deref()),
            (options.microphone, "Microphone", Kind::Hears, options.mic_device.as_deref()),
        ];
        for (wanted, title, kind, device) in sources {
            if wanted {
                match Track::start(title, kind, device, scratch.path(), started) {
                    Ok((track, warning)) => {
                        tracks.push(track);
                        warnings.extend(warning);
                    }
                    Err(e) => warnings.push(e),
                }
            }
        }
        let screen = pick_screen(options.screen, &mut warnings);
        Ok((tracks, Capture::start(screen, options.fps, &picture)?))
    };
    let (tracks, capture) = match setup() {
        Ok(going) => going,
        Err(e) => {
            let _ = ready.send(Err(e.clone()));
            return Err(e);
        }
    };
    let _ = ready.send(Ok(()));

    let _ = stop.recv();
    let first_frame = capture.first_frame;
    // The picture is written in fragments, so even if ffmpeg doesn't finish cleanly it's
    // good up to where it stopped: it's put together with the sound all the same.
    let capture_problem = capture.stop().err();
    let stopped = SystemTime::now();
    let mut sound = Vec::new();
    for track in tracks {
        match track.finish(stopped) {
            Ok(recorded) => sound.push(recorded),
            Err(e) => warnings.push(e),
        }
    }

    let started_at = started.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();
    let lead = (first_frame / 1e6 - started_at).max(0.0);
    let partial = Partial::new(output);
    let saved = save::combine(&picture, lead, &sound, partial.path()).and_then(|()| partial.finish());
    match saved {
        Ok(()) => {
            warnings.extend(capture_problem);
            Ok(Recorded { path: output.to_owned(), warnings })
        }
        Err(e) => {
            let why = capture_problem.map_or(e.clone(), |problem| format!("{problem}\n{e}"));
            Err(save::keep_pieces(scratch.path(), output, &why))
        }
    }
}

/// The screen asked for if it's there, else the one the mouse is on.
fn pick_screen(wanted: Option<u32>, warnings: &mut Vec<String>) -> u32 {
    let screens = screen::screens();
    if let Some(wanted) = wanted {
        if screens.iter().any(|screen| screen.index == wanted) {
            return wanted;
        }
        warnings.push(format!("Screen {} isn't connected, so the one the mouse is on was recorded.", wanted + 1));
    }
    screen::under_mouse(&screens).unwrap_or(0)
}
