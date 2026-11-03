//! Voiceover for exported video: each step's text read aloud by Piper (a
//! small offline text-to-speech engine run as a helper process), placed at
//! that step on the output timeline and encoded as the Opus audio track.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use openscrib_core::mkv::OPUS_SAMPLE_RATE;
use openscrib_core::{Manifest, Step, StepKind};

#[derive(Debug, Clone)]
pub struct Voice {
    pub piper: PathBuf,
    /// Voice model (`.onnx`, with its `.onnx.json` next to it).
    pub model: PathBuf,
}

/// Pause between consecutive spoken steps.
const GAP_MS: u64 = 300;
/// Start speaking slightly before the click is shown.
const LEAD_MS: u64 = 300;

pub struct Track {
    /// 48 kHz mono samples covering the whole output.
    pub pcm: Vec<f32>,
    /// What is spoken when, for captions.
    pub captions: Vec<(u64, u64, String)>,
}

/// Step text as it should be spoken: no Markdown, keys read as words.
pub fn spoken_text(step: &Step) -> String {
    let t = step
        .text
        .replace("**", "")
        .replace('`', "")
        .replace("{Enter}", ", then press Enter")
        .replace("{Tab}", ", then press Tab")
        .replace("{Esc}", ", then press Escape")
        .replace("{Backspace}", "");
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if step.kind == StepKind::Shortcut { t.replace('+', " plus ") } else { t }
}

impl Voice {
    fn sample_rate(&self) -> u32 {
        // Piper voices state their rate in the config; 22050 is the common one.
        std::fs::read_to_string(self.model.with_extension("onnx.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v["audio"]["sample_rate"].as_u64())
            .unwrap_or(22_050) as u32
    }

    /// Synthesise `text` as 48 kHz mono.
    pub fn speak(&self, text: &str) -> Result<Vec<f32>> {
        let mut cmd = Command::new(&self.piper);
        cmd.arg("--model").arg(&self.model).arg("--output_raw");
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().with_context(|| format!("starting {}", self.piper.display()))?;
        {
            let mut stdin = child.stdin.take().context("piper stdin")?;
            stdin.write_all(text.replace('\n', " ").as_bytes())?;
            stdin.write_all(b"\n")?;
        }
        let mut raw = Vec::new();
        child.stdout.take().context("piper stdout")?.read_to_end(&mut raw)?;
        if !child.wait()?.success() {
            bail!("the voice engine failed");
        }
        let samples: Vec<f32> = raw.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c) as f32 / 32768.0).collect();
        Ok(resample(&samples, self.sample_rate(), OPUS_SAMPLE_RATE))
    }
}

/// Speak every step that falls inside `segs` (the stitched clips) at its
/// place on the output timeline. Overlong lines push later ones back rather
/// than overlapping.
pub fn build(manifest: &Manifest, segs: &[(u64, u64)], voice: &Voice, mut progress: impl FnMut(f32)) -> Result<Track> {
    let total_ms: u64 = segs.iter().map(|(a, b)| b - a).sum();
    let per_ms = OPUS_SAMPLE_RATE as u64 / 1000;
    let mut pcm = vec![0f32; (total_ms * per_ms) as usize];
    let mut captions = Vec::new();
    let mut free_at = 0u64;

    // Output position of each step: the first clip that contains it.
    let mut placed: Vec<(u64, &Step)> = Vec::new();
    let mut base = 0;
    for &(a, b) in segs {
        for s in &manifest.steps {
            if s.t_ms >= a && s.t_ms < b && !placed.iter().any(|(_, p)| p.id == s.id) {
                placed.push((base + s.t_ms.saturating_sub(LEAD_MS).max(a) - a, s));
            }
        }
        base += b - a;
    }
    placed.sort_by_key(|(t, _)| *t);

    for (i, (at, step)) in placed.iter().enumerate() {
        progress(i as f32 / placed.len().max(1) as f32);
        let text = spoken_text(step);
        if text.is_empty() {
            continue;
        }
        let audio = voice.speak(&text)?;
        let start = (*at).max(free_at);
        let len_ms = audio.len() as u64 / per_ms;
        let from = (start * per_ms) as usize;
        if from + audio.len() > pcm.len() {
            pcm.resize(from + audio.len(), 0.0);
        }
        for (dst, src) in pcm[from..].iter_mut().zip(&audio) {
            *dst += src;
        }
        captions.push((start, start + len_ms, step.text.replace("**", "").replace('`', "")));
        free_at = start + len_ms + GAP_MS;
    }
    progress(1.0);
    Ok(Track { pcm, captions })
}

/// Encode mono 48 kHz PCM as 20 ms Opus packets.
pub fn encode(pcm: &[f32]) -> Result<Vec<Vec<u8>>> {
    const FRAME: usize = OPUS_SAMPLE_RATE as usize / 50;
    let mut enc = opus::Encoder::new(OPUS_SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)?;
    enc.set_bitrate(opus::Bitrate::Bits(32_000))?;
    let mut out = Vec::with_capacity(pcm.len() / FRAME + 1);
    let mut buf = vec![0u8; 4000];
    let mut frame = vec![0f32; FRAME];
    for chunk in pcm.chunks(FRAME) {
        frame[..chunk.len()].copy_from_slice(chunk);
        frame[chunk.len()..].fill(0.0);
        let n = enc.encode_float(&frame, &mut buf)?;
        out.push(buf[..n].to_vec());
    }
    Ok(out)
}

fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let step = from as f64 / to as f64;
    let n = (input.len() as f64 / step) as usize;
    (0..n)
        .map(|i| {
            let p = i as f64 * step;
            let j = p as usize;
            let k = (p - j as f64) as f32;
            let a = input[j];
            let b = *input.get(j + 1).unwrap_or(&a);
            a * (1.0 - k) + b * k
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spoken_text_reads_naturally() {
        let mut s = Step::new(1, 0, StepKind::Typed);
        s.text = "Type `youtube.com{Enter}`.".into();
        assert_eq!(spoken_text(&s), "Type youtube.com, then press Enter.");
        s.kind = StepKind::Shortcut;
        s.text = "Press **Ctrl+S**.".into();
        assert_eq!(spoken_text(&s), "Press Ctrl plus S.");
    }

    #[test]
    fn encodes_twenty_ms_packets_and_resamples() {
        let pcm = vec![0.1f32; 48_000];
        assert_eq!(encode(&pcm).unwrap().len(), 50);
        assert_eq!(resample(&vec![0.0; 22_050], 22_050, 48_000).len(), 48_000);
    }
}
