//! The model catalogue and on-demand downloads into the portable `models/`
//! folder. Nothing here runs unless the user asks for a model; a missing
//! model just means that processing stage is skipped.

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    Whisper,
    OcrDetection,
    OcrRecognition,
    VisionModel,
    VisionProjector,
    /// Piper text-to-speech engine (a zip unpacked into `models/piper/`).
    TtsEngine,
    /// A Piper voice; its `.onnx.json` config is fetched alongside.
    TtsVoice,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ModelInfo {
    pub id: &'static str,
    pub kind: ModelKind,
    pub label: &'static str,
    pub note: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub size_mb: u32,
}

#[cfg(test)]
const WHISPER_BASE: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";
#[cfg(test)]
const OCR_BASE: &str = "https://ocrs-models.s3-accelerate.amazonaws.com/";

pub const DEFAULT_WHISPER: &str = "small.en";

pub const CATALOG: &[ModelInfo] = &[
    ModelInfo {
        id: "tiny.en",
        kind: ModelKind::Whisper,
        label: "Whisper tiny (English)",
        note: "Fastest, least accurate",
        file: "ggml-tiny.en.bin",
        url: concat!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/", "ggml-tiny.en.bin"),
        size_mb: 75,
    },
    ModelInfo {
        id: "base.en",
        kind: ModelKind::Whisper,
        label: "Whisper base (English)",
        note: "Fast, some mistakes on names",
        file: "ggml-base.en.bin",
        url: concat!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/", "ggml-base.en.bin"),
        size_mb: 142,
    },
    ModelInfo {
        id: "small.en",
        kind: ModelKind::Whisper,
        label: "Whisper small (English)",
        note: "Recommended balance",
        file: "ggml-small.en.bin",
        url: concat!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/", "ggml-small.en.bin"),
        size_mb: 466,
    },
    ModelInfo {
        id: "medium.en",
        kind: ModelKind::Whisper,
        label: "Whisper medium (English)",
        note: "More accurate, about 3x slower than small",
        file: "ggml-medium.en.bin",
        url: concat!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/", "ggml-medium.en.bin"),
        size_mb: 1500,
    },
    ModelInfo {
        id: "large-v3-turbo",
        kind: ModelKind::Whisper,
        label: "Whisper large v3 turbo (multilingual)",
        note: "Best accuracy, any language",
        file: "ggml-large-v3-turbo-q5_0.bin",
        url: concat!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/", "ggml-large-v3-turbo-q5_0.bin"),
        size_mb: 547,
    },
    ModelInfo {
        id: "qwen2.5-vl-3b",
        kind: ModelKind::VisionModel,
        label: "Qwen2.5-VL 3B",
        note: "Names icons and writes step text",
        file: "Qwen2.5-VL-3B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/ggml-org/Qwen2.5-VL-3B-Instruct-GGUF/resolve/main/Qwen2.5-VL-3B-Instruct-Q4_K_M.gguf",
        size_mb: 1930,
    },
    ModelInfo {
        id: "qwen2.5-vl-3b-vision",
        kind: ModelKind::VisionProjector,
        label: "Qwen2.5-VL 3B image encoder",
        note: "Lets the model see screenshots",
        file: "mmproj-Qwen2.5-VL-3B-Instruct-Q8_0.gguf",
        url: "https://huggingface.co/ggml-org/Qwen2.5-VL-3B-Instruct-GGUF/resolve/main/mmproj-Qwen2.5-VL-3B-Instruct-Q8_0.gguf",
        size_mb: 845,
    },
    ModelInfo {
        id: "piper",
        kind: ModelKind::TtsEngine,
        label: "Piper voice engine",
        note: "Reads step text aloud for video voiceovers",
        file: "piper/piper.exe",
        url: "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_windows_amd64.zip",
        size_mb: 22,
    },
    ModelInfo {
        id: "voice-en-us",
        kind: ModelKind::TtsVoice,
        label: "English (US) voice",
        note: "Lessac, clear and neutral",
        file: "en_US-lessac-medium.onnx",
        url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx",
        size_mb: 63,
    },
    ModelInfo {
        id: "voice-en-gb",
        kind: ModelKind::TtsVoice,
        label: "English (UK) voice",
        note: "Alba, Scottish English",
        file: "en_GB-alba-medium.onnx",
        url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_GB/alba/medium/en_GB-alba-medium.onnx",
        size_mb: 63,
    },
    ModelInfo {
        id: "ocr-detection",
        kind: ModelKind::OcrDetection,
        label: "OCR text detection",
        note: "Finds text on screen",
        file: "text-detection.onnx",
        url: "https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.onnx",
        size_mb: 3,
    },
    ModelInfo {
        id: "ocr-recognition",
        kind: ModelKind::OcrRecognition,
        label: "OCR text recognition",
        note: "Reads the text it finds",
        file: "text-recognition.onnx",
        url: "https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.onnx",
        size_mb: 10,
    },
];

pub fn find(id: &str) -> Option<&'static ModelInfo> {
    CATALOG.iter().find(|m| m.id == id)
}

