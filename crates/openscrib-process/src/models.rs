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
    fs::rename(&part, &dest)?;
    Ok(dest)
}

pub fn remove(m: &ModelInfo) -> Result<()> {
    let p = path(m);
    if p.exists() {
        fs::remove_file(p)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_consistent() {
        for m in CATALOG {
            assert!(m.url.ends_with(m.file), "{} url/file mismatch", m.id);
            if m.kind == ModelKind::Whisper {
                assert!(m.url.starts_with(WHISPER_BASE));
            } else {
                assert!(m.url.starts_with(OCR_BASE));
            }
        }
        assert!(find(DEFAULT_WHISPER).is_some());
    }
}
