//! Voiceover for exported video: each step's text read aloud, placed at that
//! step on the output timeline and encoded as the Opus audio track. The
//! speaking is done by a helper process: Piper, a small offline
//! text-to-speech engine downloaded on demand, on Windows and Linux, or the
//! speech synthesiser the system already ships: `say` on macOS, the WinRT
//! synthesiser on Windows. Either way `speak` hands back 48 kHz mono.

#[cfg(not(target_os = "macos"))]
use std::io::Read;
use std::io::Write;
#[cfg(not(target_os = "macos"))]
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use openscrib_core::mkv::OPUS_SAMPLE_RATE;
use openscrib_core::{Manifest, Step, StepKind};
use serde::Serialize;

/// What reads the steps aloud.
#[derive(Debug, Clone)]
pub enum Voice {
    /// Piper and one of its voices, both from the model catalogue.
    #[cfg(not(target_os = "macos"))]
    Piper {
        exe: PathBuf,
        /// Voice model (`.onnx`, with its `.onnx.json` next to it).
        model: PathBuf,
    },
    /// The system speech synthesiser.
    #[cfg(any(windows, target_os = "macos"))]
    System {
        /// A voice name from [`system_voices`], or `None` for the one chosen
        /// in the system's settings.
        name: Option<String>,
    },
}

/// A voice the system speech synthesiser offers, with nothing to download.
#[derive(Debug, Clone, Serialize)]
pub struct SystemVoice {
    /// What the synthesiser knows it by, and what settings store.
    pub name: String,
    /// Such as `en_AU`.
    pub locale: String,
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
    /// Synthesise `text` as 48 kHz mono.
    pub fn speak(&self, text: &str) -> Result<Vec<f32>> {
        // Both helpers take one line at a time.
        let line = text.replace('\n', " ");
        match self {
            #[cfg(not(target_os = "macos"))]
            Self::Piper { exe, model } => speak_piper(exe, model, &line),
            #[cfg(target_os = "macos")]
            Self::System { name } => speak_say(name.as_deref(), &line),
            #[cfg(windows)]
            Self::System { name } => winrt::speak(name.as_deref(), &line),
        }
    }

    /// The system voice called `name`, or the user's own default when that is
    /// empty or names a voice this machine doesn't have (settings outlive
    /// the voices they were saved with).
    #[cfg(any(windows, target_os = "macos"))]
    pub fn system(name: Option<&str>) -> Self {
        let installed = system_voices();
        let name = name
            .filter(|n| !n.is_empty() && installed.iter().any(|v| v.name == *n))
            .map(str::to_owned);
        Self::System { name }
    }
}

/// Voices the system speech synthesiser can read step text with. Empty
/// where there is none, and voiceover needs Piper and a voice instead.
pub fn system_voices() -> Vec<SystemVoice> {
    #[cfg(windows)]
    return winrt::voices().unwrap_or_default();
    #[cfg(not(any(windows, target_os = "macos")))]
    return Vec::new();
    #[cfg(target_os = "macos")]
    {
        let out = Command::new(SAY).arg("-v").arg("?").output();
        let out = match out {
            Ok(o) if o.status.success() => o.stdout,
            _ => return Vec::new(),
        };
        parse_voices(&String::from_utf8_lossy(&out))
    }
}

/// `/usr/bin/say` rather than `say`, so a `say` earlier on the PATH can't
/// stand in for it.
#[cfg(target_os = "macos")]
const SAY: &str = "/usr/bin/say";

/// Apple's novelty voices: sound effects and jokes rather than speech, so
/// never what someone wants reading a guide aloud.
#[cfg(target_os = "macos")]
const NOVELTY: &[&str] = &[
    "Albert", "Bad News", "Bahh", "Bells", "Boing", "Bubbles", "Cellos", "Good News", "Jester", "Organ", "Superstar",
    "Trinoids", "Whisper", "Wobble", "Zarvox",
];

/// Read `say -v '?'`, whose lines are `name`, locale, then `# a sample
/// sentence`. Names run from novelty ("Bad News") to parenthesised
/// ("Eddy (English (US))"), so the locale is taken as the last word before
/// the sample and everything before it is the name.
#[cfg(target_os = "macos")]
fn parse_voices(listing: &str) -> Vec<SystemVoice> {
    let mut out: Vec<SystemVoice> = listing
        .lines()
        .filter_map(|line| {
            let head = line.split('#').next()?.trim_end();
            let (name, locale) = head.rsplit_once(char::is_whitespace)?;
            let name = name.trim();
            if name.is_empty() || !locale.contains('_') || NOVELTY.contains(&name) {
                return None;
            }
            Some(SystemVoice { name: name.to_owned(), locale: locale.to_owned() })
        })
        .collect();
    out.sort_by(|a, b| a.locale.cmp(&b.locale).then_with(|| a.name.cmp(&b.name)));
    out.dedup_by(|a, b| a.name == b.name && a.locale == b.locale);
    out
}

