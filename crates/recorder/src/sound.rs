//! The sound side of a recording: what an output device plays, and a microphone, each
//! written to a file in step with the wall clock. A replay's sound is placed the same way,
//! but only its newest stretch is held, in memory.
//!
//! The files are raw 32-bit float samples: nothing to finish off at the end, and no size
//! limit (a WAV file stops at 4 GB, under an hour of 7.1).

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// What an output device plays (WASAPI loopback).
    Plays,
    /// What a microphone hears.
    Hears,
}

impl Kind {
    fn what(self) -> &'static str {
        match self {
            Kind::Plays => "sound output",
            Kind::Hears => "microphone",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Devices {
    pub outputs: Vec<String>,
    pub inputs: Vec<String>,
    pub default_output: Option<String>,
    pub default_input: Option<String>,
}

pub fn devices() -> Devices {
    let host = cpal::default_host();
    let names = |devices: Option<Vec<cpal::Device>>| devices.unwrap_or_default().iter().map(|d| d.to_string()).collect();
    let outputs: Vec<String> = names(host.output_devices().ok().map(Iterator::collect));
    // Output devices can be recorded from too (that's loopback), so they're left out
    // of the microphones.
    let inputs = names(host.input_devices().ok().map(Iterator::collect))
        .into_iter()
        .filter(|name: &String| !outputs.contains(name))
        .collect();
    Devices {
        outputs,
        inputs,
        default_output: host.default_output_device().map(|d| d.to_string()),
        default_input: host.default_input_device().map(|d| d.to_string()),
    }
}

/// The device called `name`, or the default one when there's no name; when the named
/// one is gone, the default with a warning saying so.
fn find(kind: Kind, name: Option<&str>) -> Result<(cpal::Device, Option<String>), String> {
    let host = cpal::default_host();
    if let Some(name) = name {
        let devices = match kind {
            Kind::Plays => host.output_devices().map(Iterator::collect::<Vec<_>>),
            Kind::Hears => host.input_devices().map(Iterator::collect::<Vec<_>>),
        };
        if let Some(device) = devices.ok().into_iter().flatten().find(|device| device.to_string() == name) {
            return Ok((device, None));
        }
    }
    let default = match kind {
        Kind::Plays => host.default_output_device(),
        Kind::Hears => host.default_input_device(),
    }
    .ok_or_else(|| format!("There's no {} to record.", kind.what()))?;
    let warning = name.map(|name| format!("{name} isn't connected, so {default} was recorded instead."));
    Ok((default, warning))
}

/// What the capture sends the thread writing the file.
enum Message {
    /// Interleaved samples that finished arriving at that time.
    Sound(Vec<f32>, SystemTime),
    /// Recording stopped then: fill out to it with silence and finish.
    Stop(SystemTime),
}

/// A device's sound being captured and placed in `W`. The capture's callback only hands the
/// samples on, so a slow disk can't make it miss any.
struct Capturing {
    stream: cpal::Stream,
    messages: mpsc::Sender<Message>,
    writer: thread::JoinHandle<io::Result<()>>,
}

impl Capturing {
    fn start<W: Write + Send + 'static>(
        device: &cpal::Device,
        config: cpal::SupportedStreamConfig,
        started: SystemTime,
        out: W,
        broken: &Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let (messages, received) = mpsc::channel();
        let mut placer = Placer::new(out, started, config.sample_rate(), config.channels());
        let writer = thread::spawn(move || placer.write(&received));

        let format = config.sample_format();
        let config = config.config();
        let stream = match format {
            SampleFormat::F32 => open::<f32>(device, config, &messages, broken),
            SampleFormat::I16 => open::<i16>(device, config, &messages, broken),
            SampleFormat::I32 => open::<i32>(device, config, &messages, broken),
            SampleFormat::U16 => open::<u16>(device, config, &messages, broken),
            SampleFormat::U8 => open::<u8>(device, config, &messages, broken),
            SampleFormat::F64 => open::<f64>(device, config, &messages, broken),
            other => Err(format!("{device} gives its sound as {other}, which relight can't read.")),
        }?;
        stream.play().map_err(|e| format!("Couldn't start recording {device}: {e}"))?;
        Ok(Self { stream, messages, writer })
    }
}

/// How a device says what it's capable of.
fn config_of(kind: Kind, device: &cpal::Device) -> Result<cpal::SupportedStreamConfig, String> {
    match kind {
        Kind::Plays => device.default_output_config(),
        Kind::Hears => device.default_input_config(),
    }
    .map_err(|e| format!("Couldn't open {device}: {e}"))
}

/// What a track's raw file is called: the name says how to read it, should it need reading by hand.
fn raw_name(title: &str, rate: u32, channels: u16) -> String {
    format!("{} {rate} Hz {channels}ch f32le.raw", title.to_lowercase())
}

/// A sound source being written to a file.
pub struct Track {
    capturing: Capturing,
    recorded: Recorded,
}

impl Track {
    /// Starts recording into a file in `dir`; also a warning if it's not the device asked for.
    pub fn start(
        title: &'static str,
        kind: Kind,
        device: Option<&str>,
        dir: &Path,
        started: SystemTime,
    ) -> Result<(Self, Option<String>), String> {
        let (device, warning) = find(kind, device)?;
        let config = config_of(kind, &device)?;
        let (rate, channels) = (config.sample_rate(), config.channels());
        let path = dir.join(raw_name(title, rate, channels));
        let file = File::create(&path).map_err(|e| format!("Couldn't record {}: {e}", kind.what()))?;
        let out = BufWriter::with_capacity(1 << 16, file);
        // A recording is short enough for someone to notice its sound going, and is left to it.
        let capturing = Capturing::start(&device, config, started, out, &Arc::default())?;
        let recorded = Recorded { title, path, rate, channels };
        Ok((Self { capturing, recorded }, warning))
    }

