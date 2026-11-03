//! Transcribe a 16 kHz mono 16-bit WAV with the installed Whisper model:
//! `cargo run --release -p openscrib-process --example transcribe_wav -- speech.wav`
//! Useful for checking a model without recording a project.

use anyhow::{Context, Result, bail};
use openscrib_process::audio::Narration;
use openscrib_process::{ProcessOptions, transcribe};

fn main() -> Result<()> {
    let path = std::env::args().nth(1).context("usage: transcribe_wav <file.wav>")?;
    let bytes = std::fs::read(&path)?;
    // Minimal RIFF reader: find the "data" chunk of 16-bit PCM.
    let pos = bytes.windows(4).position(|w| w == b"data").context("no data chunk")?;
    let pcm = &bytes[pos + 8..];
    let rate = u32::from_le_bytes(bytes[24..28].try_into()?);
    if rate != 16_000 {
        bail!("expected 16 kHz audio, got {rate} Hz");
    }
    let samples = pcm.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c) as f32 / 32768.0).collect();
    let opts = ProcessOptions::installed(None);
    let model = opts.whisper.context("no Whisper model installed")?;
    let started = std::time::Instant::now();
    for s in transcribe::transcribe(&model, &Narration { start_ms: 0, samples }, opts.english_only)? {
        println!("{:>6} - {:>6} ms  {}", s.start_ms, s.end_ms, s.text);
    }
    eprintln!("({:.1}s)", started.elapsed().as_secs_f64());
    Ok(())
}