/// Speak through the macOS synthesiser. `say` can only write WAVE to a
/// seekable file, so each line goes via a temp file; asking it for the output
/// rate up front means nothing has to be resampled.
#[cfg(target_os = "macos")]
fn speak_say(name: Option<&str>, text: &str) -> Result<Vec<f32>> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    let wav = std::env::temp_dir().join(format!(
        "openscrib-say-{}-{}.wav",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let spoken = (|| -> Result<Vec<f32>> {
        let mut cmd = Command::new(SAY);
        if let Some(name) = name {
            cmd.arg("-v").arg(name);
        }
        // `-f -` reads the text from stdin, so no line can look like a flag.
        cmd.arg("-f").arg("-");
        cmd.arg("--file-format=WAVE").arg(format!("--data-format=LEI16@{OPUS_SAMPLE_RATE}"));
        cmd.arg("-o").arg(&wav);
        cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
        let mut child = cmd.spawn().with_context(|| format!("starting {SAY}"))?;
        child.stdin.take().context("say stdin")?.write_all(text.as_bytes())?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            let why = String::from_utf8_lossy(&out.stderr);
            bail!("the system voice failed: {}", why.trim());
        }
        let (samples, rate) = read_wav(&std::fs::read(&wav)?)?;
        Ok(resample(&samples, rate, OPUS_SAMPLE_RATE))
    })();
    let _ = std::fs::remove_file(&wav);
    spoken
}

/// Mono samples and their rate from a 16-bit PCM WAVE file. The chunks are
/// walked rather than assumed: `say` pads the header with a filler chunk
/// before the samples.
#[cfg(any(windows, target_os = "macos"))]
fn read_wav(bytes: &[u8]) -> Result<(Vec<f32>, u32)> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("the system voice wrote something that isn't a WAVE file");
    }
    let word = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let mut pos = 12;
    let mut rate = 0u32;
    let mut channels = 1usize;
    while pos + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        let end = bytes.len().min(body + len);
        match &bytes[pos..pos + 4] {
            b"fmt " if len >= 16 => {
                channels = word(body + 2).max(1) as usize;
                rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().unwrap());
                let bits = word(body + 14);
                if bits != 16 {
                    bail!("expected 16-bit audio from the system voice, got {bits}-bit");
                }
            }
            b"data" => {
                let samples = bytes[body..end]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .step_by(channels)
                    .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
                    .collect();
                if rate == 0 {
                    bail!("the system voice wrote no format header");
                }
                return Ok((samples, rate));
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        pos = body + len + (len & 1);
    }
    bail!("the system voice wrote no audio")
}

/// The Windows speech synthesiser. This is the WinRT one rather than SAPI
/// because it also reaches the OneCore voices, which is where the regional
/// English voices live (Catherine and James for Australia, for a start).
#[cfg(windows)]
mod winrt {
    use anyhow::{Context, Result};
    use openscrib_core::mkv::OPUS_SAMPLE_RATE;
    use windows::Media::SpeechSynthesis::SpeechSynthesizer;
    use windows::Storage::Streams::DataReader;
    use windows::core::HSTRING;

    use super::{SystemVoice, read_wav, resample};

    pub fn voices() -> Result<Vec<SystemVoice>> {
        let mut out = Vec::new();
        for v in SpeechSynthesizer::AllVoices()? {
            out.push(SystemVoice { name: v.DisplayName()?.to_string(), locale: v.Language()?.to_string().replace('-', "_") });
        }
        out.sort_by(|a, b| a.locale.cmp(&b.locale).then_with(|| a.name.cmp(&b.name)));
        Ok(out)
    }

    pub fn speak(name: Option<&str>, text: &str) -> Result<Vec<f32>> {
        let synth = SpeechSynthesizer::new().context("starting the Windows speech synthesiser")?;
        if let Some(name) = name {
            let found = SpeechSynthesizer::AllVoices()?.into_iter().find(|v| v.DisplayName().is_ok_and(|n| n == name));
            if let Some(voice) = found {
                synth.SetVoice(&voice)?;
            }
        }
        let stream = synth.SynthesizeTextToStreamAsync(&HSTRING::from(text))?.join().context("speaking with the Windows voice")?;
        let size = stream.Size()? as u32;
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
        reader.LoadAsync(size)?.join()?;
        let mut wav = vec![0u8; size as usize];
        reader.ReadBytes(&mut wav)?;
        let (samples, rate) = read_wav(&wav)?;
        Ok(resample(&samples, rate, OPUS_SAMPLE_RATE))
    }
}