    /// Stops it, and fills it out with silence to `stopped`, so each track runs the whole
    /// recording.
    pub fn finish(self, stopped: SystemTime) -> Result<Recorded, String> {
        let Capturing { stream, messages, writer } = self.capturing;
        drop(stream);
        let _ = messages.send(Message::Stop(stopped));
        let title = self.recorded.title;
        match writer.join() {
            Ok(Ok(())) => Ok(self.recorded),
            Ok(Err(e)) => Err(format!("Couldn't write the {} sound: {e}", title.to_lowercase())),
            Err(_) => Err(format!("The {} sound stopped unexpectedly.", title.to_lowercase())),
        }
    }
}

/// A finished track's file.
pub struct Recorded {
    pub title: &'static str,
    pub path: PathBuf,
    pub rate: u32,
    pub channels: u16,
}

/// A sound source being held in memory for a replay. Stops when dropped.
pub struct Listening {
    _capturing: Capturing,
    /// Set when the device has gone (or, for the default one, another has taken over), and
    /// nothing more will come.
    broken: Arc<AtomicBool>,
    pub tape: Tape,
}

impl Listening {
    /// Starts holding the newest `keep` seconds; also a warning if it's not the device asked for.
    pub fn start(title: &'static str, kind: Kind, device: Option<&str>, keep: f64) -> Result<(Self, Option<String>), String> {
        let (device, warning) = find(kind, device)?;
        let config = config_of(kind, &device)?;
        let (rate, channels) = (config.sample_rate(), config.channels());
        let started = SystemTime::now();
        let ring = Ring::new((keep * f64::from(rate)) as usize * usize::from(channels) * SAMPLE_BYTES);
        let broken = Arc::default();
        let capturing = Capturing::start(&device, config, started, ring.clone(), &broken)?;
        let tape = Tape { title, rate, channels, started, ring };
        Ok((Self { _capturing: capturing, broken, tape }, warning))
    }

