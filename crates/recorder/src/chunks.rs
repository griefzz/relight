//! The picture a replay holds: ffmpeg's fragmented MP4 stream, split where each fragment
//! starts. Every fragment starts on a keyframe, so the stream's header followed by any run
//! of fragments is a video that plays, and the oldest can simply be dropped.
//!
//! The fragments are held in memory or, bounced, each in a temporary file of its own.

use std::collections::VecDeque;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// A keyframe (so a new fragment) comes at least this often.
pub const KEYFRAME_SECONDS: u32 = 2;

/// How much more than asked for is held: a save takes in the fragment still being written
/// when it's asked for, and starts at a keyframe at or before where it was asked to.
const EXTRA_SECONDS: f64 = KEYFRAME_SECONDS as f64 + 1.0;

/// No box in the stream is anywhere near this big; one that says so is a broken stream.
const LARGEST_BOX: u64 = 1 << 30;

/// When the picture's time zero was, from what ffmpeg keeps printing. Taken again and again,
/// as a long replay's picture slowly drifts from the wall clock the sound is placed by.
#[derive(Default)]
pub struct Clock {
    /// In seconds since 1970, newest last.
    recent: Mutex<VecDeque<f64>>,
}

impl Clock {
    const SAMPLES: usize = 8;

    /// Takes in a time ffmpeg printed, in microseconds since 1970.
    pub fn add(&self, micros: f64) {
        let mut recent = self.recent.lock().unwrap();
        if recent.len() == Self::SAMPLES {
            recent.pop_front();
        }
        recent.push_back(micros / 1e6);
    }

    /// The best guess so far. Each is printed a little after its frame came, by however
    /// long ffmpeg took to get to it, so the earliest of the recent ones is nearest the truth.
    fn origin(&self) -> Option<f64> {
        self.recent.lock().unwrap().iter().copied().reduce(f64::min)
    }
}

/// One fragment of the stream.
pub struct Chunk {
    /// The header of the stream it came from, without which it can't be read.
    header: Arc<Vec<u8>>,
    body: Body,
    /// When its first frame was taken, in seconds since 1970.
    start: f64,
    /// In seconds.
    length: f64,
}

/// Where a chunk's bytes are.
enum Body {
    Memory(Vec<u8>),
    /// In a file of its own, that Windows removes when it's closed, whether by the chunk going
    /// or by relight crashing. It's marked as temporary, so Windows may hold off writing it out
    /// while memory's going spare, but gives that memory up to anything that wants it.
    Disk(Mutex<File>),
}

impl Chunk {
    fn end(&self) -> f64 {
        self.start + self.length
    }

    /// Moves its bytes into a new file in `dir`. If that can't be done, they stay where they are.
    fn bounce(&mut self, dir: &Path) -> io::Result<()> {
        if let Body::Memory(bytes) = &self.body {
            let mut file = tempfile::tempfile_in(dir)?;
            file.write_all(bytes)?;
            self.body = Body::Disk(Mutex::new(file));
        }
        Ok(())
    }

    fn write_to(&self, out: &mut impl Write) -> io::Result<()> {
        match &self.body {
            Body::Memory(bytes) => out.write_all(bytes),
            // Two saves can want the same chunk, and its file has only the one position.
            Body::Disk(file) => {
                let mut file = file.lock().unwrap();
                file.seek(SeekFrom::Start(0))?;
                io::copy(&mut *file, out).map(drop)
            }
        }
    }
}

/// A stretch of the picture to save. Its chunks stay around for as long as this does, even
/// once the replay itself has moved on.
pub struct Clip {
    chunks: Vec<Arc<Chunk>>,
    /// When its first frame was taken, in seconds since 1970.
    pub start: f64,
    /// In seconds: to where it was asked for, or to the end of what there was.
    pub length: f64,
    /// Whether it had to stop short of what was asked for, reaching back to a stream that can't
    /// be joined to the newest (the screen was another size).
    pub cut_short: bool,
    /// Whether the picture reached the moment it was asked for.
    complete: bool,
}

