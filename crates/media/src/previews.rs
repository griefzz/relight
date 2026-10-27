//! Pictures of a clip for the timeline: a strip of thumbnails, and how loud its sound is
//! over time. Made once per file, in the background, and kept in the app's cache.

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

use relight_ffmpeg::{self as ffmpeg, Job, Scratch, args, ffmpeg_path};
use serde::{Deserialize, Serialize};

/// Goes up when what's made changes, so older caches are made again.
const VERSION: u32 = 2;
const TILE_HEIGHT: u32 = 64;
const MOST_TILES: u32 = 200;
/// Thumbnails no closer together than this, in seconds.
const CLOSEST_TILES: f64 = 0.25;
/// How many thumbnails are read at once.
const THUMBNAILERS: usize = 4;
/// Decoder threads for each: more only hold more frames, for one frame wanted.
const DECODER_THREADS: &str = "2";
const PEAKS_PER_SECOND: u32 = 50;
/// The sound is read at this rate for its peaks: plenty to draw, and quick to decode.
const PEAK_SAMPLE_RATE: u32 = 8000;
/// Previews unused for this long are deleted.
const KEEP_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Previews {
    /// An image of `tiles` thumbnails side by side, `tile_width` × `tile_height` each,
    /// one every `step` seconds; none if the clip has no picture.
    pub strip: Option<String>,
    pub tiles: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub step: f64,
    /// Whether there are peaks to read with `peaks`.
    pub sound: bool,
    pub peaks_per_second: u32,
    /// For a recording, each sound source ("Game", "Microphone") on its own, as a file the
    /// player can play alongside the picture; its peaks are read with `peaks` too.
    pub sources: Vec<String>,
}

/// The peaks `make` made for the clip at `source`, of its sound or of one of its sources:
/// one byte each, 0 to 255.
pub fn peaks(cache: &Path, source: &Path, of: Option<usize>) -> Result<Vec<u8>, String> {
    let key = cache_key(source)?;
    let file = match of {
        None => cache.join(format!("{key}.peaks")),
        Some(n) => cache.join(format!("{key}.source{n}.peaks")),
    };
    fs::read(file).map_err(|e| format!("No waveform made for {}: {e}", source.display()))
}

/// Names a file's previews after its path, size and time, so a changed file gets new ones.
fn cache_key(source: &Path) -> Result<String, String> {
    let meta = fs::metadata(source).map_err(|e| format!("Couldn't read {}: {e}", source.display()))?;
    let mut hasher = DefaultHasher::new();
    (VERSION, source, meta.len(), meta.modified().ok()).hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

/// The previews of the clip at `source`, kept in `dir`: made now if they aren't there.
pub fn make(dir: &Path, source: &Path) -> Result<Previews, String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let key = cache_key(source)?;
    let index = dir.join(format!("{key}.json"));
    let peaks = dir.join(format!("{key}.peaks"));
    if let Some(cached) = cached(dir, &key) {
        return Ok(cached);
    }

    let info = ffmpeg::probe(source)?;
    let mut previews = Previews {
        strip: None,
        tiles: 0,
        tile_width: 0,
        tile_height: TILE_HEIGHT,
        step: 0.0,
        sound: false,
        peaks_per_second: PEAKS_PER_SECOND,
        sources: Vec::new(),
    };
    if let Some(video) = &info.video {
        let tiles = ((info.duration / CLOSEST_TILES).ceil() as u32).clamp(1, MOST_TILES);
        let step = info.duration / f64::from(tiles);
        let tile_width = (TILE_HEIGHT * video.width / video.height.max(1)).max(2) / 2 * 2;
        let strip = dir.join(format!("{key}.jpg"));
        if make_strip(source, tiles, step, tile_width, &strip)? {
            previews.strip = Some(strip.to_string_lossy().into_owned());
            previews.tiles = tiles;
            previews.tile_width = tile_width;
            previews.step = step;
        }
    }
    if info.audio.is_some() {
        fs::write(&peaks, read_peaks(source, 0)?).map_err(|e| e.to_string())?;
        previews.sound = true;
    }
    // A recording's sources, after its mix: each copied out on its own (no encoding, so
    // it's quick) for the player to balance, with its peaks.
    for n in 0..info.sources().map_or(0, <[String]>::len) {
        let track = n + 1;
        let sound = dir.join(format!("{key}.source{n}.m4a"));
        ffmpeg::run(&Job::silent(), "copying out a sound track", 0.0, args![
            "-i", source, "-map", format!("0:a:{track}"), "-vn", "-c", "copy", "-f", "mp4", &sound,
        ])?;
        let peaks = dir.join(format!("{key}.source{n}.peaks"));
        fs::write(&peaks, read_peaks(source, track)?).map_err(|e| e.to_string())?;
        previews.sources.push(sound.to_string_lossy().into_owned());
    }
    // Written last: its being there means the rest is.
    let json = serde_json::to_string(&previews).map_err(|e| e.to_string())?;
    fs::write(&index, json).map_err(|e| e.to_string())?;
    Ok(previews)
}

