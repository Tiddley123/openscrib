//! The "Process" stage: narration → transcript (Whisper), click labels and
//! tooltips → OCR, then draft step text from both. Each stage runs only if
//! its model is installed, one model at a time, and never overwrites text
//! the user has edited.

pub mod audio;
pub mod models;
pub mod ocr;
pub mod transcribe;
pub mod vision;

use std::path::PathBuf;

use anyhow::{Context, Result};
use openscrib_core::project::SCREEN_FILE;
use openscrib_core::{Manifest, NarrationSegment, ProjectDir, StepKind};
use serde::Serialize;

#[derive(Debug, Clone, Default)]
pub struct ProcessOptions {
    pub whisper: Option<PathBuf>,
    /// English-only Whisper models must be told the language.
    pub english_only: bool,
    pub ocr: Option<(PathBuf, PathBuf)>,
    /// Vision language model and its image projector; needs the helper exe too.
    pub vision: Option<(PathBuf, PathBuf)>,
}

impl ProcessOptions {
    /// Use whatever is installed: the chosen Whisper model (or the default)
    /// and the OCR models.
    pub fn installed(whisper_id: Option<&str>) -> Self {
        let wm = models::find(whisper_id.unwrap_or(models::DEFAULT_WHISPER))
            .filter(|m| models::installed(m))
            .or_else(|| {
                models::CATALOG.iter().find(|m| m.kind == models::ModelKind::Whisper && models::installed(m))
            });
        Self {
            whisper: wm.map(models::path),
            english_only: wm.is_some_and(|m| m.id.ends_with(".en")),
            ocr: models::ocr_paths(),
            vision: models::vision_paths().filter(|_| vision::helper_path().is_some()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub stage: &'static str,
    pub fraction: f32,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub segments: usize,
    pub steps_with_narration: usize,
    pub labels: usize,
    pub tooltips: usize,
    pub redrafted: usize,
    /// The guide was untitled and got a title from the narration.
    pub titled: bool,
    /// Click targets named by the vision model.
    pub identified: usize,
    pub skipped: Vec<String>,
}

pub fn process(
    dir: &ProjectDir,
    manifest: &mut Manifest,
    opts: &ProcessOptions,
    mut progress: impl FnMut(Progress),
) -> Result<Report> {
    let mut report = Report::default();
    let mut say = |stage: &'static str, fraction: f32, message: String| progress(Progress { stage, fraction, message });

    // 1. Narration. Whisper loads, runs and is dropped before OCR loads.
    match (&opts.whisper, manifest.audio.is_some()) {
        (Some(model), true) => {
            say("narration", 0.0, "Decoding narration".into());
            if let Some(narration) = audio::decode(&dir.path(SCREEN_FILE))? {
                say("narration", 0.1, format!("Transcribing {:.0}s of narration", narration.duration_ms() as f64 / 1000.0));
                let segments = transcribe::transcribe(model, &narration, opts.english_only)?;
                report.segments = segments.len();
                let times: Vec<u64> = manifest.steps.iter().map(|s| s.t_ms).collect();
                let owners = transcribe::assign(&times, &segments);
                manifest.narration = segments
                    .into_iter()
                    .zip(owners)
                    .map(|(s, owner)| NarrationSegment {
                        start_ms: s.start_ms,
                        end_ms: s.end_ms,
                        text: s.text,
                        step: owner.map(|i| manifest.steps[i].id),
                    })
                    .collect();
                manifest.sync_transcripts();
                report.steps_with_narration = manifest.steps.iter().filter(|s| s.transcript.is_some()).count();
            }
            say("narration", 1.0, format!("{} narration segments", report.segments));
        }
        (None, true) => report.skipped.push("Narration: no Whisper model installed".into()),
        (_, false) => report.skipped.push("Narration: this recording has no audio".into()),
    }

    // 2. On-screen text around each click.
    if let Some((det, rec)) = &opts.ocr {
        say("ocr", 0.0, "Loading OCR".into());
        let engine = ocr::Ocr::load(det, rec)?;
        let total = manifest.steps.len().max(1);
        for (i, step) in manifest.steps.iter_mut().enumerate() {
            say("ocr", i as f32 / total as f32, format!("Reading step {} of {}", i + 1, total));
            let (StepKind::Click, Some(click), Some(frame)) = (step.kind, &step.click, &step.frame) else { continue };
            let img = image::open(dir.path(frame)).with_context(|| format!("reading {frame}"))?.to_rgba8();
            step.ocr = engine.label_at(&img, click.in_frame)?;
            report.labels += step.ocr.is_some() as usize;
            step.tooltip = match &step.pre_frame {
                Some(pre) => {
                    let before = image::open(dir.path(pre)).with_context(|| format!("reading {pre}"))?.to_rgba8();
                    engine.tooltip(&before, &img, click.in_frame)?
                }
                None => None,
            };
            report.tooltips += step.tooltip.is_some() as usize;
        }
        say("ocr", 1.0, format!("Read {} labels", report.labels));
    } else {
        report.skipped.push("Labels: OCR models not installed".into());
    }

    // A guide named "Untitled" helps nobody; the first thing said is usually
    // what the guide is about.
    if manifest.title.as_deref().is_none_or(|t| t.trim().is_empty())
        && let Some(title) = manifest.narration.first().and_then(|s| title_from(&s.text))
    {
        manifest.title = Some(title);
        report.titled = true;
    }

    // 3. Draft text from everything we now know.
    let before: Vec<String> = manifest.steps.iter().map(|s| s.text.clone()).collect();
    for step in &mut manifest.steps {
        step.refresh_text();
    }

    // 4. Vision model: name what OCR couldn't, then write the steps and title.
    match &opts.vision {
        Some((model, mmproj)) => {
            say("vision", 0.0, "Loading the vision model".into());
            let mut vision = vision::Vision::start(model, mmproj)?;
            let crop = tempfile_path("openscrib-crop.png");
            let total = manifest.steps.len().max(1) * 2;
            for (i, step) in manifest.steps.iter_mut().enumerate() {
                say("vision", i as f32 / total as f32, format!("Looking at step {}", i + 1));
                step.vision = None;
                let (StepKind::Click, Some(click), Some(frame)) = (step.kind, &step.click, &step.frame) else { continue };
                if !vision::label_is_weak(step.ocr.as_deref()) {
                    continue;
                }
                let img = image::open(dir.path(frame)).with_context(|| format!("reading {frame}"))?.to_rgba8();
                vision::click_crop(&img, click, &crop)?;
                let answer = vision.ask(&vision::identify_prompt(step), Some(&crop), 24)?;
                step.vision = vision::clean_reply(&answer, 60);
                report.identified += step.vision.is_some() as usize;
            }
            let _ = std::fs::remove_file(&crop);
            let n = manifest.steps.len();
            for (i, step) in manifest.steps.iter_mut().enumerate() {
                say("vision", (n + i) as f32 / total as f32, format!("Writing step {}", i + 1));
                // The draft (action + name) is already right; only narration
                // adds something a small model can't make up.
                step.refresh_text();
                if step.text_edited || step.transcript.is_none() {
                    continue;
                }
                let draft = step.draft_text();
                let answer = vision.ask(&vision::polish_prompt(step, &draft), None, 80)?;
                if let Some(text) = vision::clean_reply(&answer, 300) {
                    step.text = text;
                }
            }
            if report.titled || manifest.title.as_deref().is_none_or(|t| t.trim().is_empty()) {
                let steps: Vec<String> = manifest.steps.iter().map(|s| s.text.clone()).collect();
                if let Some(t) = vision::clean_reply(&vision.ask(&vision::title_prompt(&steps), None, 24)?, 80) {
                    manifest.title = Some(t.trim_end_matches('.').to_string());
                    report.titled = true;
                }
            }
            say("vision", 1.0, format!("Named {} items", report.identified));
        }
        None => report.skipped.push("Writing: vision model not installed".into()),
    }

    report.redrafted = manifest.steps.iter().zip(&before).filter(|(s, b)| s.text != **b).count();
    Ok(report)
}

fn tempfile_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{}-{name}", std::process::id()))
}

/// A short title from the first sentence of `text`: capitalised, at most
/// about 60 characters (cut at a word), without trailing punctuation.
pub fn title_from(text: &str) -> Option<String> {
    let first = text.split_inclusive(['.', '!', '?']).next()?.trim();
    let mut out = String::new();
    for word in first.split_whitespace() {
        if out.len() + word.len() + 1 > 60 {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    let out = out.trim_end_matches(['.', ',', '!', '?', ';', ':']);
    let mut chars = out.chars();
    let first = chars.next()?;
    let title: String = first.to_uppercase().chain(chars).collect();
    (title.split_whitespace().count() >= 2).then_some(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_from_first_sentence() {
        assert_eq!(
            title_from("today we're resetting a user's password in AD. First open the console."),
            Some("Today we're resetting a user's password in AD".into())
        );
        assert_eq!(title_from("Okay."), None);
        let long = "this is a very long opening sentence that goes on and on well past sixty characters total";
        assert!(title_from(long).unwrap().len() <= 60);
    }
}