/// `models/` next to the executable, or the nearest existing `models/` in
/// an ancestor folder (so development builds under `target/` share one).
pub fn models_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("OPENSCRIB_MODELS") {
        return PathBuf::from(p);
    }
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    let mut dir = exe_dir.as_deref();
    while let Some(d) = dir {
        let candidate = d.join("models");
        if candidate.is_dir() {
            return candidate;
        }
        dir = d.parent();
    }
    exe_dir.unwrap_or_default().join("models")
}

pub fn path(m: &ModelInfo) -> PathBuf {
    models_dir().join(m.file)
}

pub fn installed(m: &ModelInfo) -> bool {
    path(m).is_file()
}

/// The two OCR models, if both are present.
pub fn ocr_paths() -> Option<(PathBuf, PathBuf)> {
    let det = CATALOG.iter().find(|m| m.kind == ModelKind::OcrDetection)?;
    let rec = CATALOG.iter().find(|m| m.kind == ModelKind::OcrRecognition)?;
    (installed(det) && installed(rec)).then(|| (path(det), path(rec)))
}

/// The vision language model and its image projector, if both are present.
pub fn vision_paths() -> Option<(PathBuf, PathBuf)> {
    let model = CATALOG.iter().find(|m| m.kind == ModelKind::VisionModel)?;
    let proj = CATALOG.iter().find(|m| m.kind == ModelKind::VisionProjector)?;
    (installed(model) && installed(proj)).then(|| (path(model), path(proj)))
}

/// Download `m` into the models folder, reporting `(bytes, total)`.
/// Writes to a `.part` file first so an interrupted download never looks
/// installed.
pub fn download(m: &ModelInfo, mut progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf> {
    let dest = path(m);
    fs::create_dir_all(dest.parent().context("models folder")?)?;
    let part = dest.with_extension("part");
    let resp = ureq::get(m.url).call().with_context(|| format!("downloading {}", m.url))?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let mut reader = resp.into_body().into_reader();
    let mut out = BufWriter::new(File::create(&part)?);
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e).context("download interrupted"),
        };
        out.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total);
    }
    out.flush()?;
    drop(out);
    if let Some(t) = total
        && t != done
    {
        let _ = fs::remove_file(&part);
        bail!("download incomplete: got {done} of {t} bytes");
    }
    if m.url.ends_with(".zip") {
        // Unpack next to where `file` should end up, flattening the zip's top folder.
        let target = dest.parent().context("unpack folder")?.to_path_buf();
        let mut zip = zip::ZipArchive::new(File::open(&part)?)?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            let Some(name) = entry.enclosed_name() else { continue };
            let rel: PathBuf = name.components().skip(1).collect();
            if rel.as_os_str().is_empty() {
                continue;
            }
            let out = target.join(rel);
            if entry.is_dir() {
                fs::create_dir_all(&out)?;
            } else {
                fs::create_dir_all(out.parent().context("entry folder")?)?;
                io::copy(&mut entry, &mut File::create(&out)?)?;
            }
        }
        drop(zip);
        let _ = fs::remove_file(&part);
        if !dest.is_file() {
            bail!("{} was not in the download", m.file);
        }
        return Ok(dest);
    }
    fs::rename(&part, &dest)?;
    if m.kind == ModelKind::TtsVoice {
        // Piper needs the voice's config next to it.
        let cfg = ureq::get(&format!("{}.json", m.url)).call().context("downloading voice config")?;
        let mut json = Vec::new();
        cfg.into_body().into_reader().read_to_end(&mut json)?;
        fs::write(dest.with_extension("onnx.json"), json)?;
    }
    Ok(dest)
}

/// Piper and the chosen (or first installed) voice, if both are present.
pub fn tts_paths(voice_id: Option<&str>) -> Option<(PathBuf, PathBuf)> {
    let engine = CATALOG.iter().find(|m| m.kind == ModelKind::TtsEngine).filter(|m| installed(m))?;
    let voice = voice_id
        .and_then(find)
        .filter(|m| m.kind == ModelKind::TtsVoice && installed(m))
        .or_else(|| CATALOG.iter().find(|m| m.kind == ModelKind::TtsVoice && installed(m)))?;
    Some((path(engine), path(voice)))
}

pub fn remove(m: &ModelInfo) -> Result<()> {
    let p = path(m);
    if m.kind == ModelKind::TtsEngine {
        // The engine is a whole unpacked folder.
        if let Some(dir) = p.parent().filter(|d| d.is_dir()) {
            fs::remove_dir_all(dir)?;
        }
        return Ok(());
    }
    if p.exists() {
        fs::remove_file(&p)?;
    }
    let cfg = p.with_extension("onnx.json");
    if m.kind == ModelKind::TtsVoice && cfg.exists() {
        fs::remove_file(cfg)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_consistent() {
        for m in CATALOG {
            assert!(m.url.ends_with(".zip") || m.url.ends_with(m.file), "{} url/file mismatch", m.id);
            if m.kind == ModelKind::Whisper {
                assert!(m.url.starts_with(WHISPER_BASE));
            } else if matches!(m.kind, ModelKind::OcrDetection | ModelKind::OcrRecognition) && !m.url.contains("piper") {
                assert!(m.url.starts_with(OCR_BASE));
            }
        }
        assert!(find(DEFAULT_WHISPER).is_some());
    }
}
