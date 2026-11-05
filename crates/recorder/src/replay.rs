//! Instant replay: the screen and its sound captured all along but only held in memory, the
//! oldest dropping away, until the last stretch of it is asked for and saved.
//!
//! Nothing here touches the disk until then, and the sound isn't encoded until then either.
//! Left going for hours, it has to get through what a short recording never meets: a game
//! changing the display mode, the screen locking, the PC sleeping, a headset unplugged. When
//! the capture ends it's started again, and what was held goes with it (it might not even be
//! the same size); when a sound device goes, the next one takes over.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use relight_ffmpeg::{Job, Partial, Scratch};

use crate::chunks::{self, Clock, KEYFRAME_SECONDS, Picture};
use crate::screen::{Source, Stream};
use crate::sound::{Kind, Listening, Tape};
use crate::{Options, Recorded, pick_screen, save};

/// How often the capture and sound are checked on.
const CHECK_EVERY: Duration = Duration::from_millis(500);
/// A chunk comes every couple of seconds: this long without one, and ffmpeg has hung.
const LONGEST_QUIET: Duration = Duration::from_secs(15);
/// A jump this big between checks means the PC slept or the clock was changed. Frames kept
/// to a steady rate would have that whole gap to fill, so the capture starts again.
const LARGEST_JUMP: Duration = Duration::from_secs(5);
/// How long to wait before trying the capture again, doubling after each failure up to the
/// most. The usual reasons (a lock screen, a display mode changing) sort themselves out.
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LONGEST_RETRY: Duration = Duration::from_secs(10);
/// How often to try for a sound device that's gone.
const SOUND_RETRY: Duration = Duration::from_secs(5);
/// How long a save waits for the chunk still being written.
const SAVE_PATIENCE_SECONDS: u32 = KEYFRAME_SECONDS + 3;
/// How much more sound than asked for is held. A clip reaches back to a keyframe, and its
/// sound can only be read once the save's done waiting, by when more has come in behind it.
const SOUND_EXTRA_SECONDS: u32 = KEYFRAME_SECONDS + SAVE_PATIENCE_SECONDS + 1;

/// A replay going on, on its own thread.
pub struct Replay {
    stop: mpsc::Sender<()>,
    owner: thread::JoinHandle<()>,
    held: Arc<Held>,
}

impl Replay {
    /// Starts it, holding the last `seconds`; returns once the first frame is in, with things
    /// that didn't go as asked. `changed` is called when `is_interrupted` changes.
    pub fn start(options: Options, seconds: u32, changed: impl Fn() + Send + 'static) -> Result<(Self, Vec<String>), String> {
        let mut warnings = Vec::new();
        let screen = pick_screen(options.screen, &mut warnings);
        let (replay, more) = Self::start_from(Source::Screen(screen), options, seconds, changed)?;
        warnings.extend(more);
        Ok((replay, warnings))
    }

    fn start_from(
        source: Source,
        options: Options,
        seconds: u32,
        changed: impl Fn() + Send + 'static,
    ) -> Result<(Self, Vec<String>), String> {
        let held = Arc::new(Held {
            seconds: f64::from(seconds),
            picture: Picture::new(f64::from(seconds)),
            tapes: Mutex::default(),
            interrupted: AtomicBool::new(false),
        });
        let (stop, stopped) = mpsc::channel();
        let (ready_tx, ready) = mpsc::channel();
        let its_held = held.clone();
        // The sound streams can't move between threads, so one thread has them throughout.
        let owner = thread::spawn(move || keep_going(source, &options, &its_held, &stopped, &ready_tx, &changed));
        match ready.recv() {
            Ok(Ok(warnings)) => Ok((Self { stop, owner, held }, warnings)),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("Instant replay didn't start.".into()),
        }
    }

    /// What saves from this replay, and carries on being able to whatever becomes of it.
    pub fn saver(&self) -> Saver {
        Saver(self.held.clone())
    }