    pub fn is_broken(&self) -> bool {
        self.broken.load(Ordering::Relaxed)
    }
}

const SAMPLE_BYTES: usize = 4;

/// The sound a replay holds of one source: what's needed to save a stretch of it, whatever
/// has become of the source by then.
#[derive(Clone)]
pub struct Tape {
    pub title: &'static str,
    rate: u32,
    channels: u16,
    /// When frame 0 was.
    started: SystemTime,
    ring: Ring,
}

impl Tape {
    /// Writes `length` seconds from `from` (seconds since 1970) to a file in `dir`: exactly
    /// that long, with silence for whatever isn't held.
    pub fn save(&self, from: f64, length: f64, dir: &Path) -> io::Result<Recorded> {
        let rate = f64::from(self.rate);
        let frame_bytes = i64::from(self.channels) * SAMPLE_BYTES as i64;
        let zero = self.started.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();
        let first = ((from - zero) * rate).round() as i64;
        let frames = (length * rate).round() as i64;
        let path = dir.join(raw_name(self.title, self.rate, self.channels));
        fs::write(&path, self.ring.read(first * frame_bytes, (first + frames) * frame_bytes))?;
        Ok(Recorded { title: self.title, path, rate: self.rate, channels: self.channels })
    }
}

#[cfg(test)]
impl Tape {
    /// A tape with nothing on it, standing in for a device in tests.
    pub fn blank(title: &'static str) -> Self {
        Self { title, rate: 48000, channels: 2, started: SystemTime::now(), ring: Ring::new(1 << 16) }
    }

    /// A tape running from `zero` (seconds since 1970) for `length` seconds, with a click
    /// `first` seconds in and then every `every` seconds.
    pub fn of_clicks(title: &'static str, zero: f64, first: f64, every: f64, length: f64) -> Self {
        const RATE: u32 = 48000;
        let mut ring = Ring::new((length * f64::from(RATE)) as usize * SAMPLE_BYTES);
        for frame in 0..(length * f64::from(RATE)) as u32 {
            let time = f64::from(frame) / f64::from(RATE);
            let clicking = time >= first && (time - first) % every < 0.02;
            ring.write_all(&(if clicking { 0.8f32 } else { 0.0 }).to_le_bytes()).unwrap();
        }
        let started = UNIX_EPOCH + std::time::Duration::from_secs_f64(zero);
        Self { title, rate: RATE, channels: 1, started, ring }
    }
}

/// The newest bytes written to it, up to a limit.
#[derive(Clone)]
struct Ring(Arc<Mutex<RingBytes>>);

struct RingBytes {
    bytes: VecDeque<u8>,
    capacity: usize,
    /// Bytes ever written, so where each one falls is known once older ones have gone.
    written: u64,
}

impl Ring {
    fn new(capacity: usize) -> Self {
        Self(Arc::new(Mutex::new(RingBytes { bytes: VecDeque::with_capacity(capacity), capacity, written: 0 })))
    }

    /// Bytes `from` to `to`, counting from the first ever written: zeros for those not held,
    /// whether long gone or not written yet.
    fn read(&self, from: i64, to: i64) -> Vec<u8> {
        let mut out = vec![0; (to - from).max(0) as usize];
        let ring = self.0.lock().unwrap();
        let newest = ring.written as i64;
        let oldest = newest - ring.bytes.len() as i64;
        let (start, end) = (from.max(oldest), to.min(newest));
        if start < end {
            let held = ring.bytes.range((start - oldest) as usize..(end - oldest) as usize);
            for (byte, &value) in out[(start - from) as usize..].iter_mut().zip(held) {
                *byte = value;
            }
        }
        out
    }
}

impl Write for Ring {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut ring = self.0.lock().unwrap();
        ring.bytes.extend(data);
        ring.written += data.len() as u64;
        let over = ring.bytes.len().saturating_sub(ring.capacity);
        ring.bytes.drain(..over);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn open<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    messages: &mpsc::Sender<Message>,
    broken: &Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let messages = messages.clone();
    let broken = broken.clone();
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                let samples = data.iter().map(|&sample| sample.to_sample::<f32>()).collect();
                let _ = messages.send(Message::Sound(samples, SystemTime::now()));
            },
            move |e| {
                eprintln!("Sound capture: {e}");
                // A glitch is a glitch; anything else, and no more is coming.
                if !matches!(e.kind(), ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged) {
                    broken.store(true, Ordering::Relaxed);
                }
            },
            None,
        )
        .map_err(|e| format!("Couldn't record {device}: {e}"))
}

/// Writes the samples where they belong in time.
struct Placer<W> {
    out: W,
    /// When the recording started: frame 0 of what's written.
    started: SystemTime,
    rate: u32,
    channels: u16,
    /// Frames written so far.
    frames: u64,
    bytes: Vec<u8>,
}

