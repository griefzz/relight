//! The sounds that say a replay was saved, or wasn't, for those who turn them on: from inside
//! a game, where the window can't be seen, they're the only word there is.
//!
//! Each plays as soon as the hotkey's pressed, after the moment the replay ends on, so it
//! isn't on that replay; a later replay reaching back this far will hear it.

use std::f32::consts::TAU;
use std::sync::LazyLock;

const RATE: u32 = 22050;
/// How loud, out of 1: to be noticed over a game without getting in the way of it.
const LOUDNESS: f32 = 0.2;

static SAVED: LazyLock<Vec<u8>> = LazyLock::new(|| wav(&[(880.0, 0.07), (1320.0, 0.14)]));
static FAILED: LazyLock<Vec<u8>> = LazyLock::new(|| wav(&[(330.0, 0.1), (220.0, 0.22)]));

pub fn saved() {
    play(&SAVED);
}

pub fn failed() {
    play(&FAILED);
}

/// Plays `sound` and returns at once. It's kept for the life of the app, as Windows reads
/// from it while it plays.
#[cfg(windows)]
fn play(sound: &'static [u8]) {
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

    // SAFETY: with SND_MEMORY, the first argument is the sound itself rather than a name;
    // it's a whole WAV file and, being static, outlives the sound playing.
    unsafe {
        PlaySoundW(sound.as_ptr().cast(), std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

#[cfg(not(windows))]
fn play(_sound: &'static [u8]) {}

/// A WAV file of the notes given, each a frequency and how long it rings for in seconds.
fn wav(notes: &[(f32, f32)]) -> Vec<u8> {
    let mut samples: Vec<u8> = Vec::new();
    for &(frequency, length) in notes {
        let frames = (length * RATE as f32) as u32;
        for frame in 0..frames {
            let time = frame as f32 / RATE as f32;
            // A quick start, so it doesn't click, then dying away like a struck note.
            let attack = (time / 0.004).min(1.0);
            let decay = (1.0 - frame as f32 / frames as f32).powi(2);
            let sample = (TAU * frequency * time).sin() * attack * decay * LOUDNESS;
            samples.extend_from_slice(&((sample * f32::from(i16::MAX)) as i16).to_le_bytes());
        }
    }
    let mut file = Vec::with_capacity(44 + samples.len());
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
    file.extend_from_slice(b"WAVEfmt ");
    file.extend_from_slice(&16u32.to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes()); // Plain samples.
    file.extend_from_slice(&1u16.to_le_bytes()); // One channel.
    file.extend_from_slice(&RATE.to_le_bytes());
    file.extend_from_slice(&(RATE * 2).to_le_bytes()); // Bytes a second.
    file.extend_from_slice(&2u16.to_le_bytes()); // Bytes a frame.
    file.extend_from_slice(&16u16.to_le_bytes()); // Bits a sample.
    file.extend_from_slice(b"data");
    file.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    file.extend_from_slice(&samples);
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wav_says_how_long_it_is_and_stays_quiet() {
        let file = wav(&[(440.0, 0.1), (880.0, 0.1)]);
        let samples = 2 * (0.1 * RATE as f32) as usize;
        assert_eq!(file.len(), 44 + samples * 2);
        assert_eq!(&file[..4], b"RIFF");
        assert_eq!(&file[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(file[4..8].try_into().unwrap()) as usize, file.len() - 8);
        assert_eq!(u32::from_le_bytes(file[40..44].try_into().unwrap()) as usize, samples * 2);
        let loudest = file[44..].chunks(2).map(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs()).max().unwrap();
        assert!(loudest > 0 && f32::from(loudest) <= f32::from(i16::MAX) * LOUDNESS);
    }
}
