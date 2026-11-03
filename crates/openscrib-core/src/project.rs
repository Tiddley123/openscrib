//! Project storage. While recording, a project lives as a plain directory so
//! a crash loses at most the step in flight; `pack` turns it into the
//! `.scrib` zip that users move around.
//!
//! ```text
//! myguide.scrib (zip)
//! ├── project.json
//! ├── events.jsonl     # raw input log, for re-deriving steps later
//! ├── screen.mkv       # H.264 screen + Opus narration
//! └── frames/
//! ```

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::manifest::Manifest;

pub const PROJECT_EXT: &str = "scrib";
pub const MANIFEST_FILE: &str = "project.json";
pub const EVENTS_FILE: &str = "events.jsonl";
pub const FRAMES_DIR: &str = "frames";
pub const SCREEN_FILE: &str = "screen.mkv";

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error("invalid project.json: {0}")]
    Manifest(#[from] serde_json::Error),
    #[error(transparent)]
    Image(#[from] image::ImageError),
}

pub type Result<T> = std::result::Result<T, ProjectError>;

/// An unpacked project directory.
#[derive(Debug, Clone)]
pub struct ProjectDir {
    root: PathBuf,
}

impl ProjectDir {
    pub fn create(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join(FRAMES_DIR))?;
        Ok(Self { root })
    }

    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        if !root.join(MANIFEST_FILE).is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no {MANIFEST_FILE} in {}", root.display()),
            )
            .into());
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a manifest-relative path such as `frames/0003.png`.
    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// Manifest-relative name for a frame, e.g. `frames/0003_pre.png`.
    pub fn frame_name(step_id: u32, suffix: &str) -> String {
        format!("{FRAMES_DIR}/{step_id:04}{suffix}.png")
    }

    pub fn read_manifest(&self) -> Result<Manifest> {
        Ok(Manifest::from_json(&fs::read(self.root.join(MANIFEST_FILE))?)?)
    }

    /// Write via a temp file and rename, so a crash never leaves a torn manifest.
    pub fn write_manifest(&self, manifest: &Manifest) -> Result<()> {
        let tmp = self.root.join(format!("{MANIFEST_FILE}.tmp"));
        fs::write(&tmp, manifest.to_json()?)?;
        fs::rename(&tmp, self.root.join(MANIFEST_FILE))?;
        Ok(())
    }

    pub fn events_writer(&self) -> Result<BufWriter<File>> {
        let f = File::options()
            .create(true)
            .append(true)
            .open(self.root.join(EVENTS_FILE))?;
        Ok(BufWriter::new(f))
    }

    /// Zip the directory into a `.scrib` file. Already-compressed media is
    /// stored as-is; text is deflated.
    pub fn pack(&self, out: &Path) -> Result<()> {
        let tmp = out.with_extension(format!("{PROJECT_EXT}.tmp"));
        let mut zip = ZipWriter::new(BufWriter::new(File::create(&tmp)?));
        let mut files = Vec::new();
        collect_files(&self.root, &self.root, &mut files)?;
        files.sort();
        for rel in files {
            if rel.ends_with(".tmp") {
                continue;
            }
            let method = if is_compressed_media(&rel) {
                CompressionMethod::Stored
            } else {
                CompressionMethod::Deflated
            };
            let opts = SimpleFileOptions::default()
                .compression_method(method)
                .large_file(true);
            zip.start_file(rel.as_str(), opts)?;
            io::copy(&mut File::open(self.root.join(&rel))?, &mut zip)?;
        }
        zip.finish()?.flush()?;
        fs::rename(&tmp, out)?;
        Ok(())
    }

    /// Extract a `.scrib` file into `dir` (created if missing).
    pub fn unpack(file: &Path, dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        ZipArchive::new(File::open(file)?)?.extract(&dir)?;
        Self::open(dir)
    }
}

/// Read just the manifest from a `.scrib` file without extracting it.
pub fn read_packed_manifest(file: &Path) -> Result<Manifest> {
    let mut zip = ZipArchive::new(File::open(file)?)?;
    let mut buf = Vec::new();
    zip.by_name(MANIFEST_FILE)?.read_to_end(&mut buf)?;
    Ok(Manifest::from_json(&buf)?)
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            // Zip entries always use forward slashes.
            let rel: Vec<_> = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect();
            out.push(rel.join("/"));
        }
    }
    Ok(())
}

fn is_compressed_media(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".png", ".mkv", ".mp4", ".opus", ".ogg", ".jpg", ".webm"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Step, StepKind};

    #[test]
    fn pack_and_unpack_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = ProjectDir::create(tmp.path().join("work")).unwrap();

        let mut m = Manifest::new("test");
        let mut s = Step::new(1, 100, StepKind::Shortcut);
        s.keys = Some("Ctrl+S".into());
        s.frame = Some(ProjectDir::frame_name(1, ""));
        s.refresh_text();
        m.steps.push(s);
        dir.write_manifest(&m).unwrap();

        let img = image::RgbaImage::from_pixel(4, 2, image::Rgba([1, 2, 3, 255]));
        img.save(dir.path(&ProjectDir::frame_name(1, ""))).unwrap();

        let packed = tmp.path().join("out.scrib");
        dir.pack(&packed).unwrap();
        assert_eq!(read_packed_manifest(&packed).unwrap(), m);

        let back = ProjectDir::unpack(&packed, tmp.path().join("back")).unwrap();
        assert_eq!(back.read_manifest().unwrap(), m);
        let img_back = image::open(back.path("frames/0001.png")).unwrap().to_rgba8();
        assert_eq!(img_back, img);
    }
}
