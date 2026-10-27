//! The Discord preset from a terminal, like discord_encode.py:
//!
//!     cargo run --release -p relight-media --example discord_encode -- <input> [output] [--mb 20] [--av1] [--film]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use relight_ffmpeg::{self as ffmpeg, Job, Report};
use relight_media::discord::{Options, Tune};
use relight_media::export::{Preset, run};
use relight_media::timeline::{Picture, Segment};

fn main() {
    let mut opts = Options::default();
    let mut paths = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mb" => opts.target_mb = args.next().and_then(|mb| mb.parse().ok()).expect("--mb takes a number"),
            "--av1" => opts.av1 = true,
            "--film" => opts.tune = Tune::Film,
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    let Some(input) = paths.first() else {
        eprintln!("usage: discord_encode <input> [output] [--mb 20] [--av1] [--film]");
        std::process::exit(2);
    };
    let output = paths.get(1).cloned().unwrap_or_else(|| {
        let mut name = input.with_extension("").into_os_string();
        name.push("_discord.mp4");
        name.into()
    });

    let job = Job::new(Arc::new(AtomicBool::new(false)), |report| match report {
        Report::Stage { label } => println!("{label}..."),
        Report::Note { text } => println!("{text}"),
        Report::Progress { .. } => {}
    });
    let whole = ffmpeg::probe(input).map(|info| Segment {
        path: input.to_string_lossy().into_owned(),
        start: 0.0,
        end: info.duration,
    });
    let preset = Preset::Discord(opts);
    match whole.and_then(|segment| run(&job, &[segment], &Picture::default(), &output, &preset)) {
        Ok(done) => println!("Done!     {} ({:.2} MB)", done.path, done.bytes as f64 / 1000.0 / 1000.0),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}