impl Clip {
    /// Each stream's chunks in turn.
    fn streams(&self) -> impl Iterator<Item = &[Arc<Chunk>]> {
        self.chunks.chunk_by(|a, b| Arc::ptr_eq(&a.header, &b.header))
    }

    /// When each stream starts in it, in seconds since 1970.
    #[cfg(test)]
    pub fn stream_starts(&self) -> Vec<f64> {
        self.streams().map(|stream| stream[0].start).collect()
    }

    /// Writes it into `dir`, and says which file to give ffmpeg. If the capture started again
    /// partway through, that's a list of a video for each stream, each placed by when it began.
    /// So nothing shows for the moment there was no picture but the last frame before it, and
    /// the rest stays in step with the sound, which carried on throughout.
    pub fn write_in(&self, dir: &Path) -> io::Result<PathBuf> {
        let streams: Vec<_> = self.streams().collect();
        if let [stream] = streams[..] {
            let path = dir.join("screen.mp4");
            write_stream(stream, &path)?;
            return Ok(path);
        }
        let mut list = String::from("ffconcat version 1.0\n");
        for (i, stream) in streams.iter().enumerate() {
            // With no spaces: ffmpeg won't take them from a list.
            let name = format!("screen-{}.mp4", i + 1);
            write_stream(stream, &dir.join(&name))?;
            list.push_str(&format!("file '{name}'\n"));
            if let Some(next) = streams.get(i + 1) {
                list.push_str(&format!("duration {:.6}\n", next[0].start - stream[0].start));
            }
        }
        let path = dir.join("screen.ffconcat");
        fs::write(&path, list)?;
        Ok(path)
    }
}

/// A stream's header followed by any run of its chunks is a video that plays.
fn write_stream(chunks: &[Arc<Chunk>], path: &Path) -> io::Result<()> {
    let mut out = BufWriter::with_capacity(1 << 20, File::create(path)?);
    out.write_all(&chunks[0].header)?;
    for chunk in chunks {
        chunk.write_to(&mut out)?;
    }
    out.flush()
}

struct Held {
    chunks: VecDeque<Arc<Chunk>>,
    last_arrival: Instant,
    /// Goes up when a stream ends.
    ended: u64,
    closed: bool,
    /// Why the newest chunk couldn't go to disk, if it couldn't.
    bounce_problem: Option<String>,
}

/// The newest stretch of the picture.
pub struct Picture {
    /// In seconds.
    keep: f64,
    /// Where chunks go as they arrive; without this, they're held in memory.
    bounce_to: Option<PathBuf>,
    held: Mutex<Held>,
    arrived: Condvar,
}

impl Picture {
    pub fn new(seconds: f64) -> Self {
        Self {
            keep: seconds + EXTRA_SECONDS,
            bounce_to: None,
            held: Mutex::new(Held {
                chunks: VecDeque::new(),
                last_arrival: Instant::now(),
                ended: 0,
                closed: false,
                bounce_problem: None,
            }),
            arrived: Condvar::new(),
        }
    }

    /// One that keeps its chunks on disk, leaving only the one arriving in memory.
    pub fn bounced(seconds: f64) -> Self {
        Self { bounce_to: Some(env::temp_dir()), ..Self::new(seconds) }
    }

    /// Why chunks are being held in memory despite being meant for the disk, if they are.
    pub fn bounce_problem(&self) -> Option<String> {
        self.held.lock().unwrap().bounce_problem.clone()
    }

    fn add(&self, mut chunk: Chunk) {
        // Before taking the lock, so no save has to wait for the disk. Nor does ffmpeg: a chunk
        // comes all at once and then nothing does until the next, so a slow write is no matter.
        // A failed one costs no more memory than not bouncing would, so the replay carries on.
        let problem = self.bounce_to.as_ref().and_then(|dir| chunk.bounce(dir).err());
        let oldest_wanted = chunk.end() - self.keep;
        let mut held = self.held.lock().unwrap();
        held.bounce_problem = problem.map(|e| e.to_string());
        held.chunks.push_back(Arc::new(chunk));
        // The oldest goes once the one after it reaches back far enough by itself.
        while held.chunks.len() > 1 && held.chunks[1].start <= oldest_wanted {
            held.chunks.pop_front();
        }
        held.last_arrival = Instant::now();
        drop(held);
        self.arrived.notify_all();
    }