    /// Whether the capture has ended and is waiting to start again, so there's nothing to save.
    pub fn is_interrupted(&self) -> bool {
        self.held.interrupted.load(Ordering::Relaxed)
    }

    /// Stops it, and lets go of what it held (once any save going on has finished with it).
    pub fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.owner.join();
    }
}

/// What's held, shared with whoever's saving.
struct Held {
    seconds: f64,
    picture: Picture,
    tapes: Mutex<Vec<Tape>>,
    interrupted: AtomicBool,
}

/// Saves a replay's last stretch.
#[derive(Clone)]
pub struct Saver(Arc<Held>);

impl Saver {
    /// Saves the time held up to `pressed` as an MP4, like a recording's. The newest frames
    /// are still being encoded, so it takes a couple of seconds to begin.
    pub fn save(&self, pressed: SystemTime, output: &Path) -> Result<Recorded, String> {
        let held = &self.0;
        let pressed = pressed.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();
        let patience = Duration::from_secs(u64::from(SAVE_PATIENCE_SECONDS));
        let clip = held
            .picture
            .clip(pressed, held.seconds, patience)
            .filter(|clip| clip.length > 0.0)
            .ok_or("There's nothing to save yet: instant replay has only just started, or the screen capture is restarting.")?;

        // The sound first: it's still moving on, whereas the picture's chunks are kept for
        // as long as the clip is, however long a busy disk takes over them.
        let scratch = Scratch::new()?;
        let tapes = held.tapes.lock().unwrap().clone();
        let mut sound = Vec::new();
        let mut warnings = Vec::new();
        for tape in tapes {
            match tape.save(clip.start, clip.length, scratch.path()) {
                Ok(saved) => sound.push(saved),
                Err(e) => warnings.push(format!("Couldn't save the {} sound: {e}", tape.title.to_lowercase())),
            }
        }
        let picture = scratch.path().join("screen.mp4");
        clip.write(&picture).map_err(|e| format!("Couldn't save the replay: {e}"))?;

        let partial = Partial::new(output);
        let job = Job::silent().in_background();
        match save::combine(&picture, 0.0, clip.length, &sound, partial.path(), &job).and_then(|()| partial.finish()) {
            Ok(()) => Ok(Recorded { path: output.to_owned(), warnings }),
            Err(e) => Err(save::keep_pieces(scratch.path(), output, &e)),
        }
    }
}

/// The replay, start to finish: says on `ready` once it's going (or why it isn't), then keeps
/// it going until told to `stop`.
fn keep_going(
    source: Source,
    options: &Options,
    held: &Arc<Held>,
    stop: &mpsc::Receiver<()>,
    ready: &mpsc::Sender<Result<Vec<String>, String>>,
    changed: &impl Fn(),
) {
    // The sound first, so none is missed while the screen capture gets going. A source that
    // can't be had is left out rather than costing the replay, and is tried for again.
    let mut warnings = Vec::new();
    let keep = held.seconds + f64::from(SOUND_EXTRA_SECONDS);
    let mut sound: Vec<Sound> = [
        (options.game_sound, "Game", Kind::Plays, &options.game_device),
        (options.microphone, "Microphone", Kind::Hears, &options.mic_device),
    ]
    .into_iter()
    .filter(|(wanted, ..)| *wanted)
    .map(|(_, title, kind, device)| Sound::new(title, kind, device.clone(), keep, &mut warnings))
    .collect();
    held.show(&sound);

    // NVENC encodes on an NVIDIA card's own encoder; without one, the frames come off the
    // GPU for x264, which costs a lot more, as it always will while a replay's on.
    let (mut feed, nvenc) = match Feed::start(source, options.fps, true, held) {
        Ok(feed) => (Some(feed), true),
        Err(_) => match Feed::start(source, options.fps, false, held) {
            Ok(feed) => (Some(feed), false),
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        },
    };
    if !nvenc {
        warnings.push("Instant replay is encoding on the CPU, as there's no NVIDIA encoder here. Games may run slower while it's on.".into());
    }
    let _ = ready.send(Ok(warnings));

    let mut last_check = SystemTime::now();
    let mut retry_at = Instant::now();
    let mut retry_wait = FIRST_RETRY;
    while let Err(mpsc::RecvTimeoutError::Timeout) = stop.recv_timeout(CHECK_EVERY) {
        let now = SystemTime::now();
        let jump = now.duration_since(last_check).unwrap_or_else(|e| e.duration());
        last_check = now;

        let failed = feed.as_ref().is_some_and(|feed| feed.has_ended() || held.picture.quiet_for() > LONGEST_QUIET);
        if failed || (feed.is_some() && jump > LARGEST_JUMP) {
            if let Some(feed) = feed.take() {
                feed.stop();
            }
            held.picture.clear();
            held.interrupted.store(true, Ordering::Relaxed);
            changed();
            retry_at = Instant::now();
            retry_wait = FIRST_RETRY;
        }
        if feed.is_none() && Instant::now() >= retry_at {
            if let Ok(started) = Feed::start(source, options.fps, nvenc, held) {
                feed = Some(started);
                held.interrupted.store(false, Ordering::Relaxed);
                changed();
            } else {
                retry_at = Instant::now() + retry_wait;
                retry_wait = (retry_wait * 2).min(LONGEST_RETRY);
            }
        }

        let mut reopened = false;
        for sound in &mut sound {
            reopened |= sound.check();
        }
        if reopened {
            held.show(&sound);
        }
    }

    held.picture.close();
    if let Some(feed) = feed {
        feed.stop();
    }
}