impl<W: Write> Placer<W> {
    fn new(out: W, started: SystemTime, rate: u32, channels: u16) -> Self {
        Self { out, started, rate, channels, frames: 0, bytes: Vec::new() }
    }

    /// Writes what arrives until told to stop. After a write fails it keeps taking what
    /// arrives, so it doesn't pile up, and says so at the end.
    fn write(&mut self, messages: &mpsc::Receiver<Message>) -> io::Result<()> {
        let mut failed = None;
        for message in messages {
            let wrote = match message {
                Message::Sound(samples, at) if failed.is_none() => self.push(&samples, at),
                Message::Sound(..) => Ok(()),
                Message::Stop(at) => {
                    if failed.is_none() {
                        failed = self.pad_to(at).and_then(|()| self.out.flush()).err();
                    }
                    break;
                }
            };
            if let Err(e) = wrote {
                failed = Some(e);
            }
        }
        failed.map_or(Ok(()), Err)
    }

    /// Adds a buffer of interleaved samples that finished arriving at `now`.
    ///
    /// Windows sends nothing from an output device while it's silent, and a sound card's
    /// clock drifts from the wall clock, so each buffer is placed by when it came: a gap
    /// before it is filled with silence, and a lead over the wall clock is trimmed off.
    fn push(&mut self, samples: &[f32], now: SystemTime) -> io::Result<()> {
        let channels = usize::from(self.channels);
        let frames = (samples.len() / channels) as i64;
        let rate = i64::from(self.rate);
        let since = now.duration_since(self.started).unwrap_or_default().as_secs_f64();
        let belongs_at = (since * self.rate as f64) as i64 - frames;
        let behind = belongs_at - self.frames as i64;
        let mut skip = 0;
        if behind > rate / 50 {
            self.silence(behind as u64)?;
        } else if behind < -(rate / 25) {
            skip = (-behind).min(frames) as usize;
        }
        self.bytes.clear();
        self.bytes.extend(samples[skip * channels..frames as usize * channels].iter().flat_map(|s| s.to_le_bytes()));
        self.out.write_all(&self.bytes)?;
        self.frames += frames as u64 - skip as u64;
        Ok(())
    }

    fn silence(&mut self, frames: u64) -> io::Result<()> {
        const ZEROS: [u8; 1 << 14] = [0; 1 << 14];
        let mut left = frames * u64::from(self.channels) * 4;
        while left > 0 {
            let n = left.min(ZEROS.len() as u64) as usize;
            self.out.write_all(&ZEROS[..n])?;
            left -= n as u64;
        }
        self.frames += frames;
        Ok(())
    }

