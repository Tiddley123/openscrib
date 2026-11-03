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
        if seg.no_speech_probability() > 0.6 {
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

/// People usually say what they're about to do just before doing it, so a
/// segment is matched as if it happened this much later.
const SPEECH_LEAD_MS: u64 = 700;

/// Assign each segment to the step closest in time and join the text per
/// step. `step_times` may be in any order (the editor reorders steps); the
/// result lines up with it.
pub fn align(step_times: &[u64], segments: &[Segment]) -> Vec<Option<String>> {
    let mut order: Vec<usize> = (0..step_times.len()).collect();
    order.sort_by_key(|&i| step_times[i]);
    let mut out: Vec<Vec<&str>> = vec![Vec::new(); step_times.len()];
    for seg in segments {
        let at = (seg.start_ms + seg.end_ms) / 2 + SPEECH_LEAD_MS;
        // The step window runs from halfway after the previous step to
        // halfway before the next one.
        let pos = order.partition_point(|&i| step_times[i] <= at);
        let idx = match (pos.checked_sub(1), order.get(pos)) {
            (Some(p), Some(&n)) => {
                let prev = order[p];
                if at - step_times[prev] <= step_times[n] - at { prev } else { n }
            }
            (Some(p), None) => order[p],
            (None, Some(&n)) => n,
            (None, None) => continue,
        };
        out[idx].push(seg.text.as_str());
    }
    out.into_iter().map(|t| (!t.is_empty()).then(|| t.join(" "))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(a: u64, b: u64, t: &str) -> Segment {
        Segment { start_ms: a, end_ms: b, text: t.into() }
    }

    #[test]
    fn segments_go_to_the_nearest_step() {
        let steps = [2_000, 6_000, 12_000];
        let segs = [
            seg(500, 1_500, "Open the start menu"),
            seg(4_000, 5_000, "now this PC"),
            seg(5_200, 5_600, "here"),
            seg(11_000, 11_800, "and the C drive"),
        ];
        assert_eq!(
            align(&steps, &segs),
            [Some("Open the start menu".into()), Some("now this PC here".into()), Some("and the C drive".into())]
        );
    }

    #[test]
    fn works_with_reordered_steps() {
        let steps = [12_000, 2_000];
        let segs = [seg(500, 1_500, "first"), seg(11_000, 11_800, "second")];
        assert_eq!(align(&steps, &segs), [Some("second".into()), Some("first".into())]);
        assert_eq!(align(&[], &segs), Vec::<Option<String>>::new());
    }
}
