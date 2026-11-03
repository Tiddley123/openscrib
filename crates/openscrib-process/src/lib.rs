//! The "Process" stage: narration â transcript (Whisper), click labels and
//! tooltips â OCR, then draft step text from both. Each stage runs only if
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
                let img = image::open(dir.path(frame)).with_context(|| format!("reading {frame}"))?.to_rgba8();
                vision::click_crop(&img, click, &crop)?;
                let answer = vision.ask(&vision::identify_prompt(step), Some(&crop), 24)?;
                // OCR has the exact text when it read real text; the model wins when
                // they disagree (OCR "reads" icons as nonsense like "HQR").
                step.vision = vision::clean_reply(&answer, 60)
                    .filter(|v| !v.eq_ignore_ascii_case("unknown"))
                    .filter(|v| !step.ocr.as_deref().is_some_and(|o| vision::same_label(o, v)));
                report.identified += step.vision.is_some() as usize;
            }
            let _ = std::fs::remove_file(&crop);
            name_from_context(manifest);
            refine_narration(manifest);
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
                let said = step.transcript.clone().unwrap_or_default();
                step.text = match vision::clean_reply(&answer, 300) {
                    Some(text) if vision::accept_polish(&draft, &said, &text) => text,
                    _ if step.kind == StepKind::Note => openscrib_core::sentence(&said),
                    _ => vision::combine(&draft, &said),
                };
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
        None => {
            name_from_context(manifest);
            refine_narration(manifest);
            for step in &mut manifest.steps {
                step.refresh_text();
            }
            report.skipped.push("Writing: vision model not installed".into());
        }
    }

    report.redrafted = manifest.steps.iter().zip(&before).filter(|(s, b)| s.text != **b).count();
    Ok(report)
}

/// Move narration that names the next action onto that action: people often
/// say "now I'll open Firefox" a few seconds before clicking it. A segment
/// moves forward to one of the next three steps (within 15 s) whose label or
/// typed text it mentions; otherwise it stays with the step it followed.
/// Facts from the recording beat the model's guesses:
/// - A click on the taskbar that is followed by work in another app opened
///   that app, so the icon is named after it ("scales - BambuStudio" ->
///   "BambuStudio"), not whatever the icon looked like to the model.
/// - A model answer that is just the current app's own name says nothing
///   about what was clicked, so it is dropped.
fn name_from_context(manifest: &mut Manifest) {
    let mut order: Vec<usize> = (0..manifest.steps.len()).collect();
    order.sort_by_key(|&i| manifest.steps[i].t_ms);
    for (n, &i) in order.iter().enumerate() {
        let next_app = order.get(n + 1).map(|&j| &manifest.steps[j]).filter(|next| {
            next.t_ms <= manifest.steps[i].t_ms + 20_000
        }).and_then(|next| next.window.as_ref()).filter(|w| !w.process.eq_ignore_ascii_case("explorer.exe")).map(app_name);
        let step = &mut manifest.steps[i];
        let Some(win) = &step.window else { continue };
        let on_taskbar = win.process.eq_ignore_ascii_case("explorer.exe") && win.rect.h <= 120 && win.rect.w >= 400;
        if step.kind == StepKind::Click && on_taskbar && let Some(app) = next_app.filter(|a| !a.is_empty()) {
            step.vision = Some(app);
            continue;
        }
        let own = app_name(win);
        if step.vision.as_deref().is_some_and(|v| similar(v, &own)) {
            step.vision = None;
        }
    }
}

/// The app part of a window title ("scales - BambuStudio" -> "BambuStudio"),
/// or a tidied process name when the title has none.
fn app_name(w: &openscrib_core::WindowInfo) -> String {
    let from_title = [" - ", " — ", " | "]
        .iter()
        .filter_map(|sep| w.title.rsplit_once(sep).map(|(_, app)| app.trim()))
        .find(|a| !a.is_empty());
    match from_title {
        Some(app) => app.to_string(),
        None if !w.title.trim().is_empty() && w.title.len() <= 40 => w.title.trim().to_string(),
        None => {
            let stem = w.process.trim_end_matches(".exe").trim_end_matches(".EXE");
            stem.split(['-', '_'])
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let mut c = p.chars();
                    c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

/// Loose name match ("Bamboo Studio" vs "BambuStudio"): letters only, and
/// at most a quarter of them different.
fn similar(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<Vec<_>>();
    let (a, b) = (norm(a), norm(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    // Levenshtein distance.
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()] * 4 <= a.len().max(b.len())
}

fn refine_narration(manifest: &mut Manifest) {
    let keywords = |s: &openscrib_core::Step| -> Vec<String> {
        let text = [s.vision.as_deref(), s.ocr.as_deref(), s.keys.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() >= 4 && !["button", "icon", "click", "enter", "drive", "menu", "field"].contains(w))
            .map(str::to_string)
            .collect()
    };
    let mut order: Vec<usize> = (0..manifest.steps.len()).collect();
    order.sort_by_key(|&i| manifest.steps[i].t_ms);
    for seg in &mut manifest.narration {
        let said = seg.text.to_lowercase();
        let Some(pos) = order.iter().position(|&i| Some(manifest.steps[i].id) == seg.step) else { continue };
        for &next in order.iter().skip(pos + 1).take(3) {
            let step = &manifest.steps[next];
            if step.t_ms > seg.end_ms + 15_000 {
                break;
            }
            // "youtu" typed matches "YouTube" said: compare on prefixes.
            if keywords(step).iter().any(|k| said.split(|c: char| !c.is_alphanumeric()).any(|w| w.len() >= 4 && (w.starts_with(k.as_str()) || k.starts_with(w)))) {
                seg.step = Some(step.id);
                break;
            }
        }
    }
    manifest.sync_transcripts();
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
    fn names_apps_from_windows() {
        let w = |title: &str, process: &str| openscrib_core::WindowInfo {
            rect: openscrib_core::Rect { x: 0, y: 0, w: 800, h: 600 },
            client: None,
            title: title.into(),
            process: process.into(),
        };
        assert_eq!(app_name(&w("scales - BambuStudio", "bambu-studio.exe")), "BambuStudio");
        assert_eq!(app_name(&w("", "bambu-studio.exe")), "Bambu Studio");
        assert!(similar("Bamboo Studio", "BambuStudio"));
        assert!(!similar("World coordinates", "BambuStudio"));
    }

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