impl Held {
    /// Saves from then on use these sources.
    fn show(&self, sound: &[Sound]) {
        *self.tapes.lock().unwrap() = sound.iter().filter_map(|sound| Some(sound.going.as_ref()?.tape.clone())).collect();
    }
}

/// ffmpeg's stream being read into the picture.
struct Feed {
    stream: Stream,
    reader: thread::JoinHandle<()>,
}

impl Feed {
    fn start(source: Source, fps: u32, nvenc: bool, held: &Arc<Held>) -> Result<Self, String> {
        let clock = Arc::new(Clock::default());
        let (stream, stdout) = Stream::start(source, fps, nvenc, clock.clone())?;
        held.picture.expect_chunks();
        let held = held.clone();
        let reader = thread::spawn(move || {
            let _ = chunks::follow(stdout, fps, &clock, &held.picture);
        });
        Ok(Self { stream, reader })
    }

    fn has_ended(&self) -> bool {
        self.reader.is_finished()
    }

    fn stop(self) {
        self.stream.stop();
        let _ = self.reader.join();
    }
}

/// A sound source to keep going, whatever becomes of its device.
struct Sound {
    title: &'static str,
    kind: Kind,
    device: Option<String>,
    keep: f64,
    going: Option<Listening>,
    next_try: Instant,
}

impl Sound {
    fn new(title: &'static str, kind: Kind, device: Option<String>, keep: f64, warnings: &mut Vec<String>) -> Self {
        let mut sound = Self { title, kind, device, keep, going: None, next_try: Instant::now() + SOUND_RETRY };
        match Listening::start(title, kind, sound.device.as_deref(), keep) {
            Ok((going, warning)) => {
                sound.going = Some(going);
                warnings.extend(warning);
            }
            Err(e) => warnings.push(e),
        }
        sound
    }

