//! Continuous screen video: poll one monitor at up to `fps`, skip frames that
//! are pixel-identical to the last encoded one, and encode the rest with
//! Cisco's OpenH264 binary loaded at runtime (the patent licence only covers
//! Cisco's own builds, so we never compile it in).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, FrameType, RateControlMode, UsageType,
};
use openh264::formats::{RgbaSliceU8, YUVBuffer};
use openh264::Timestamp;
use openscrib_core::mkv::{NAL_PPS, NAL_SPS, Track, VideoSpec, annexb_nals, avcc, nal_type, to_length_prefixed};
use openscrib_core::{Clock, MonitorInfo};
use tracing::{info, warn};
use xcap::Monitor;

use crate::media::Muxer;

/// The exact Cisco build `openh264` 0.9 verifies by hash.
pub const OPENH264_DLL: &str = "openh264-2.6.0-win64.dll";
/// Keyframe at least this often while anything is changing, for seeking.
const IDR_INTERVAL_MS: u64 = 10_000;

#[derive(Debug, Clone)]
pub struct VideoConfig {
    /// Monitor id; `None` records the primary monitor.
    pub monitor: Option<u32>,
    pub fps: u32,
    pub openh264: PathBuf,
}

#[derive(Debug, Clone)]
pub struct VideoStats {
    pub monitor: MonitorInfo,
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    pub first_ms: Option<u64>,
}

pub struct ScreenVideo {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<Result<VideoStats>>,
}

/// Look for the OpenH264 binary: `$OPENSCRIB_OPENH264`, then beside the
/// executable, then in a `runtime/` folder beside it or any ancestor (so dev
/// builds under `target/` find the repo's copy).
pub fn locate_openh264() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("OPENSCRIB_OPENH264").map(PathBuf::from) {
        return p.is_file().then_some(p);
    }
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent();
    let mut first = true;
    while let Some(d) = dir {
        if first && d.join(OPENH264_DLL).is_file() {
            return Some(d.join(OPENH264_DLL));
        }
        first = false;
        let candidate = d.join("runtime").join(OPENH264_DLL);
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

impl ScreenVideo {
    pub fn start(cfg: &VideoConfig, clock: Clock, muxer: Muxer) -> Result<Self> {
        let info = crate::screen::info(&find_monitor(cfg.monitor)?)?;
        let (width, height) = (info.rect.w & !1, info.rect.h & !1);
        if width == 0 || height == 0 {
            bail!("monitor {} has no usable size", info.id);
        }
        let encoder = new_encoder(&cfg.openh264, width, height, cfg.fps)?;
        info!("video: monitor {} \"{}\" {}x{} @ up to {} fps", info.id, info.name, width, height, cfg.fps);

        let stop = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            info,
            width,
            height,
            interval: Duration::from_millis(1000 / cfg.fps.max(1) as u64),
            clock,
            muxer,
            encoder,
            stop: stop.clone(),
        };
        let handle = thread::Builder::new().name("screen-video".into()).spawn(move || worker.run())?;
        Ok(Self { stop, handle })
    }

    pub fn stop(self) -> Result<VideoStats> {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.join().map_err(|_| anyhow!("video thread panicked"))?
    }
}

fn find_monitor(id: Option<u32>) -> Result<Monitor> {
    let all = Monitor::all().context("listing monitors")?;
    let found = match id {
        Some(id) => all.into_iter().find(|m| m.id().ok() == Some(id)),
        None => all.into_iter().find(|m| m.is_primary().unwrap_or(false)),
    };
    found.ok_or_else(|| match id {
        Some(id) => anyhow!("no monitor with id {id}"),
        None => anyhow!("no primary monitor"),
    })
}

fn new_encoder(dll: &Path, width: u32, height: u32, fps: u32) -> Result<Encoder> {
    let api = OpenH264API::from_blob_path(dll)
        .with_context(|| format!("loading {} (expected Cisco's {OPENH264_DLL})", dll.display()))?;
    // Screen content is mostly static; this budget only matters while
    // things move, and the frame skipping keeps idle time near free.
    let bps = (width as u64 * height as u64 * fps as u64 / 30).clamp(500_000, 6_000_000) as u32;
    let config = EncoderConfig::new()
        .usage_type(UsageType::ScreenContentRealTime)
        .rate_control_mode(RateControlMode::Bitrate)
        .bitrate(BitRate::from_bps(bps))
        .max_frame_rate(FrameRate::from_hz(fps as f32))
        // Lets rate control drop frames under pressure; the stream is VFR anyway.
        .skip_frames(true)
        .num_threads(4);
    Encoder::with_api_config(api, config).context("creating H.264 encoder")
}