    /// The stream has ended, so a save shouldn't wait for more of it. What came from it stays,
    /// to be saved along with what comes from the next, until it's too old like any other.
    pub fn stream_ended(&self) {
        let mut held = self.held.lock().unwrap();
        held.ended += 1;
        held.last_arrival = Instant::now();
        drop(held);
        self.arrived.notify_all();
    }

    /// The stream has ended, and what's held is no use: the clock it's placed by has moved.
    pub fn clear(&self) {
        self.held.lock().unwrap().chunks.clear();
        self.stream_ended();
    }

    /// No more is coming, so nobody should wait for it.
    pub fn close(&self) {
        self.held.lock().unwrap().closed = true;
        self.arrived.notify_all();
    }

    /// How long since a chunk last came (or the stream last started again).
    pub fn quiet_for(&self) -> Duration {
        self.held.lock().unwrap().last_arrival.elapsed()
    }

    /// Restarts the wait for the next chunk: a new stream is on its way.
    pub fn expect_chunks(&self) {
        self.held.lock().unwrap().last_arrival = Instant::now();
    }

    /// The `seconds` up to `pressed` (seconds since 1970), reaching back to the keyframe at
    /// or before where that starts. The newest frames are in a chunk still being written,
    /// so this waits for it, up to `patience`, then settles for what there is.
    pub fn clip(&self, pressed: f64, seconds: f64, patience: Duration) -> Option<Clip> {
        let deadline = Instant::now() + patience;
        let mut held = self.held.lock().unwrap();
        let ended = held.ended;
        let mut clip = pick(&held, pressed, seconds);
        while !held.closed && !clip.as_ref().is_some_and(|clip| clip.complete) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            held = self.arrived.wait_timeout(held, left).unwrap().0;
            // The stream ended under us: what had come by then is what there is.
            if held.ended != ended {
                break;
            }
            clip = pick(&held, pressed, seconds);
        }
        clip
    }
}

fn pick(held: &Held, pressed: f64, seconds: f64) -> Option<Clip> {
    let from = pressed - seconds;
    let mut chunks: Vec<_> = held.chunks.iter().filter(|chunk| chunk.end() > from && chunk.start < pressed).cloned().collect();
    // Back only as far as it can be joined to the newest. The same settings give the same
    // header, unless the screen's changed size, and a video can't simply change size partway.
    let newest = chunks.last()?.header.clone();
    let joinable = chunks.iter().rposition(|chunk| chunk.header != newest).map_or(0, |older| older + 1);
    chunks.drain(..joinable);

    let start = chunks.first()?.start;
    let end = chunks.last()?.end();
    // To the newest stream by the clock, as there may be gaps before it, and along it by what's
    // actually there, as the clock it's placed by moves a little.
    let newest_stream = chunks.iter().position(|chunk| Arc::ptr_eq(&chunk.header, &newest)).unwrap_or(0);
    let there_is = chunks[newest_stream].start - start + chunks[newest_stream..].iter().map(|chunk| chunk.length).sum::<f64>();
    Some(Clip {
        start,
        length: (pressed - start).min(there_is),
        cut_short: joinable > 0,
        // Or a chunk from after it has come, so no more that reach it are coming.
        complete: end >= pressed || held.chunks.back().is_some_and(|chunk| chunk.start >= pressed),
        chunks,
    })
}

/// Reads ffmpeg's stream into `picture` until it ends. Its timestamps are turned into the
/// wall clock's with `clock`, and each chunk's length comes from its frames at `fps`, which
/// the stream keeps to exactly.
pub fn follow(mut stream: impl Read, fps: u32, clock: &Clock, picture: &Picture) -> io::Result<()> {
    let mut header = Arc::default();
    let mut scale = None;
    let mut first_time = None;
    let mut piece = Vec::new();
    loop {
        let start = piece.len();
        let Some(kind) = read_box(&mut stream, &mut piece)? else { return Ok(()) };
        match &kind {
            b"moov" => {
                scale = timescale(&piece[start..]);
                header = Arc::new(std::mem::take(&mut piece));
            }
            // The header starts with this, and a fragment with the other: its data follows.
            b"ftyp" | b"moof" => {}
            b"mdat" => {
                let bytes = std::mem::take(&mut piece);
                if let (Some(scale), Some((time, frames)), Some(origin)) = (scale, fragment(&bytes), clock.origin()) {
                    let first = *first_time.get_or_insert(time);
                    picture.add(Chunk {
                        header: header.clone(),
                        body: Body::Memory(bytes),
                        start: origin + (time - first) as f64 / f64::from(scale),
                        length: f64::from(frames) / f64::from(fps),
                    });
                }
            }
            _ => piece.truncate(start),
        }
    }
}