    /// If it's stopped, tries to get it going again; true if there's a new one to save from.
    /// The default device is whichever it is by then. What the old one held goes with it, as
    /// the new one may well have a different format.
    fn check(&mut self) -> bool {
        if self.going.as_ref().is_some_and(|going| !going.is_broken()) {
            return false;
        }
        if Instant::now() < self.next_try {
            return false;
        }
        self.next_try = Instant::now() + SOUND_RETRY;
        // Left going until there's another, so a save still has what it held.
        match Listening::start(self.title, self.kind, self.device.as_deref(), self.keep) {
            Ok((going, _)) => {
                self.going = Some(going);
                true
            }
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::sync::atomic::AtomicUsize;

    use super::*;

    fn no_sound() -> Options {
        Options { screen: None, fps: 30, game_sound: false, game_device: None, microphone: false, mic_device: None }
    }

    fn frames_in(path: &Path) -> u32 {
        let out = Command::new("ffprobe")
            .args(["-v", "error", "-count_frames", "-select_streams", "v:0"])
            .args(["-show_entries", "stream=nb_read_frames", "-of", "csv=p=0"])
            .arg(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
    }

    fn stream_length(path: &Path, stream: &str) -> f64 {
        let out = Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", stream])
            .args(["-show_entries", "stream=duration", "-of", "csv=p=0"])
            .arg(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
    }

    /// A file's raw output from ffmpeg, given what to pick and how to turn it into bytes.
    fn decoded(path: &Path, how: &[&str]) -> Vec<u8> {
        let out = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(path).args(how).arg("-").output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        out.stdout
    }

    /// When each flash starts, in seconds, in a video at 30 fps: every frame is shrunk to a
    /// single grey pixel, so each byte is how bright a frame is.
    fn flashes_in(path: &Path) -> Vec<f64> {
        let brightness = decoded(path, &["-map", "0:v", "-vf", "scale=1:1,format=gray", "-f", "rawvideo"]);
        let rises = (1..brightness.len()).filter(|&i| brightness[i] > 128 && brightness[i - 1] <= 128);
        rises.map(|i| i as f64 / 30.0).collect()
    }

    /// When each click starts, in seconds, in the file's first sound track.
    fn clicks_in(path: &Path) -> Vec<f64> {
        const RATE: usize = 8000;
        let bytes = decoded(path, &["-map", "0:a:0", "-ac", "1", "-ar", "8000", "-f", "f32le"]);
        let samples: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|&b| f32::from_le_bytes(b)).collect();
        let mut clicks = Vec::new();
        let mut last = None;
        for (i, sample) in samples.iter().enumerate() {
            if sample.abs() > 0.2 {
                // The same click until it's been quiet for a good while.
                if last.is_none_or(|last| i - last > RATE / 2) {
                    clicks.push(i as f64 / RATE as f64);
                }
                last = Some(i);
            }
        }
        clicks
    }

    #[test]
    #[ignore = "needs ffmpeg"]
    fn the_sound_stays_in_step_with_the_picture() {
        let dir = tempfile::tempdir().unwrap();
        let (replay, _) = Replay::start_from(Source::Flashes, no_sound(), 8, || {}).unwrap();
        // The first chunk is the picture's time zero, which says when that was by the clock.
        thread::sleep(Duration::from_secs(4));
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64();
        let zero = replay.held.picture.clip(now, 1e6, Duration::ZERO).unwrap().start;
        // A click each time the picture flashes, by the same clock the sound's placed by.
        replay.held.tapes.lock().unwrap().push(Tape::of_clicks("Game", zero, 0.7, 3.0, 60.0));
        // Long enough for the start to have been dropped, so a save starts well into the picture.
        thread::sleep(Duration::from_secs(12));

        let output = dir.path().join("replay.mp4");
        replay.saver().save(SystemTime::now(), &output).unwrap();
        replay.stop();

        let (flashes, clicks) = (flashes_in(&output), clicks_in(&output));
        assert!(flashes.len() >= 2, "flashes at {flashes:?}");
        assert_eq!(flashes.len(), clicks.len(), "flashes at {flashes:?}, clicks at {clicks:?}");
        for (flash, click) in flashes.iter().zip(&clicks) {
            // A frame is 33 ms; people notice sound out of step from about twice that.
            assert!((flash - click).abs() < 0.05, "flashes at {flashes:?}, clicks at {clicks:?}");
        }
    }

    #[test]
    #[ignore = "needs ffmpeg"]
    fn saves_the_last_stretch_as_a_video_with_its_sound_the_same_length() {
        let dir = tempfile::tempdir().unwrap();
        let pattern = Source::Pattern { lasting: 3600.0 };
        let (replay, warnings) = Replay::start_from(pattern, no_sound(), 4, || {}).unwrap();
        assert!(warnings.iter().all(|w| w.contains("CPU")), "{warnings:?}");
        replay.held.tapes.lock().unwrap().push(Tape::blank("Game"));

        // Long enough for some to have been dropped.
        thread::sleep(Duration::from_secs(10));
        let output = dir.path().join("replay.mp4");
        let saved = replay.saver().save(SystemTime::now(), &output).unwrap();
        replay.stop();
        assert_eq!(saved.path, output);
        let info = relight_ffmpeg::probe(&output).unwrap();
        // At least the 4 seconds asked for, reaching back no more than a keyframe further.
        assert!((3.9..=6.5).contains(&info.duration), "lasts {} s", info.duration);
        let frames = frames_in(&output);
        assert!((f64::from(frames) / 30.0 - info.duration).abs() < 0.2, "{frames} frames in {} s", info.duration);
        let sound = stream_length(&output, "a:0");
        assert!((sound - info.duration).abs() < 0.1, "{sound} s of sound in {} s", info.duration);
    }

    #[test]
    #[ignore = "needs ffmpeg"]
    fn falls_back_to_x264_when_the_nvidia_encoder_wont_open() {
        let dir = tempfile::tempdir().unwrap();
        let (replay, warnings) = Replay::start_from(Source::NoNvidia, no_sound(), 5, || {}).unwrap();
        assert!(warnings.iter().any(|w| w.contains("CPU")), "{warnings:?}");

        thread::sleep(Duration::from_secs(4));
        assert!(!replay.is_interrupted(), "it keeps going on x264");
        let output = dir.path().join("replay.mp4");
        replay.saver().save(SystemTime::now(), &output).unwrap();
        replay.stop();
        let info = relight_ffmpeg::probe(&output).unwrap();
        assert!(info.duration > 1.0, "lasts {} s", info.duration);
    }

    #[test]
    #[ignore = "needs ffmpeg"]
    fn saving_right_after_starting_saves_what_there_is() {
        let dir = tempfile::tempdir().unwrap();
        let (replay, _) = Replay::start_from(Source::Pattern { lasting: 3600.0 }, no_sound(), 30, || {}).unwrap();
        let output = dir.path().join("replay.mp4");
        // Asked for straight away, it waits for the first chunk to finish.
        let saved = replay.saver().save(SystemTime::now() + Duration::from_secs(1), &output);
        replay.stop();
        let info = relight_ffmpeg::probe(&saved.unwrap().path).unwrap();
        assert!(info.duration > 0.5 && info.duration < 3.0, "lasts {} s", info.duration);
    }

    #[test]
    #[ignore = "needs ffmpeg"]
    fn a_capture_that_ends_is_started_again_with_a_fresh_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let changes = Arc::new(AtomicUsize::new(0));
        let seen = changes.clone();
        // Its ffmpeg finishes after 3 seconds, as one does when the display mode changes.
        let (replay, _) = Replay::start_from(Source::Pattern { lasting: 3.0 }, no_sound(), 30, move || {
            seen.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();

        // Going, ended and going again are each seen by now.
        let deadline = Instant::now() + Duration::from_secs(10);
        while changes.load(Ordering::Relaxed) < 2 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(changes.load(Ordering::Relaxed) >= 2, "said {} times", changes.load(Ordering::Relaxed));
        assert!(!replay.is_interrupted());

        let output = dir.path().join("replay.mp4");
        thread::sleep(Duration::from_millis(2500));
        let saved = replay.saver().save(SystemTime::now(), &output).unwrap();
        replay.stop();
        // Only what's come since it started again, not the 3 seconds before.
        let info = relight_ffmpeg::probe(&saved.path).unwrap();
        assert!(info.duration < 3.0, "lasts {} s", info.duration);
    }
}
