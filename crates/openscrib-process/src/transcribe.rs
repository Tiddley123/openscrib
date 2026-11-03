//! Narration to timed text with whisper.cpp, then matched to steps.

use std::path::Path;

use anyhow::{Context, Result};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::audio::Narration;

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// Below this RMS the narration is treated as silence and not transcribed.
const SILENCE_RMS: f32 = 0.004;

/// Whisper's stock outputs for silence or noise.
const NOISE: [&str; 8] = ["[blank_audio]", "(silence)", "[silence]", "(music)", "[music]", "you", "thank you.", "[ silence ]"];

pub fn transcribe(model: &Path, narration: &Narration, english_only: bool) -> Result<Vec<Segment>> {
    if narration.rms() < SILENCE_RMS {
        return Ok(Vec::new());
    }
    whisper_rs::install_logging_hooks();
    let ctx = WhisperContext::new_with_params(model, WhisperContextParameters::default())
        .with_context(|| format!("loading {}", model.display()))?;
    let mut state = ctx.create_state().context("creating Whisper state")?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some(if english_only { "en" } else { "auto" }));
    params.set_n_threads(std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_print_special(false);
    params.set_suppress_blank(true);
    state.full(params, &narration.samples).context("transcribing narration")?;

    let mut out = Vec::new();
    for i in 0..state.full_n_segments() {
        let Some(seg) = state.get_segment(i) else { continue };
        if seg.no_speech_probability() > 0.85 {
            continue;
        }
        let text = seg.to_str_lossy().map(|t| t.trim().to_string()).unwrap_or_default();
        if text.is_empty() || NOISE.contains(&text.to_ascii_lowercase().as_str()) || is_sound_tag(&text) {
            continue;
        }
        // Whisper timestamps are in 10 ms units from the start of the audio.
        out.push(Segment {
            start_ms: narration.start_ms + seg.start_timestamp().max(0) as u64 * 10,
            end_ms: narration.start_ms + seg.end_timestamp().max(0) as u64 * 10,
            text,
        });
    }
    Ok(out)
}

/// Whisper annotates non-speech as `[typing]`, `(keyboard clicking)`, `*music*`.
fn is_sound_tag(text: &str) -> bool {
    let t = text.trim();
    [('[', ']'), ('(', ')'), ('*', '*')]
        .iter()
        .any(|(open, close)| t.starts_with(*open) && t.ends_with(*close))
}

/// Speech this close before a click is about that click ("now click Save");
/// anything later, up to the next click, is a follow-up on it.
const LEAD_MS: u64 = 1_500;

/// For each segment, the index into `step_times` of the step it belongs to.
/// A step owns the time from `LEAD_MS` before it until `LEAD_MS` before the
/// next step; speech before the first step goes to the first step.
/// `step_times` may be in any order (the editor reorders steps).
pub fn assign(step_times: &[u64], segments: &[Segment]) -> Vec<Option<usize>> {
    let mut order: Vec<usize> = (0..step_times.len()).collect();
    order.sort_by_key(|&i| step_times[i]);
    segments
        .iter()
        .map(|seg| {
            let at = seg.start_ms + LEAD_MS;
            let pos = order.partition_point(|&i| step_times[i] <= at);
            order.get(pos.saturating_sub(1)).copied()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(a: u64, b: u64, t: &str) -> Segment {
        Segment { start_ms: a, end_ms: b, text: t.into() }
    }

    #[test]
    fn speech_belongs_to_the_click_it_follows() {
        let steps = [2_000, 6_000, 12_000];
        let segs = [
            seg(500, 1_500, "Open the start menu"),    // before any click
            seg(2_400, 3_500, "that opens the menu"),  // follow-up on step 1
            seg(4_800, 5_600, "now this PC"),          // just before step 2
            seg(13_000, 15_000, "and we are done"),    // after the last click
        ];
        assert_eq!(assign(&steps, &segs), [Some(0), Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn works_with_reordered_steps() {
        let steps = [12_000, 2_000];
        let segs = [seg(500, 1_500, "first"), seg(11_000, 11_800, "second")];
        assert_eq!(assign(&steps, &segs), [Some(1), Some(0)]);
        assert_eq!(assign(&[], &segs), [None, None]);
    }
}
