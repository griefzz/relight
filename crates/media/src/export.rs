//! Exporting a timeline with one of the presets.

use std::fs;
use std::path::Path;

use relight_ffmpeg::{self as ffmpeg, Job, Partial, Scratch};
use serde::{Deserialize, Serialize};

use crate::discord;
use crate::timeline::{self, Picture, Segment};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Preset {
    /// Visually lossless H.264 at the first clip's size and frame rate.
    Full,
    /// Fits under a Discord upload limit.
    Discord(discord::Options),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exported {
    pub path: String,
    pub bytes: u64,
}

/// Exports `segments` to `output`, an MP4, which only appears once it's finished.
pub fn run(
    job: &Job,
    segments: &[Segment],
    picture: &Picture,
    output: &Path,
    preset: &Preset,
) -> Result<Exported, String> {
    if segments.is_empty() {
        return Err("The timeline is empty.".into());
    }
    if !output.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) {
        return Err("Exports are MP4 files: give it a name ending in .mp4.".into());
    }
    if let Ok(output) = fs::canonicalize(output)
        && segments.iter().any(|s| fs::canonicalize(&s.path).is_ok_and(|p| p == output))
    {
        return Err("Save it somewhere else: that file is on the timeline.".into());
    }

    let partial = Partial::new(output);
    match preset {
        Preset::Full => {
            job.stage("Encoding");
            timeline::render(job, segments, picture, partial.path(), &timeline::full_quality())?;
        }
        Preset::Discord(opts) => for_discord(job, segments, picture, partial.path(), opts)?,
    }
    partial.finish()?;

    let bytes = fs::metadata(output).map_err(|e| e.to_string())?.len();
    Ok(Exported { path: output.to_string_lossy().into_owned(), bytes })
}

fn for_discord(
    job: &Job,
    segments: &[Segment],
    picture: &Picture,
    output: &Path,
    opts: &discord::Options,
) -> Result<(), String> {
    // A whole clip, picture and sound unchanged, goes to the encoder as it is, which
    // copies it when it already fits.
    if let [only] = segments
        && !only.rebalanced()
    {
        let source = Path::new(&only.path);
        let info = ffmpeg::probe(source)?;
        let whole = only.start <= 0.001 && only.end >= info.duration - 0.001;
        if whole && info.video.as_ref().is_some_and(|video| picture.keeps(video)) {
            return discord::encode(job, source, output, opts);
        }
    }
    let scratch = Scratch::new()?;
    let rendered = scratch.path().join("timeline.mov");
    job.stage("Rendering the timeline");
    timeline::render(job, segments, picture, &rendered, &timeline::intermediate())?;
    discord::encode(job, &rendered, output, opts)
}