/// Puts a thumbnail from the middle of each `step` of `source` side by side in `strip`;
/// false if not one frame could be read.
///
/// Each thumbnail is its own quick seek, a few at a time: decoding the whole clip would
/// take minutes for a long recording, and reading only keyframes finds nothing in clips
/// with seconds between them.
fn make_strip(source: &Path, tiles: u32, step: f64, tile_width: u32, strip: &Path) -> Result<bool, String> {
    let scratch = Scratch::new()?;
    let tile = |i: u32| scratch.path().join(format!("{i:03}.jpg"));
    let next = AtomicU32::new(0);
    let failure = Mutex::new(None);
    let job = Job::silent();
    thread::scope(|threads| {
        for _ in 0..THUMBNAILERS {
            threads.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= tiles {
                        break;
                    }
                    let at = (f64::from(i) + 0.5) * step;
                    let made = ffmpeg::run(&job, "a thumbnail", 0.0, args![
                        "-threads", DECODER_THREADS, "-ss", format!("{at:.3}"), "-i", source, "-an", "-frames:v", "1",
                        "-vf", format!("scale={tile_width}:{TILE_HEIGHT}"), "-q:v", "3", tile(i),
                    ]);
                    if let Err(e) = made {
                        *failure.lock().unwrap() = Some(e);
                        break;
                    }
                }
            });
        }
    });
    if let Some(e) = failure.into_inner().unwrap() {
        return Err(e);
    }

    // A seek past the last frame (sound that runs on longer) makes nothing; those take
    // the thumbnail before, so the strip has no gaps.
    let Some(first) = (0..tiles).find(|&i| tile(i).exists()) else { return Ok(false) };
    let mut last = tile(first);
    for i in 0..tiles {
        if tile(i).exists() {
            last = tile(i);
        } else {
            fs::copy(&last, tile(i)).map_err(|e| e.to_string())?;
        }
    }
    ffmpeg::run(&job, "the thumbnails", 0.0, args![
        "-framerate", "1", "-i", scratch.path().join("%03d.jpg"),
        "-vf", format!("tile={tiles}x1"), "-frames:v", "1", "-q:v", "5", strip,
    ])?;
    Ok(true)
}

/// The previews already made, if they're all there. Marks them used, for `prune`.
fn cached(dir: &Path, key: &str) -> Option<Previews> {
    let index = dir.join(format!("{key}.json"));
    let previews: Previews = serde_json::from_str(&fs::read_to_string(&index).ok()?).ok()?;
    let strip_there = previews.strip.as_ref().is_none_or(|strip| Path::new(strip).exists());
    let peaks_there = !previews.sound || dir.join(format!("{key}.peaks")).exists();
    let sources_there = previews.sources.iter().enumerate().all(|(n, sound)| {
        Path::new(sound).exists() && dir.join(format!("{key}.source{n}.peaks")).exists()
    });
    if !(strip_there && peaks_there && sources_there) {
        return None;
    }
    let _ = fs::File::options().write(true).open(&index).and_then(|f| f.set_modified(SystemTime::now()));
    Some(previews)
}

