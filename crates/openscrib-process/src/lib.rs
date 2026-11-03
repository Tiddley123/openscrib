//! The "Process" stage: narration → transcript (Whisper), click labels and
//! tooltips → OCR, then draft step text from both. Each stage runs only if
//! its model is installed, one model at a time, and never overwrites text
//! the user has edited.

pub mod audio;
pub mod models;
pub mod ocr;
pub mod transcribe;

use std::path::PathBuf;

use anyhow::{Context, Result};
use openscrib_core::project::SCREEN_FILE;
use openscrib_core::{Manifest, ProjectDir, StepKind};
use serde::Serialize;

#[derive(Debug, Clone, Default)]
pub struct ProcessOptions {
    pub whisper: Option<PathBuf>,
    /// English-only Whisper models must be told the language.
    pub english_only: bool,
    pub ocr: Option<(PathBuf, PathBuf)>,
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
                for (step, text) in manifest.steps.iter_mut().zip(transcribe::align(&times, &segments)) {
                    report.steps_with_narration += text.is_some() as usize;
                    step.transcript = text;
                }
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

    // 3. Draft text from everything we now know.
    for step in &mut manifest.steps {
        let before = step.text.clone();
        step.refresh_text();
        report.redrafted += (step.text != before) as usize;
    }
    Ok(report)
}