struct Worker {
    info: MonitorInfo,
    width: u32,
    height: u32,
    interval: Duration,
    clock: Clock,
    muxer: Muxer,
    encoder: Encoder,
    stop: Arc<AtomicBool>,
}

impl Worker {
    fn run(mut self) -> Result<VideoStats> {
        // `Monitor` is not Send, so the worker looks it up again by id.
        let monitor = find_monitor(Some(self.info.id))?;
        let mut prev: Option<Vec<u8>> = None;
        let mut started = false;
        let mut last_idr: Option<u64> = None;
        let mut frames = 0u64;
        let mut first_ms = None;
        let mut capture_errors = 0u32;
        let mut next = Instant::now();

        while !self.stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            if now < next {
                thread::sleep(next - now);
            }
            next += self.interval;
            // Fell behind (slow capture or encode): don't try to catch up.
            if next < Instant::now() {
                next = Instant::now() + self.interval;
            }

            let t = self.clock.now_ms();
            let img = match monitor.capture_image() {
                Ok(img) => img,
                Err(e) => {
                    capture_errors += 1;
                    if capture_errors == 1 || capture_errors.is_multiple_of(100) {
                        warn!("video capture failed ({capture_errors}x): {e}");
                    }
                    continue;
                }
            };
            let rgba = crop(img.as_raw(), img.width(), img.height(), self.width, self.height);
            let Some(rgba) = rgba else {
                warn!("monitor resolution changed; video stops here");
                break;
            };
            if prev.as_deref() == Some(&rgba[..]) {
                continue;
            }

            if last_idr.is_some_and(|i| t - i >= IDR_INTERVAL_MS) {
                self.encoder.force_intra_frame();
            }
            let yuv = YUVBuffer::from_rgb_source(RgbaSliceU8::new(&rgba, (self.width as usize, self.height as usize)));
            let bs = self.encoder.encode_at(&yuv, Timestamp::from_millis(t)).context("encoding frame")?;
            let key = match bs.frame_type() {
                FrameType::Skip | FrameType::Invalid => continue,
                FrameType::IDR => true,
                _ => false,
            };
            let annexb = bs.to_vec();
            let nals = annexb_nals(&annexb);

            if !started {
                let sps = nals.iter().find(|n| nal_type(n) == NAL_SPS);
                let pps = nals.iter().find(|n| nal_type(n) == NAL_PPS);
                let (Some(sps), Some(pps)) = (sps, pps) else {
                    bail!("first H.264 frame has no SPS/PPS");
                };
                self.muxer.start_video(VideoSpec { width: self.width, height: self.height, avcc: avcc(sps, pps) })?;
                started = true;
                first_ms = Some(t);
            }
            if key {
                last_idr = Some(t);
            }
            self.muxer.push(Track::Video, t, key, to_length_prefixed(&nals));
            frames += 1;
            prev = Some(rgba);
        }

        Ok(VideoStats { monitor: self.info, width: self.width, height: self.height, frames, first_ms })
    }
}

/// Copy to even dimensions (H.264 4:2:0 needs them). `None` if the source is
/// smaller than the target, i.e. the monitor changed resolution.
fn crop(raw: &[u8], src_w: u32, src_h: u32, w: u32, h: u32) -> Option<Vec<u8>> {
    if src_w < w || src_h < h {
        return None;
    }
    if src_w == w && src_h == h {
        return Some(raw.to_vec());
    }
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h as usize {
        let start = row * src_w as usize * 4;
        out.extend_from_slice(&raw[start..start + w as usize * 4]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_odd_sizes() {
        let raw: Vec<u8> = (0..3 * 3 * 4).map(|i| i as u8).collect();
        let out = crop(&raw, 3, 3, 2, 2).unwrap();
        assert_eq!(out.len(), 16);
        assert_eq!(&out[8..12], &raw[12..16]);
        assert!(crop(&raw, 3, 3, 4, 2).is_none());
    }
}
