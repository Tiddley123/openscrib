//! The shared `screen.mkv` writer fed by the video and audio threads.
//!
//! The Matroska header needs the H.264 SPS/PPS, which only exist once the
//! first frame is encoded, so packets that arrive earlier (usually audio)
//! wait in the interleaver until the video track can be described.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use openscrib_core::mkv::{AudioSpec, Interleaver, MkvWriter, Track, VideoSpec};
use tracing::error;

/// Producers may lag each other by this much (video encode is the slow one).
const INTERLEAVE_WINDOW_MS: u64 = 2_000;
const APP: &str = concat!("OpenSCRIB ", env!("CARGO_PKG_VERSION"));

type Writer = MkvWriter<BufWriter<File>>;

struct Inner {
    file: Option<File>,
    audio: Option<AudioSpec>,
    writer: Option<Writer>,
    interleaver: Interleaver,
    pending: Vec<(Track, u64, bool, Vec<u8>)>,
    failed: bool,
}

#[derive(Clone)]
pub struct Muxer(Arc<Mutex<Inner>>);

impl Muxer {
    /// `expect_video`: whether to wait for [`Muxer::start_video`] before
    /// writing the header.
    pub fn create(path: &Path, expect_video: bool, audio: Option<AudioSpec>) -> Result<Self> {
        let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
        let mut inner = Inner {
            file: Some(file),
            audio,
            writer: None,
            interleaver: Interleaver::new(INTERLEAVE_WINDOW_MS),
            pending: Vec::new(),
            failed: false,
        };
        if !expect_video {
            inner.open(None)?;
        }
        Ok(Self(Arc::new(Mutex::new(inner))))
    }

    pub fn start_video(&self, spec: VideoSpec) -> Result<()> {
        self.0.lock().expect("muxer lock").open(Some(&spec))
    }

    /// Video was expected but could not start: write an audio-only header now
    /// so narration is still streamed to disk.
    pub fn start_without_video(&self) -> Result<()> {
        self.0.lock().expect("muxer lock").open(None)
    }

    /// Queue a packet. Write errors are logged once and further packets
    /// dropped, so a full disk never takes the input hooks down with it.
    pub fn push(&self, track: Track, t_ms: u64, keyframe: bool, data: Vec<u8>) {
        let mut g = self.0.lock().expect("muxer lock");
        let inner = &mut *g;
        if inner.failed {
            return;
        }
        let Some(w) = inner.writer.as_mut() else {
            inner.pending.push((track, t_ms, keyframe, data));
            return;
        };
        if let Err(e) = inner.interleaver.push(w, track, t_ms, keyframe, data) {
            error!("writing screen.mkv: {e}");
            inner.failed = true;
        }
    }

    pub fn finish(&self, duration_ms: u64) -> Result<()> {
        let mut g = self.0.lock().expect("muxer lock");
        let inner = &mut *g;
        if inner.writer.is_none() {
            // No video frame ever arrived; still produce a valid file.
            inner.open(None)?;
        }
        let mut w = inner.writer.take().expect("writer opened");
        inner.interleaver.flush(&mut w)?;
        w.finish(duration_ms)?;
        Ok(())
    }
}

impl Inner {
    fn open(&mut self, video: Option<&VideoSpec>) -> Result<()> {
        let Some(file) = self.file.take() else { return Ok(()) };
        let mut w = MkvWriter::new(BufWriter::new(file), video, self.audio.as_ref(), APP)?;
        for (track, t, key, data) in self.pending.drain(..) {
            // Without a video track, drop any stray video packets.
            if track == Track::Video && video.is_none() {
                continue;
            }
            self.interleaver.push(&mut w, track, t, key, data)?;
        }
        self.writer = Some(w);
        Ok(())
    }
}
