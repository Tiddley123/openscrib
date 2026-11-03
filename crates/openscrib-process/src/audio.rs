//! Narration as 16 kHz mono PCM for Whisper, decoded from the Opus track of
//! `screen.mkv`.

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result};
use matroska_demuxer::{Frame, MatroskaFile};

pub const WHISPER_RATE: u32 = 16_000;

pub struct Narration {
    /// Recording time of the first sample, in ms.
    pub start_ms: u64,
    pub samples: Vec<f32>,
}

impl Narration {
    pub fn duration_ms(&self) -> u64 {
        self.samples.len() as u64 * 1000 / WHISPER_RATE as u64
    }

    /// Root-mean-square level; near zero means nobody spoke (or the mic was
    /// muted), and Whisper tends to invent words for silence.
    pub fn rms(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        (self.samples.iter().map(|s| s * s).sum::<f32>() / self.samples.len() as f32).sqrt()
    }
}

/// Decode the audio track, or `None` if the recording has none.
pub fn decode(mkv: &Path) -> Result<Option<Narration>> {
    let mut file = MatroskaFile::open(File::open(mkv).with_context(|| format!("opening {}", mkv.display()))?)?;
    let Some(track) = file.tracks().iter().find(|t| t.codec_id() == "A_OPUS").map(|t| t.track_number().get()) else {
        return Ok(None);
    };
    let mut decoder = opus::Decoder::new(48_000, opus::Channels::Mono)?;
    let mut pcm48: Vec<f32> = Vec::new();
    let mut buf = vec![0f32; 5760];
    let mut start_ms = None;
    let mut frame = Frame::default();
    while file.next_frame(&mut frame)? {
        if frame.track != track {
            continue;
        }
        start_ms.get_or_insert(frame.timestamp);
        let n = decoder.decode_float(&frame.data, &mut buf, false).context("decoding narration")?;
        pcm48.extend_from_slice(&buf[..n]);
    }
    let Some(start_ms) = start_ms else { return Ok(None) };
    Ok(Some(Narration { start_ms, samples: downsample_48k_to_16k(&pcm48) }))
}

/// 3:1 decimation with a short low-pass, good enough for speech recognition.
fn downsample_48k_to_16k(input: &[f32]) -> Vec<f32> {
    const TAPS: [f32; 5] = [0.1, 0.2, 0.4, 0.2, 0.1];
    let at = |i: isize| input.get(i.max(0) as usize).copied().unwrap_or(0.0);
    (0..input.len() / 3)
        .map(|o| {
            let c = (o * 3 + 1) as isize;
            TAPS.iter().enumerate().map(|(k, w)| w * at(c + k as isize - 2)).sum()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsampling_keeps_length_and_low_tones() {
        // 200 Hz tone survives; length becomes a third.
        let tone: Vec<f32> = (0..48_000).map(|i| (i as f32 * 200.0 * std::f32::consts::TAU / 48_000.0).sin()).collect();
        let out = downsample_48k_to_16k(&tone);
        assert_eq!(out.len(), 16_000);
        let peak = out.iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.9, "peak {peak}");
    }
}
