//! The sound side of a recording: what an output device plays, and a microphone, each
//! written to a file in step with the wall clock.
//!
//! The files are raw 32-bit float samples: nothing to finish off at the end, and no size
//! limit (a WAV file stops at 4 GB, under an hour of 7.1).

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::SystemTime;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
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

/// A sound source being written to a file. The capture's callback only hands the samples
/// on, so a slow disk can't make it miss any.
pub struct Track {
    stream: cpal::Stream,
    messages: mpsc::Sender<Message>,
    writer: thread::JoinHandle<io::Result<()>>,
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
        let config = match kind {
            Kind::Plays => device.default_output_config(),
            Kind::Hears => device.default_input_config(),
        }
        .map_err(|e| format!("Couldn't open {device}: {e}"))?;
        let (rate, channels) = (config.sample_rate(), config.channels());
        // The name says how to read it, should it need reading by hand.
        let path = dir.join(format!("{} {rate} Hz {channels}ch f32le.raw", title.to_lowercase()));
        let file = File::create(&path).map_err(|e| format!("Couldn't record {}: {e}", kind.what()))?;

        let (messages, received) = mpsc::channel();
        let mut placer = Placer::new(file, started, rate, channels);
        let writer = thread::spawn(move || placer.write(&received));

        let format = config.sample_format();
        let config = config.config();
        let stream = match format {
            SampleFormat::F32 => open::<f32>(&device, config, &messages),
            SampleFormat::I16 => open::<i16>(&device, config, &messages),
            SampleFormat::I32 => open::<i32>(&device, config, &messages),
            SampleFormat::U16 => open::<u16>(&device, config, &messages),
            SampleFormat::U8 => open::<u8>(&device, config, &messages),
            SampleFormat::F64 => open::<f64>(&device, config, &messages),
            other => Err(format!("{device} gives its sound as {other}, which relight can't read.")),
        }?;
        stream.play().map_err(|e| format!("Couldn't start recording {device}: {e}"))?;
        let recorded = Recorded { title, path, rate, channels };
        Ok((Self { stream, messages, writer, recorded }, warning))
    }

    /// Stops it, and fills it out with silence to `stopped`, so each track runs the whole
    /// recording.
    pub fn finish(self, stopped: SystemTime) -> Result<Recorded, String> {
        drop(self.stream);
        let _ = self.messages.send(Message::Stop(stopped));
        let title = self.recorded.title;
        match self.writer.join() {
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

fn open<T>(device: &cpal::Device, config: cpal::StreamConfig, messages: &mpsc::Sender<Message>) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let messages = messages.clone();
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                let samples = data.iter().map(|&sample| sample.to_sample::<f32>()).collect();
                let _ = messages.send(Message::Sound(samples, SystemTime::now()));
            },
            |e| eprintln!("Sound capture: {e}"),
            None,
        )
        .map_err(|e| format!("Couldn't record {device}: {e}"))
}

/// Writes the samples where they belong in time.
struct Placer {
    out: BufWriter<File>,
    /// When the recording started: frame 0 of the file.
    started: SystemTime,
    rate: u32,
    channels: u16,
    /// Frames written so far.
    frames: u64,
    bytes: Vec<u8>,
}

impl Placer {
    fn new(file: File, started: SystemTime, rate: u32, channels: u16) -> Self {
        Self { out: BufWriter::with_capacity(1 << 16, file), started, rate, channels, frames: 0, bytes: Vec::new() }
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
        let mut placer = Placer::new(File::create(&path).unwrap(), started, 100, 1);
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
}