    fn pad_to(&mut self, until: SystemTime) -> io::Result<()> {
        let want = (until.duration_since(self.started).unwrap_or_default().as_secs_f64() * self.rate as f64) as u64;
        if want > self.frames {
            self.silence(want - self.frames)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use super::*;

    fn samples_in(path: &Path) -> Vec<f32> {
        fs::read(path).unwrap().as_chunks::<4>().0.iter().map(|&b| f32::from_le_bytes(b)).collect()
    }

    #[test]
    fn sound_is_placed_by_when_it_arrived() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("track.raw");
        let started = SystemTime::now();
        let at = |ms| started + Duration::from_millis(ms);
        // 100 Hz mono: a frame every 10 ms.
        let mut placer = Placer::new(BufWriter::new(File::create(&path).unwrap()), started, 100, 1);
        let (tx, rx) = mpsc::channel();
        // Ten frames arriving 100 ms in, as they should; then nothing for a second (an
        // output device going quiet); then ten more; then it stops at 1.5 s.
        tx.send(Message::Sound(vec![1.0; 10], at(100))).unwrap();
        tx.send(Message::Sound(vec![2.0; 10], at(1200))).unwrap();
        tx.send(Message::Stop(at(1500))).unwrap();
        placer.write(&rx).unwrap();

        let samples = samples_in(&path);
        assert_eq!(samples.len(), 150);
        assert!(samples[..10].iter().all(|&s| s == 1.0));
        assert!(samples[10..110].iter().all(|&s| s == 0.0), "the quiet second is silence");
        assert!(samples[110..120].iter().all(|&s| s == 2.0));
        assert!(samples[120..].iter().all(|&s| s == 0.0), "filled out to the stop");
    }

    #[test]
    fn a_ring_keeps_the_newest_and_knows_where_they_fall() {
        let mut ring = Ring::new(4);
        ring.write_all(&[1, 2, 3]).unwrap();
        assert_eq!(ring.read(0, 3), [1, 2, 3]);
        ring.write_all(&[4, 5, 6]).unwrap();
        // 1 and 2 have gone: the same positions still mean the same bytes.
        assert_eq!(ring.read(2, 6), [3, 4, 5, 6]);
        assert_eq!(ring.read(0, 3), [0, 0, 3], "what's gone reads as silence");
        assert_eq!(ring.read(5, 8), [6, 0, 0], "and so does what hasn't come");
        assert_eq!(ring.read(-2, 0), [0, 0], "and what came before anything did");
        assert_eq!(ring.read(9, 11), [0, 0]);
        assert!(ring.read(4, 4).is_empty());
    }

    #[test]
    fn a_tape_saves_exactly_the_stretch_asked_for_in_step_with_the_clock() {
        let dir = tempfile::tempdir().unwrap();
        let started = UNIX_EPOCH + Duration::from_secs(1000);
        let at = |ms| started + Duration::from_millis(ms);
        // 100 Hz stereo, holding 2 seconds. Each frame's left channel says which it is.
        let ring = Ring::new(200 * 2 * SAMPLE_BYTES);
        let mut placer = Placer::new(ring.clone(), started, 100, 2);
        let (tx, rx) = mpsc::channel();
        for second in 0..3u64 {
            let samples = (0..100).flat_map(|i| [(second * 100 + i) as f32, 0.0]).collect();
            tx.send(Message::Sound(samples, at((second + 1) * 1000))).unwrap();
        }
        tx.send(Message::Stop(at(3000))).unwrap();
        placer.write(&rx).unwrap();
        let tape = Tape { title: "Game", rate: 100, channels: 2, started, ring };

        let saved = tape.save(1001.5, 0.5, dir.path()).unwrap();
        assert_eq!(saved.path, dir.path().join("game 100 Hz 2ch f32le.raw"));
        let lefts: Vec<f32> = samples_in(&saved.path).chunks(2).map(|frame| frame[0]).collect();
        assert_eq!(lefts, (150..200).map(|i| i as f32).collect::<Vec<_>>());

        // Reaching back past what's held (the first second has gone) and on past the end,
        // it's still the length asked for, with silence at either end.
        let saved = tape.save(1000.5, 3.0, dir.path()).unwrap();
        let lefts: Vec<f32> = samples_in(&saved.path).chunks(2).map(|frame| frame[0]).collect();
        assert_eq!(lefts.len(), 300);
        assert!(lefts[..50].iter().all(|&s| s == 0.0));
        assert_eq!(lefts[50], 100.0);
        assert_eq!(lefts[249], 299.0);
        assert!(lefts[250..].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn silence_is_placed_in_a_ring_as_it_is_in_a_file() {
        let started = UNIX_EPOCH + Duration::from_secs(1000);
        let at = |ms| started + Duration::from_millis(ms);
        let ring = Ring::new(1000 * SAMPLE_BYTES);
        let mut placer = Placer::new(ring.clone(), started, 100, 1);
        let (tx, rx) = mpsc::channel();
        tx.send(Message::Sound(vec![1.0; 10], at(100))).unwrap();
        tx.send(Message::Sound(vec![2.0; 10], at(1200))).unwrap();
        tx.send(Message::Stop(at(1200))).unwrap();
        placer.write(&rx).unwrap();
        let bytes = ring.read(0, 120 * SAMPLE_BYTES as i64);
        let samples: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|&b| f32::from_le_bytes(b)).collect();
        assert!(samples[..10].iter().all(|&s| s == 1.0));
        assert!(samples[10..110].iter().all(|&s| s == 0.0));
        assert!(samples[110..].iter().all(|&s| s == 2.0));
    }
}