/// The loudest sample in each `1 / PEAKS_PER_SECOND` of sound track `track`, 0 to 255.
fn read_peaks(source: &Path, track: usize) -> Result<Vec<u8>, String> {
    let mut command = Command::new(ffmpeg_path());
    command
        .args(["-v", "error", "-i"])
        .arg(source)
        .args(["-map", &format!("0:a:{track}"), "-ac", "1", "-ar", &PEAK_SAMPLE_RATE.to_string(), "-f", "s16le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = ffmpeg::hide_window(&mut command)
        .spawn()
        .map_err(ffmpeg::start_failed)?;
    let mut sound = child.stdout.take().expect("stdout is piped");

    let mut bucket = vec![0u8; (PEAK_SAMPLE_RATE / PEAKS_PER_SECOND * 2) as usize];
    let mut peaks = Vec::new();
    loop {
        let filled = fill(&mut sound, &mut bucket).map_err(|e| e.to_string())?;
        if filled == 0 {
            break;
        }
        let loudest = bucket[..filled]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&sample| i16::from_le_bytes(sample).unsigned_abs())
            .max()
            .unwrap_or(0);
        peaks.push((u32::from(loudest) * 255 / 32768) as u8);
        if filled < bucket.len() {
            break;
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() && peaks.is_empty() {
        return Err(format!("ffmpeg couldn't read the sound in {}", source.display()));
    }
    Ok(peaks)
}

/// Reads until `buf` is full or the stream ends; how much it read.
fn fill(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// Deletes previews that haven't been used for `KEEP_FOR`, and pieces left by a run that
/// didn't finish.
pub fn prune(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    let older_than = |path: &Path, age: Duration| {
        fs::metadata(path)
            .and_then(|meta| meta.modified())
            .is_ok_and(|time| now.duration_since(time).unwrap_or_default() > age)
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let Some(key) = name.to_str().and_then(|name| name.split('.').next()) else { continue };
        // A file's previews ("key.jpg", "key.source0.peaks") go with its index ("key.json");
        // without one, they're from a run that didn't finish.
        let index = dir.join(format!("{key}.json"));
        let stale = if index.exists() {
            older_than(&index, KEEP_FOR)
        } else {
            older_than(&path, Duration::from_secs(60 * 60))
        };
        if stale {
            let _ = fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::recording;

    #[test]
    #[ignore = "runs ffmpeg"]
    fn a_recording_gets_its_sources_on_their_own() {
        let scratch = Scratch::new().unwrap();
        let dir = scratch.path();
        let clip = recording(dir);
        let cache = dir.join("cache");
        let previews = make(&cache, &clip).unwrap();

        assert_eq!(previews.sources.len(), 2);
        for sound in &previews.sources {
            let info = ffmpeg::probe(Path::new(sound)).unwrap();
            assert!(info.video.is_none() && info.audio.is_some(), "{sound} is just its sound");
        }
        let loudest = |of| peaks(&cache, &clip, of).unwrap().into_iter().max().unwrap();
        assert!(loudest(Some(0)) > 10, "the game's tone shows");
        assert_eq!(loudest(Some(1)), 0, "the microphone was silent");
        assert_eq!(make(&cache, &clip).unwrap().sources, previews.sources, "and they're kept");

        // Pruning keeps what's in use, whatever it's called.
        prune(&cache);
        assert!(previews.sources.iter().all(|sound| Path::new(sound).exists()));
        assert!(peaks(&cache, &clip, Some(1)).is_ok());
    }

    #[test]
    #[ignore = "runs ffmpeg"]
    fn makes_a_strip_and_peaks_then_reuses_them() {
        let scratch = Scratch::new().unwrap();
        let dir = scratch.path();
        let clip = dir.join("clip.mp4");
        // Two seconds of silence, then two of a full-scale tone.
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "testsrc=size=1280x720:rate=30:duration=4"])
            .args(["-f", "lavfi", "-i", "aevalsrc='if(lt(t,2),0,sin(2*PI*440*t))':d=4"])
            .args(["-c:v", "libx264", "-c:a", "aac"])
            .arg(&clip)
            .status()
            .unwrap();
        assert!(status.success());

        let cache = dir.join("cache");
        let previews = make(&cache, &clip).unwrap();
        assert_eq!((previews.tiles, previews.tile_width, previews.tile_height), (16, 112, 64));
        assert!(previews.sources.is_empty(), "a clip that isn't a recording has no sources");
        let strip = ffmpeg::probe(Path::new(previews.strip.as_ref().unwrap())).unwrap();
        let strip = strip.video.unwrap();
        assert_eq!((strip.width, strip.height), (16 * 112, 64));

        let peaks = peaks(&cache, &clip, None).unwrap();
        assert!((peaks.len() as i64 - 200).abs() <= 3, "{} peaks", peaks.len());
        assert!(peaks[10..90].iter().all(|&p| p < 5), "silence reads quiet");
        assert!(peaks[110..190].iter().all(|&p| p > 200), "the tone reads loud");

        // The second time comes from the cache instead of being made again...
        let strip = previews.strip.unwrap();
        let made = fs::metadata(&strip).unwrap().modified().unwrap();
        assert_eq!(make(&cache, &clip).unwrap().strip.as_deref(), Some(strip.as_str()));
        assert_eq!(fs::metadata(&strip).unwrap().modified().unwrap(), made);
        // ...until the file changes.
        fs::write(&clip, b"").unwrap();
        assert!(make(&cache, &clip).is_err(), "an empty file has nothing to preview");
    }
}