/// Appends the stream's next box to `into`, and says what kind it is; none at the stream's end.
fn read_box(stream: &mut impl Read, into: &mut Vec<u8>) -> io::Result<Option<[u8; 4]>> {
    let mut head = [0; 8];
    match stream.read_exact(&mut head) {
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        other => other?,
    }
    let [a, b, c, d, kind @ ..] = head;
    let mut size = u64::from(u32::from_be_bytes([a, b, c, d]));
    into.extend_from_slice(&head);
    let mut read = 8;
    // A size of 1 means the real one is in the 8 bytes after the kind.
    if size == 1 {
        let mut large = [0; 8];
        stream.read_exact(&mut large)?;
        size = u64::from_be_bytes(large);
        into.extend_from_slice(&large);
        read = 16;
    }
    if !(read..=LARGEST_BOX).contains(&size) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a broken MP4 box"));
    }
    let left = size - read;
    into.reserve(left as usize);
    if stream.take(left).read_to_end(into)? < left as usize {
        return Ok(None);
    }
    Ok(Some(kind))
}

/// The contents of the first box of `kind` among those laid end to end in `data`.
fn inside<'a>(mut data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    while data.len() >= 8 {
        let size = u32::from_be_bytes(data[..4].try_into().ok()?) as usize;
        if size < 8 || size > data.len() {
            return None;
        }
        if &data[4..8] == kind {
            return Some(&data[8..size]);
        }
        data = &data[size..];
    }
    None
}

fn be_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// How many units of the video's timestamps make a second, from a `moov` box.
fn timescale(moov: &[u8]) -> Option<u32> {
    let trak = inside(inside(moov, b"moov")?, b"trak")?;
    let mdhd = inside(inside(trak, b"mdia")?, b"mdhd")?;
    // Past the version and flags, and the times made and changed, which are wider in version 1.
    be_u32(mdhd, if *mdhd.first()? == 1 { 20 } else { 12 }).filter(|&scale| scale > 0)
}