/// Speak through Piper, which writes raw PCM at its voice's own rate.
#[cfg(not(target_os = "macos"))]
fn speak_piper(exe: &std::path::Path, model: &std::path::Path, text: &str) -> Result<Vec<f32>> {
    // Piper voices state their rate in the config; 22050 is the common one.
    let rate = std::fs::read_to_string(model.with_extension("onnx.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["audio"]["sample_rate"].as_u64())
        .unwrap_or(22_050) as u32;

    let mut cmd = Command::new(exe);
    cmd.arg("--model").arg(model).arg("--output_raw");
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().with_context(|| format!("starting {}", exe.display()))?;
    {
        let mut stdin = child.stdin.take().context("piper stdin")?;
        stdin.write_all(text.as_bytes())?;
        stdin.write_all(b"\n")?;
    }
    let mut raw = Vec::new();
    child.stdout.take().context("piper stdout")?.read_to_end(&mut raw)?;
    if !child.wait()?.success() {
        bail!("the voice engine failed");
    }
    let samples: Vec<f32> = raw.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c) as f32 / 32768.0).collect();
    Ok(resample(&samples, rate, OPUS_SAMPLE_RATE))
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
        for s in manifest.steps.iter().filter(|s| !s.deleted) {
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

    #[cfg(target_os = "macos")]
    #[test]
    fn lists_system_voices_without_the_novelties() {
        let listing = "\
Samantha            en_US    # Hello! My name is Samantha.
Bad News            en_US    # Hello! My name is Bad News.
Eddy (English (UK)) en_GB    # Hello! My name is Eddy.
Majed               ar_001   # \u{645}\u{631}\u{62d}\u{628}\u{64b}\u{627}!
nonsense
";
        let voices = parse_voices(listing);
        assert_eq!(
            voices.iter().map(|v| (v.name.as_str(), v.locale.as_str())).collect::<Vec<_>>(),
            [("Majed", "ar_001"), ("Eddy (English (UK))", "en_GB"), ("Samantha", "en_US")]
        );
        // The real list from this Mac has usable voices and no novelties.
        let real = system_voices();
        assert!(real.iter().any(|v| v.locale.starts_with("en")), "no English voice: {real:?}");
        assert!(!real.iter().any(|v| v.name == "Bells"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn system_voice_speaks_and_falls_back() {
        // An unknown name leaves the user's own default in place.
        assert!(matches!(Voice::system(Some("Nobody At All")), Voice::System { name: None }));
        let named = system_voices().first().map(|v| v.name.clone()).expect("a voice");
        assert!(matches!(Voice::system(Some(&named)), Voice::System { name: Some(n) } if n == named));

        let pcm = Voice::system(None).speak("One. Two. Three.").expect("spoke");
        let secs = pcm.len() as f32 / OPUS_SAMPLE_RATE as f32;
        assert!((0.5..10.0).contains(&secs), "{secs}s of audio");
        assert!(pcm.iter().any(|s| s.abs() > 0.01), "silence");
    }

    #[cfg(windows)]
    #[test]
    fn windows_voice_speaks() {
        let voices = system_voices();
        assert!(voices.iter().all(|v| v.locale.contains('_')), "{voices:?}");
        assert!(voices.iter().any(|v| v.locale.starts_with("en")), "no English voice: {voices:?}");
        for voice in [None, voices.first().map(|v| v.name.as_str())] {
            let pcm = Voice::system(voice).speak("One. Two. Three.").expect("spoke");
            let secs = pcm.len() as f32 / OPUS_SAMPLE_RATE as f32;
            assert!((0.5..10.0).contains(&secs), "{secs}s of audio");
            assert!(pcm.iter().any(|s| s.abs() > 0.01), "silence");
        }
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn reads_wav_past_the_filler_chunk() {
        let mut wav = b"RIFF\0\0\0\0WAVE".to_vec();
        wav.extend(b"fmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes()); // PCM
        wav.extend(1u16.to_le_bytes()); // mono
        wav.extend(48_000u32.to_le_bytes());
        wav.extend(96_000u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"FLLR");
        wav.extend(3u32.to_le_bytes());
        wav.extend([0, 0, 0, 0]); // 3 bytes plus the pad byte
        wav.extend(b"data");
        wav.extend(4u32.to_le_bytes());
        wav.extend(i16::MAX.to_le_bytes());
        wav.extend(i16::MIN.to_le_bytes());
        let (pcm, rate) = read_wav(&wav).unwrap();
        assert_eq!(rate, 48_000);
        assert_eq!(pcm.len(), 2);
        assert!(pcm[0] > 0.99 && pcm[1] < -0.99, "{pcm:?}");
        assert!(read_wav(b"not a wav at all").is_err());
    }

    #[test]
    fn encodes_twenty_ms_packets_and_resamples() {
        let pcm = vec![0.1f32; 48_000];
        assert_eq!(encode(&pcm).unwrap().len(), 50);
        assert_eq!(resample(&vec![0.0; 22_050], 22_050, 48_000).len(), 48_000);
    }
}
