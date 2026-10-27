//! Running ffmpeg and ffprobe, and reading what they report.

mod files;
mod probe;

use std::ffi::OsString;
use std::fmt::Display;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::{FfmpegEvent, LogLevel};
use ffmpeg_sidecar::ffmpeg_time_duration::FfmpegTimeDuration;
use serde::Serialize;

pub use ffmpeg_sidecar::paths::ffmpeg_path;
pub use files::{Partial, Scratch, remove_soon};
pub use probe::{AudioInfo, MediaInfo, VideoInfo, probe};

/// An ffmpeg argument list, from strings and paths alike.
#[macro_export]
macro_rules! args {
    ($($arg:expr),* $(,)?) => { vec![$(::std::ffi::OsString::from($arg)),*] };
}

/// The error a cancelled job ends with.
pub const CANCELLED: &str = "Cancelled";

/// Where a clip's video ends up when it only needs measuring.
pub const NULL_OUTPUT: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

/// Keeps a console window from flashing up for a command run from the window.
pub fn hide_window(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// The error for ffmpeg not starting, which almost always means it isn't installed.
pub fn start_failed(e: impl Display) -> String {
    format!("Couldn't start ffmpeg ({e}). Is it installed and on your PATH?")
}

/// The last `n` of ffmpeg's error lines, which say what went wrong.
pub fn last_lines(lines: &[String], n: usize) -> String {
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// What a job's ffmpeg runs tell the window.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Report {
    /// A new step has started.
    Stage { label: String },
    /// How far through the current step, 0 to 1.
    Progress { fraction: f64 },
    /// A line worth showing, such as the size budget.
    Note { text: String },
}

/// A long-running piece of work: where its reports go, and whether it's been cancelled.
#[derive(Clone)]
pub struct Job {
    cancelled: Arc<AtomicBool>,
    report: Arc<dyn Fn(Report) + Send + Sync>,
    /// The part of the step this job's progress covers: 0 to 1 is reported as `from` to
    /// `from + share`.
    from: f64,
    share: f64,
}

impl Job {
    pub fn new(cancelled: Arc<AtomicBool>, report: impl Fn(Report) + Send + Sync + 'static) -> Self {
        Self { cancelled, report: Arc::new(report), from: 0.0, share: 1.0 }
    }

    /// A job nobody watches or cancels.
    pub fn silent() -> Self {
        Self::new(Arc::new(AtomicBool::new(false)), |_| {})
    }

    /// The same job, for one run of several in a step: its progress counts as `from` to
    /// `to` of what this job's does.
    pub fn part(&self, from: f64, to: f64) -> Self {
        Self { from: self.from + from * self.share, share: (to - from) * self.share, ..self.clone() }
    }

    pub fn stage(&self, label: impl Into<String>) {
        (self.report)(Report::Stage { label: label.into() });
    }

    pub fn note(&self, text: impl Into<String>) {
        (self.report)(Report::Note { text: text.into() });
    }

    fn progress(&self, fraction: f64) {
        (self.report)(Report::Progress { fraction: self.from + fraction.clamp(0.0, 1.0) * self.share });
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// Runs ffmpeg on `args`, overwriting its output, and reports how far it is through
/// `duration` seconds of input. On failure the error names the step (`what`) and
/// ends with ffmpeg's last few error lines.
pub fn run(job: &Job, what: &str, duration: f64, args: Vec<OsString>) -> Result<(), String> {
    if job.is_cancelled() {
        return Err(CANCELLED.into());
    }
    let mut child = FfmpegCommand::new().hide_banner().overwrite().args(args).spawn().map_err(start_failed)?;

    let mut errors = Vec::new();
    let events = child.iter().map_err(|e| format!("Couldn't read ffmpeg's output: {e}"))?;
    for event in events {
        if job.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CANCELLED.into());
        }
        match event {
            FfmpegEvent::Progress(p) if duration > 0.0 => {
                if let Some(time) = FfmpegTimeDuration::from_str(&p.time) {
                    job.progress(time.as_seconds() / duration);
                }
            }
            FfmpegEvent::Log(LogLevel::Error | LogLevel::Fatal, line) | FfmpegEvent::Error(line) => {
                errors.push(line)
            }
            _ => {}
        }
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        return Ok(());
    }
    Err(format!("ffmpeg failed during {what}:\n{}", last_lines(&errors, 15)))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn parts_report_their_share_of_the_whole() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let job = Job::new(Arc::new(AtomicBool::new(false)), move |report| {
            if let Report::Progress { fraction } = report {
                log.lock().unwrap().push(fraction);
            }
        });
        let second_half = job.part(0.5, 1.0);
        second_half.progress(0.5);
        second_half.part(0.0, 0.5).progress(1.0);
        job.progress(2.0);
        assert_eq!(*seen.lock().unwrap(), [0.75, 0.75, 1.0]);
    }
}
