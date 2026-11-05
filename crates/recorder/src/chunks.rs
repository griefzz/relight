//! The picture a replay holds: ffmpeg's fragmented MP4 stream, split where each fragment
//! starts. Every fragment starts on a keyframe, so the stream's header followed by any run
//! of fragments is a video that plays, and the oldest can simply be dropped.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;
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
    bytes: Vec<u8>,
    /// When its first frame was taken, in seconds since 1970.
    start: f64,
    /// In seconds.
    length: f64,
}

impl Chunk {
    fn end(&self) -> f64 {
        self.start + self.length
    }
}

/// A stretch of the picture to save. Its chunks stay around for as long as this does, even
/// once the replay itself has moved on.
pub struct Clip {
    header: Arc<Vec<u8>>,
    chunks: Vec<Arc<Chunk>>,
    /// When its first frame was taken, in seconds since 1970.
    pub start: f64,
    /// In seconds: to where it was asked for, or to the end of what there was.
    pub length: f64,
    /// Whether the picture reached the moment it was asked for.
    complete: bool,
}

impl Clip {
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let mut out = BufWriter::with_capacity(1 << 20, File::create(path)?);
        out.write_all(&self.header)?;
        for chunk in &self.chunks {
            out.write_all(&chunk.bytes)?;
        }
        out.flush()
    }
}

struct Held {
    /// The stream's header, without which the chunks can't be read.
    header: Arc<Vec<u8>>,
    chunks: VecDeque<Arc<Chunk>>,
    last_arrival: Instant,
    /// Goes up when what's held is thrown away.
    cleared: u64,
    closed: bool,
}

/// The newest stretch of the picture, kept in memory.
pub struct Picture {
    /// In seconds.
    keep: f64,
    held: Mutex<Held>,
    arrived: Condvar,
}

impl Picture {
    pub fn new(seconds: f64) -> Self {
        Self {
            keep: seconds + EXTRA_SECONDS,
            held: Mutex::new(Held {
                header: Arc::default(),
                chunks: VecDeque::new(),
                last_arrival: Instant::now(),
                cleared: 0,
                closed: false,
            }),
            arrived: Condvar::new(),
        }
    }

    /// A new stream is starting.
    pub fn begin(&self, header: Vec<u8>) {
        self.held.lock().unwrap().header = Arc::new(header);
    }

    fn add(&self, chunk: Chunk) {
        let oldest_wanted = chunk.end() - self.keep;
        let mut held = self.held.lock().unwrap();
        held.chunks.push_back(Arc::new(chunk));
        // The oldest goes once the one after it reaches back far enough by itself.
        while held.chunks.len() > 1 && held.chunks[1].start <= oldest_wanted {
            held.chunks.pop_front();
        }
        held.last_arrival = Instant::now();
        drop(held);
        self.arrived.notify_all();
    }

    /// Throws away what's held: the stream it came from has ended.
    pub fn clear(&self) {
        let mut held = self.held.lock().unwrap();
        held.chunks.clear();
        held.cleared += 1;
        held.last_arrival = Instant::now();
        drop(held);
        self.arrived.notify_all();
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
        let cleared = held.cleared;
        let mut clip = pick(&held, pressed, seconds);
        while !held.closed && !clip.as_ref().is_some_and(|clip| clip.complete) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            held = self.arrived.wait_timeout(held, left).unwrap().0;
            // The stream ended under us: what had come by then is what there is.
            if held.cleared != cleared {
                break;
            }
            clip = pick(&held, pressed, seconds);
        }
        clip
    }
}

fn pick(held: &Held, pressed: f64, seconds: f64) -> Option<Clip> {
    let first = held.chunks.iter().rposition(|chunk| chunk.start <= pressed - seconds).unwrap_or(0);
    let chunks: Vec<_> = held.chunks.range(first..).filter(|chunk| chunk.start < pressed).cloned().collect();
    let (start, end) = (chunks.first()?.start, chunks.last()?.end());
    let there_is: f64 = chunks.iter().map(|chunk| chunk.length).sum();
    Some(Clip {
        header: held.header.clone(),
        start,
        length: (pressed - start).min(there_is),
        complete: end >= pressed,
        chunks,
    })
}

/// Reads ffmpeg's stream into `picture` until it ends. Its timestamps are turned into the
/// wall clock's with `clock`, and each chunk's length comes from its frames at `fps`, which
/// the stream keeps to exactly.
pub fn follow(mut stream: impl Read, fps: u32, clock: &Clock, picture: &Picture) -> io::Result<()> {
    let mut scale = None;
    let mut first_time = None;
    let mut piece = Vec::new();
    loop {
        let start = piece.len();
        let Some(kind) = read_box(&mut stream, &mut piece)? else { return Ok(()) };
        match &kind {
            b"moov" => {
                scale = timescale(&piece[start..]);
                picture.begin(std::mem::take(&mut piece));
            }
            // The header starts with this, and a fragment with the other: its data follows.
            b"ftyp" | b"moof" => {}
            b"mdat" => {
                let bytes = std::mem::take(&mut piece);
                if let (Some(scale), Some((time, frames)), Some(origin)) = (scale, fragment(&bytes), clock.origin()) {
                    let first = *first_time.get_or_insert(time);
                    picture.add(Chunk {
                        bytes,
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

    fn chunk(start: f64, length: f64) -> Chunk {
        Chunk { bytes: vec![start as u8], start, length }
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
        assert_eq!(*held.header, header());
        assert_eq!(held.chunks.len(), 2);
        // The first fragment is the picture's time zero, whatever its timestamp says.
        assert_eq!((held.chunks[0].start, held.chunks[0].length), (1000.0, 2.0));
        assert_eq!((held.chunks[1].start, held.chunks[1].length), (1002.0, 1.0));
        assert_eq!(held.chunks[0].bytes, fragment_at(5000, 60, b"one"));
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
        let clearing = picture.clone();
        let clearer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            clearing.clear();
        });
        let clip = picture.clip(3.0, 3.0, Duration::from_secs(10)).unwrap();
        clearer.join().unwrap();
        assert_eq!(clip.length, 2.0);
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