/// When a fragment starts in the video's timestamps, and how many frames it has.
fn fragment(bytes: &[u8]) -> Option<(u64, u32)> {
    let traf = inside(inside(bytes, b"moof")?, b"traf")?;
    let tfdt = inside(traf, b"tfdt")?;
    let time = if *tfdt.first()? == 1 {
        u64::from_be_bytes(tfdt.get(4..12)?.try_into().ok()?)
    } else {
        u64::from(be_u32(tfdt, 4)?)
    };
    Some((time, be_u32(inside(traf, b"trun")?, 4)?))
}

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

    use super::*;

    fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = (body.len() as u32 + 8).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    /// A header saying 1000 units make a second.
    fn header() -> Vec<u8> {
        let mut mdhd = vec![0; 12];
        mdhd.extend_from_slice(&1000u32.to_be_bytes());
        let moov = boxed(b"moov", &boxed(b"trak", &boxed(b"mdia", &boxed(b"mdhd", &mdhd))));
        [boxed(b"ftyp", b"mp42"), moov].concat()
    }

    /// A fragment of `frames` frames starting `time` units in.
    fn fragment_at(time: u32, frames: u32, data: &[u8]) -> Vec<u8> {
        let tfdt = boxed(b"tfdt", &[&[0; 4], &time.to_be_bytes()[..]].concat());
        let trun = boxed(b"trun", &[&[0; 4], &frames.to_be_bytes()[..]].concat());
        let moof = boxed(b"moof", &boxed(b"traf", &[tfdt, trun].concat()));
        [moof, boxed(b"mdat", data)].concat()
    }

    fn clock_at(seconds: f64) -> Clock {
        let clock = Clock::default();
        clock.add(seconds * 1e6);
        clock
    }

    /// A chunk of the stream with this header, whose byte says when it starts.
    fn chunk_of(header: &Arc<Vec<u8>>, start: f64, length: f64) -> Chunk {
        Chunk { header: header.clone(), body: Body::Memory(vec![start as u8]), start, length }
    }

    /// A chunk of the one stream that's going in most tests.
    fn chunk(start: f64, length: f64) -> Chunk {
        static STREAM: LazyLock<Arc<Vec<u8>>> = LazyLock::new(Arc::default);
        chunk_of(&STREAM, start, length)
    }

    fn bytes_of(chunk: &Chunk) -> Vec<u8> {
        let mut out = Vec::new();
        chunk.write_to(&mut out).unwrap();
        out
    }

    /// A picture bouncing its chunks to `dir`.
    fn bounced_to(dir: &Path, seconds: f64) -> Picture {
        Picture { bounce_to: Some(dir.to_owned()), ..Picture::new(seconds) }
    }

    fn files_in(dir: &Path) -> usize {
        fs::read_dir(dir).unwrap().count()
    }

    /// Waits for `dir` to have `count` files, as something else may have one open for a moment
    /// (a virus scanner) when it's closed here.
    fn wait_for_files_in(dir: &Path, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while files_in(dir) != count && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(files_in(dir), count);
    }

    #[test]
    fn the_stream_is_split_at_its_fragments_and_timed_by_the_clock() {
        let stream = [
            header(),
            fragment_at(5000, 60, b"one"),
            fragment_at(7000, 30, b"two"),
            // ffmpeg ends the stream with an index, which isn't a fragment.
            boxed(b"mfra", b"index"),
        ]
        .concat();
        let picture = Picture::new(100.0);
        follow(&stream[..], 30, &clock_at(1000.0), &picture).unwrap();

        let held = picture.held.lock().unwrap();
        assert_eq!(held.chunks.len(), 2);
        assert_eq!(*held.chunks[0].header, header());
        // The first fragment is the picture's time zero, whatever its timestamp says.
        assert_eq!((held.chunks[0].start, held.chunks[0].length), (1000.0, 2.0));
        assert_eq!((held.chunks[1].start, held.chunks[1].length), (1002.0, 1.0));
        assert_eq!(bytes_of(&held.chunks[0]), fragment_at(5000, 60, b"one"));
    }

    #[test]
    fn bouncing_leaves_no_chunk_in_memory_and_saves_the_very_same_video() {
        let dir = tempfile::tempdir().unwrap();
        let stream = [header(), fragment_at(0, 30, b"one"), fragment_at(1000, 30, b"two")].concat();
        let bounced = bounced_to(dir.path(), 100.0);
        follow(&stream[..], 30, &clock_at(0.0), &bounced).unwrap();

        {
            let held = bounced.held.lock().unwrap();
            assert!(held.chunks.iter().all(|chunk| matches!(chunk.body, Body::Disk(_))));
        }
        assert_eq!(files_in(dir.path()), 2);
        assert_eq!(bounced.bounce_problem(), None);

        let saved = bounced.clip(2.0, 2.0, Duration::ZERO).unwrap().write_in(dir.path()).unwrap();
        assert_eq!(fs::read(&saved).unwrap(), stream);
    }

    #[test]
    fn a_bounced_chunk_can_be_saved_again_and_again_even_at_the_same_time() {
        let dir = tempfile::tempdir().unwrap();
        let data = vec![7; 1 << 20];
        let stream = [header(), fragment_at(0, 30, &data)].concat();
        let bounced = bounced_to(dir.path(), 100.0);
        follow(&stream[..], 30, &clock_at(0.0), &bounced).unwrap();

        std::thread::scope(|scope| {
            for save in 0..4 {
                let (bounced, stream) = (&bounced, &stream);
                scope.spawn(move || {
                    let out = tempfile::tempdir().unwrap();
                    let saved = bounced.clip(1.0, 1.0, Duration::ZERO).unwrap().write_in(out.path()).unwrap();
                    assert!(fs::read(&saved).unwrap() == *stream, "save {save} differs");
                });
            }
        });
    }

    #[test]
    fn a_bounced_chunks_file_goes_when_nothing_wants_it_any_more() {
        let dir = tempfile::tempdir().unwrap();
        let picture = bounced_to(dir.path(), 4.0); // Keeps 7 seconds, so 4 of these.
        for i in 0..4 {
            picture.add(chunk(f64::from(i) * 2.0, 2.0));
        }
        wait_for_files_in(dir.path(), 4);

        // The replay moves on past a clip being saved: what it needs stays for it.
        let clip = picture.clip(8.0, 8.0, Duration::ZERO).unwrap();
        assert_eq!(clip.chunks.len(), 4);
        for i in 4..20 {
            picture.add(chunk(f64::from(i) * 2.0, 2.0));
        }
        wait_for_files_in(dir.path(), 8);
        let out = tempfile::tempdir().unwrap();
        let saved = clip.write_in(out.path()).unwrap();
        assert_eq!(fs::read(saved).unwrap(), [0, 2, 4, 6]);

        drop(clip);
        wait_for_files_in(dir.path(), 4);
        picture.clear();
        wait_for_files_in(dir.path(), 0);
    }

    #[test]
    fn a_chunk_that_cant_be_bounced_is_held_in_memory_and_said_so() {
        let dir = tempfile::tempdir().unwrap();
        let picture = bounced_to(&dir.path().join("not there"), 100.0);
        picture.add(chunk(0.0, 2.0));
        assert!(picture.bounce_problem().is_some());
        let saved = picture.clip(2.0, 2.0, Duration::ZERO).unwrap().write_in(dir.path()).unwrap();
        assert_eq!(fs::read(&saved).unwrap(), [0], "the replay still has it");

        // The disk comes back, and so does bouncing.
        fs::create_dir(dir.path().join("not there")).unwrap();
        picture.add(chunk(2.0, 2.0));
        assert_eq!(picture.bounce_problem(), None);
        assert_eq!(files_in(&dir.path().join("not there")), 1);
    }

    #[test]
    fn nothing_is_put_on_disk_unless_bouncing() {
        let picture = Picture::new(100.0);
        picture.add(chunk(0.0, 2.0));
        assert!(matches!(picture.held.lock().unwrap().chunks[0].body, Body::Memory(_)));
    }

    #[test]
    fn a_cut_off_stream_keeps_what_finished_arriving() {
        let whole = [header(), fragment_at(0, 30, b"one"), fragment_at(1000, 30, b"two")].concat();
        let cut = &whole[..whole.len() - 1];
        let picture = Picture::new(100.0);
        follow(cut, 30, &clock_at(0.0), &picture).unwrap();
        assert_eq!(picture.held.lock().unwrap().chunks.len(), 1);
    }

    #[test]
    fn a_nonsense_box_is_an_error_rather_than_a_huge_allocation() {
        let stream = [&u32::MAX.to_be_bytes()[..], b"mdat"].concat();
        let picture = Picture::new(100.0);
        assert!(follow(&stream[..], 30, &clock_at(0.0), &picture).is_err());
    }

    #[test]
    fn the_oldest_goes_once_it_isnt_needed() {
        let picture = Picture::new(4.0); // Keeps 7 seconds.
        for i in 0..10 {
            picture.add(chunk(f64::from(i) * 2.0, 2.0));
        }
        let held = picture.held.lock().unwrap();
        // Up to 20 s: the chunk at 12 s is the last to reach back to 13 s.
        assert_eq!(held.chunks.front().unwrap().start, 12.0);
        assert_eq!(held.chunks.back().unwrap().start, 18.0);
    }

    #[test]
    fn what_goes_never_leaves_a_save_short() {
        let picture = Picture::new(4.0);
        // Whichever moment in the last chunk a save was asked for, what it reaches back for
        // is still there.
        for i in 0..20 {
            picture.add(chunk(f64::from(i) * 2.0, 2.0));
            let newest_end = f64::from(i + 1) * 2.0;
            if newest_end > 10.0 {
                for pressed in [newest_end - 2.0, newest_end - 1.0, newest_end] {
                    let clip = picture.clip(pressed, 4.0, Duration::ZERO).unwrap();
                    assert!(clip.length >= 4.0, "{} s of the 4 asked for at {pressed}", clip.length);
                }
            }
        }
    }

    #[test]
    fn a_clip_starts_on_a_keyframe_and_ends_when_it_was_asked_for() {
        let picture = Picture::new(100.0);
        for i in 0..10 {
            picture.add(chunk(f64::from(i) * 2.0, 2.0));
        }
        let clip = picture.clip(11.0, 4.0, Duration::ZERO).unwrap();
        // 4 s back from 11 s is 7 s, in the chunk starting at 6 s; the chunk starting after
        // it was asked for isn't wanted.
        assert_eq!((clip.start, clip.length), (6.0, 5.0));
        assert_eq!(clip.chunks.len(), 3);
    }

    #[test]
    fn with_less_held_than_asked_for_a_clip_has_what_there_is() {
        let picture = Picture::new(100.0);
        picture.add(chunk(10.0, 2.0));
        let clip = picture.clip(11.0, 30.0, Duration::ZERO).unwrap();
        assert_eq!((clip.start, clip.length), (10.0, 1.0));
    }

    #[test]
    fn a_save_waits_for_the_chunk_being_written() {
        let picture = Arc::new(Picture::new(100.0));
        picture.add(chunk(0.0, 2.0));
        let adding = picture.clone();
        let adder = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            adding.add(chunk(2.0, 2.0));
        });
        let clip = picture.clip(3.0, 3.0, Duration::from_secs(10)).unwrap();
        adder.join().unwrap();
        assert_eq!((clip.start, clip.length), (0.0, 3.0));
    }

    #[test]
    fn a_save_settles_for_what_there_is_once_out_of_patience() {
        let picture = Picture::new(100.0);
        picture.add(chunk(0.0, 2.0));
        let clip = picture.clip(3.0, 3.0, Duration::from_millis(10)).unwrap();
        assert_eq!((clip.start, clip.length), (0.0, 2.0));
    }

    #[test]
    fn a_save_keeps_what_had_come_when_the_stream_ends_under_it() {
        let picture = Arc::new(Picture::new(100.0));
        picture.add(chunk(0.0, 2.0));
        let ending = picture.clone();
        let ender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            ending.stream_ended();
        });
        let clip = picture.clip(3.0, 3.0, Duration::from_secs(10)).unwrap();
        ender.join().unwrap();
        assert_eq!(clip.length, 2.0);
    }

    #[test]
    fn a_stream_ending_keeps_what_came_from_it_and_clearing_doesnt() {
        let picture = Picture::new(100.0);
        picture.add(chunk(0.0, 2.0));
        picture.stream_ended();
        assert_eq!(picture.clip(2.0, 2.0, Duration::ZERO).unwrap().chunks.len(), 1);
        picture.clear();
        assert!(picture.clip(2.0, 2.0, Duration::ZERO).is_none());
    }

    #[test]
    fn a_clip_reaches_back_across_the_capture_starting_again() {
        let dir = tempfile::tempdir().unwrap();
        // The same settings, so the same header, but two streams with a gap between them.
        let (first, second) = (Arc::new(vec![1]), Arc::new(vec![1]));
        let picture = Picture::new(100.0);
        picture.add(chunk_of(&first, 0.0, 2.0));
        picture.add(chunk_of(&first, 2.0, 2.0));
        picture.stream_ended();
        picture.add(chunk_of(&second, 7.0, 2.0));
        picture.add(chunk_of(&second, 9.0, 2.0));

        let clip = picture.clip(10.0, 10.0, Duration::ZERO).unwrap();
        assert_eq!((clip.start, clip.length), (0.0, 10.0), "the gap counts, as the sound carried on through it");
        assert!(!clip.cut_short);
        assert_eq!(clip.stream_starts(), [0.0, 7.0]);

        let saved = clip.write_in(dir.path()).unwrap();
        let list = "ffconcat version 1.0\nfile 'screen-1.mp4'\nduration 7.000000\nfile 'screen-2.mp4'\n";
        assert_eq!(fs::read_to_string(saved).unwrap(), list);
        assert_eq!(fs::read(dir.path().join("screen-1.mp4")).unwrap(), [1, 0, 2]);
        assert_eq!(fs::read(dir.path().join("screen-2.mp4")).unwrap(), [1, 7, 9]);
    }

    #[test]
    fn with_less_held_than_asked_for_across_a_gap_a_clip_ends_with_what_there_is() {
        let (first, second) = (Arc::new(vec![1]), Arc::new(vec![1]));
        let picture = Picture::new(100.0);
        picture.add(chunk_of(&first, 0.0, 2.0));
        picture.add(chunk_of(&second, 7.0, 2.0));
        let clip = picture.clip(20.0, 20.0, Duration::ZERO).unwrap();
        assert_eq!((clip.start, clip.length), (0.0, 9.0));
    }

    #[test]
    fn a_stream_over_before_the_clip_begins_isnt_part_of_it() {
        let (first, second) = (Arc::new(vec![1]), Arc::new(vec![1]));
        let picture = Picture::new(1000.0);
        picture.add(chunk_of(&first, 0.0, 2.0));
        picture.add(chunk_of(&second, 100.0, 2.0));
        picture.add(chunk_of(&second, 102.0, 2.0));
        // Not a clip with a long gap in it, which would be a lot of silence to write.
        let clip = picture.clip(103.0, 10.0, Duration::ZERO).unwrap();
        assert_eq!((clip.start, clip.length), (100.0, 3.0));
        assert_eq!(clip.stream_starts(), [100.0]);
        assert!(!clip.cut_short, "nothing asked for was left out");
    }

    #[test]
    fn with_the_capture_gone_a_while_theres_nothing_to_save() {
        let picture = Picture::new(100.0);
        picture.add(chunk(0.0, 2.0));
        picture.stream_ended();
        assert!(picture.clip(100.0, 10.0, Duration::ZERO).is_none(), "what's there is from long before");
    }

    #[test]
    fn a_clip_only_goes_back_as_far_as_the_screen_being_this_size() {
        let (size, other) = (Arc::new(vec![1]), Arc::new(vec![2]));
        let picture = Picture::new(100.0);
        picture.add(chunk_of(&size, 0.0, 2.0));
        picture.add(chunk_of(&other, 3.0, 2.0));
        picture.add(chunk_of(&size, 6.0, 2.0));
        let clip = picture.clip(8.0, 8.0, Duration::ZERO).unwrap();
        // Back this size again, but past a stretch that isn't.
        assert_eq!((clip.start, clip.length), (6.0, 2.0));
        assert!(clip.cut_short);
    }

    #[test]
    fn a_save_doesnt_wait_when_it_was_asked_for_in_the_gap() {
        let (first, second) = (Arc::new(vec![1]), Arc::new(vec![1]));
        let picture = Picture::new(100.0);
        picture.add(chunk_of(&first, 0.0, 2.0));
        picture.add(chunk_of(&second, 7.0, 2.0));
        // Nothing will ever reach 5 seconds: that's over, with the second stream under way.
        let asked = Instant::now();
        let clip = picture.clip(5.0, 5.0, Duration::from_secs(10)).unwrap();
        assert!(asked.elapsed() < Duration::from_secs(5), "waited {:?}", asked.elapsed());
        assert_eq!((clip.start, clip.length), (0.0, 2.0));
    }

    #[test]
    fn nothing_held_is_nothing_to_save() {
        assert!(Picture::new(100.0).clip(3.0, 3.0, Duration::ZERO).is_none());
    }

    #[test]
    fn the_clock_goes_by_the_earliest_of_the_recent_times() {
        let clock = Clock::default();
        assert_eq!(clock.origin(), None);
        clock.add(5e6);
        clock.add(4e6);
        clock.add(6e6);
        assert_eq!(clock.origin(), Some(4.0));
        for _ in 0..Clock::SAMPLES {
            clock.add(7e6);
        }
        assert_eq!(clock.origin(), Some(7.0), "older times fall away, so drift is followed");
    }
}
