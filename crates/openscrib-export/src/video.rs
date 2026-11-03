//! Video export: decode `screen.mkv`, cut it to the steps' clips (or keep
//! the whole recording), draw the cursor and click ripples from the input
//! log, re-encode, and write MKV or faststart MP4. Narration packets are
//! copied without re-encoding.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use image::imageops;
use image::RgbaImage;
use matroska_demuxer::{Frame, MatroskaFile};
use openh264::decoder::{Decoder, DecoderConfig};
use openh264::encoder::{Encoder, EncoderConfig, FrameType, RateControlMode, UsageType};
use openh264::formats::{RgbaSliceU8, YUVBuffer, YUVSource};
use openh264::{OpenH264API, Timestamp};
use openscrib_core::mkv::{
    AudioSpec, MkvWriter, NAL_PPS, NAL_SPS, Track, VideoSpec, annexb_nals, avcc, nal_type, to_length_prefixed,
};
use openscrib_core::mp4::Mp4Writer;
use openscrib_core::project::{EVENTS_FILE, SCREEN_FILE};
use openscrib_core::{Manifest, Point, ProjectDir, quality};

use crate::render::stroke_segment;

const RIPPLE_MS: u64 = 450;
const IDR_INTERVAL_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoFormat {
    Mkv,
    Mp4,
}

#[derive(Debug, Clone)]
pub struct VideoExportOptions {
    pub openh264: PathBuf,
    pub format: VideoFormat,
    /// Scale down to at most this width (1920 plays everywhere).
    pub max_width: u32,
    /// Output frames are only emitted when something changed, at most this often.
    pub max_fps: u32,
    pub cursor: bool,
    pub clicks: bool,
    /// 1..=100, see `openscrib_core::quality`.
    pub quality: u8,
}

impl VideoExportOptions {
    pub fn new(openh264: PathBuf, format: VideoFormat) -> Self {
        Self { openh264, format, max_width: 1920, max_fps: 15, cursor: true, clicks: true, quality: quality::DEFAULT_QUALITY }
    }
}

#[derive(Debug, Clone)]
pub struct VideoExportStats {
    pub duration_ms: u64,
    pub frames: u64,
    pub segments: usize,
    pub width: u32,
    pub height: u32,
}

/// The source ranges to stitch, in output order: each step's clip in step
/// order, or the whole recording when no step has one.
pub fn segments(manifest: &Manifest) -> Vec<(u64, u64)> {
    let clips: Vec<_> = manifest
        .steps
        .iter()
        .filter_map(|s| s.clip)
        .filter(|c| c.end_ms > c.start_ms)
        .map(|c| (c.start_ms, c.end_ms.min(manifest.duration_ms)))
        .filter(|(a, b)| b > a)
        .collect();
    if !clips.is_empty() {
        return clips;
    }
    let start = manifest.video.as_ref().map_or(0, |v| v.first_ms);
    vec![(start, manifest.duration_ms)]
}

/// What an output frame shows: source frame, rounded cursor position, and
/// whether a click ripple is animating. Unchanged keys are not re-encoded.
type FrameKey = (Option<usize>, Option<(i32, i32)>, bool);

struct Packet {
    t: u64,
    key: bool,
    data: Vec<u8>,
}

struct Pointer {
    moves: Vec<(u64, Point)>,
    presses: Vec<(u64, Point)>,
}

impl Pointer {
    fn load(path: &Path) -> Result<Self> {
        let mut moves = Vec::new();
        let mut presses = Vec::new();
        if !path.exists() {
            return Ok(Self { moves, presses });
        }
        let mut last = None;
        for line in BufReader::new(File::open(path)?).lines() {
            let v: serde_json::Value = match serde_json::from_str(&line?) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let t = v["t"].as_u64().unwrap_or(0);
            match v["ev"].as_str() {
                Some("move") => {
                    let p = Point::new(v["x"].as_i64().unwrap_or(0) as i32, v["y"].as_i64().unwrap_or(0) as i32);
                    moves.push((t, p));
                    last = Some(p);
                }
                Some("press") => {
                    if let Some(p) = last {
                        presses.push((t, p));
                    }
                }
                _ => {}
            }
        }
        Ok(Self { moves, presses })
    }

    /// Cursor position at `t`, interpolated between logged moves.
    fn at(&self, t: u64) -> Option<(f32, f32)> {
        let i = self.moves.partition_point(|(mt, _)| *mt <= t);
        let (t0, p0) = *self.moves.get(i.checked_sub(1)?)?;
        let Some(&(t1, p1)) = self.moves.get(i) else { return Some((p0.x as f32, p0.y as f32)) };
        // Only glide across short gaps; a long gap means the mouse sat still.
        if t1 - t0 > 100 {
            return Some((p0.x as f32, p0.y as f32));
        }
        let k = (t - t0) as f32 / (t1 - t0).max(1) as f32;
        Some((p0.x as f32 + (p1.x - p0.x) as f32 * k, p0.y as f32 + (p1.y - p0.y) as f32 * k))
    }

    /// Clicks whose ripple is visible at `t`, with their progress 0..1.
    fn ripples(&self, t: u64) -> impl Iterator<Item = (Point, f32)> + '_ {
        let from = self.presses.partition_point(|(pt, _)| *pt + RIPPLE_MS < t);
        self.presses[from..]
            .iter()
            .take_while(move |(pt, _)| *pt <= t)
            .map(move |(pt, p)| (*p, (t - pt) as f32 / RIPPLE_MS as f32))
    }
}

enum Sink {
    Mkv(MkvWriter<BufWriter<File>>),
    Mp4(Mp4Writer),
}

impl Sink {
    fn write(&mut self, track: Track, t: u64, key: bool, data: &[u8]) -> Result<()> {
        match self {
            Sink::Mkv(w) => w.write(track, t, key, data)?,
            Sink::Mp4(w) => w.write(track, t, key, data)?,
        }
        Ok(())
    }
}

pub fn export_video(
    dir: &ProjectDir,
    manifest: &Manifest,
    opts: &VideoExportOptions,
    out: &Path,
    mut progress: impl FnMut(f32),
) -> Result<VideoExportStats> {
    let video_info = manifest.video.as_ref().context("this project has no screen video")?;
    let origin = manifest.monitor(video_info.monitor).map_or(Point::new(0, 0), |m| m.rect.origin());
    let ui_scale = manifest.monitor(video_info.monitor).map_or(1.0, |m| m.scale).max(1.0);
    let accent = manifest.accent_rgb();

    let (video, audio) = demux(&dir.path(SCREEN_FILE))?;
    if video.is_empty() {
        bail!("screen.mkv has no video frames");
    }
    let pointer = Pointer::load(&dir.path(EVENTS_FILE))?;
    let segs = segments(manifest);

    let (src_w, src_h) = (video_info.width, video_info.height);
    let scale = (opts.max_width as f32 / src_w as f32).min(1.0);
    let out_w = ((src_w as f32 * scale) as u32) & !1;
    let out_h = ((src_h as f32 * scale) as u32) & !1;

    let api = || OpenH264API::from_blob_path(&opts.openh264).with_context(|| format!("loading {}", opts.openh264.display()));
    let mut decoder = Decoder::with_api_config(api()?, DecoderConfig::new()).context("creating decoder")?;
    let (qp_min, qp_max) = quality::qp_range(opts.quality);
    let config = EncoderConfig::new()
        // Cisco's build rejects ScreenContentNonRealTime ("invalid usage type").
        .usage_type(UsageType::ScreenContentRealTime)
        .rate_control_mode(RateControlMode::Quality)
        .bitrate(openh264::encoder::BitRate::from_bps(quality::bitrate(opts.quality, out_w, out_h, opts.max_fps)))
        .qp(openh264::encoder::QpRange::new(qp_min, qp_max))
        .max_frame_rate(openh264::encoder::FrameRate::from_hz(opts.max_fps as f32))
        .skip_frames(false);
    let mut encoder = Encoder::with_api_config(api()?, config).context("creating encoder")?;

    let audio_spec = (!audio.is_empty()).then(|| AudioSpec { channels: manifest.audio.as_ref().map_or(1, |a| a.channels) });
    // Audio packets remapped onto the output timeline, written as video
    // catches up so the file stays interleaved.
    let mut out_audio = Vec::new();
    let mut offset = 0u64;
    for &(a, b) in &segs {
        for p in audio.iter().filter(|p| p.t >= a && p.t < b) {
            out_audio.push((offset + p.t - a, &p.data));
        }
        offset += b - a;
    }
    let total_ms = offset;
    let mut audio_next = 0;

    let tmp_mp4 = out.with_extension("mp4.part");
    let mut sink: Option<Sink> = None;
    let mut pending_header = audio_spec.clone();

    // SIMD scaling; the image crate's scalar resize dominated export time.
    let mut resizer = fast_image_resize::Resizer::new();
    let resize_opts = fast_image_resize::ResizeOptions::new()
        .resize_alg(fast_image_resize::ResizeAlg::Convolution(fast_image_resize::FilterType::Bilinear))
        .use_alpha(false);
    let step_ms = 1000 / opts.max_fps.max(1) as u64;
    let mut cur: Option<usize> = None; // index of the last decoded source frame
    let mut src_rgba: Option<RgbaImage> = None;
    let mut last_emit: Option<FrameKey> = None;
    let mut last_idr: Option<u64> = None;
    let mut frames = 0u64;
    let mut out_base = 0u64;

    for &(seg_a, seg_b) in &segs {
        let mut t = seg_a;
        let mut first_in_seg = true;
        while t < seg_b {
            // Decode up to the source frame showing at `t`.
            let target = video.partition_point(|f| f.t <= t).checked_sub(1);
            if let Some(target) = target
                && cur != Some(target)
            {
                let start = match cur {
                    Some(c) if c < target => c + 1,
                    _ => video[..=target].iter().rposition(|f| f.key).unwrap_or(0),
                };
                for (i, f) in video.iter().enumerate().take(target + 1).skip(start) {
                    if let Some(yuv) = decoder.decode(&to_annexb(&f.data)).context("decoding source")?
                        && i == target
                    {
                        let (w, h) = yuv.dimensions();
                        let mut buf = vec![0u8; w * h * 4];
                        yuv.write_rgba8(&mut buf);
                        src_rgba = RgbaImage::from_raw(w as u32, h as u32, buf);
                    }
                }
                cur = Some(target);
            }

            let cursor = if opts.cursor {
                pointer.at(t).map(|(x, y)| ((x - origin.x as f32) * scale, (y - origin.y as f32) * scale))
            } else {
                None
            };
            let rippling = opts.clicks && pointer.ripples(t).next().is_some();
            let key = (cur, cursor.map(|(x, y)| (x.round() as i32, y.round() as i32)), rippling);
            let changed = last_emit.as_ref() != Some(&key) || rippling;

            if (changed || first_in_seg)
                && let Some(src) = &src_rgba
            {
                let mut frame = if scale < 1.0 {
                    let mut dst = RgbaImage::new(out_w, out_h);
                    resizer.resize(src, &mut dst, &resize_opts).context("scaling frame")?;
                    dst
                } else {
                    imageops::crop_imm(src, 0, 0, out_w, out_h).to_image()
                };
                if opts.clicks {
                    for (p, k) in pointer.ripples(t) {
                        let at = ((p.x - origin.x) as f32 * scale, (p.y - origin.y) as f32 * scale);
                        draw_ripple(&mut frame, at, k, ui_scale, accent);
                    }
                }
                if let Some(at) = cursor {
                    draw_cursor(&mut frame, at, ui_scale * 1.3);
                }

                let out_t = out_base + (t - seg_a);
                if first_in_seg || last_idr.is_none_or(|i| out_t - i >= IDR_INTERVAL_MS) {
                    encoder.force_intra_frame();
                }
                let yuv = YUVBuffer::from_rgb_source(RgbaSliceU8::new(frame.as_raw(), (out_w as usize, out_h as usize)));
                let bs = encoder.encode_at(&yuv, Timestamp::from_millis(out_t)).context("encoding output frame")?;
                let idr = match bs.frame_type() {
                    FrameType::Skip | FrameType::Invalid => None,
                    FrameType::IDR => Some(true),
                    _ => Some(false),
                };
                if let Some(idr) = idr {
                    let annexb = bs.to_vec();
                    let nals = annexb_nals(&annexb);
                    if sink.is_none() {
                        let sps = nals.iter().find(|n| nal_type(n) == NAL_SPS).context("no SPS in first frame")?;
                        let pps = nals.iter().find(|n| nal_type(n) == NAL_PPS).context("no PPS in first frame")?;
                        let spec = VideoSpec { width: out_w, height: out_h, avcc: avcc(sps, pps) };
                        sink = Some(match opts.format {
                            VideoFormat::Mkv => Sink::Mkv(MkvWriter::new(
                                BufWriter::new(File::create(out)?),
                                Some(&spec),
                                pending_header.as_ref(),
                                concat!("OpenSCRIB ", env!("CARGO_PKG_VERSION")),
                            )?),
                            VideoFormat::Mp4 => Sink::Mp4(Mp4Writer::new(&tmp_mp4, Some(spec), pending_header.take())?),
                        });
                    }
                    let s = sink.as_mut().expect("sink created");
                    while let Some((at, data)) = out_audio.get(audio_next).filter(|(at, _)| *at <= out_t) {
                        s.write(Track::Audio, *at, true, data)?;
                        audio_next += 1;
                    }
                    s.write(Track::Video, out_t, idr, &to_length_prefixed(&nals))?;
                    if idr {
                        last_idr = Some(out_t);
                    }
                    frames += 1;
                }
                last_emit = Some(key);
                first_in_seg = false;
            }
            t += step_ms;
            progress(((out_base + (t - seg_a).min(seg_b - seg_a)) as f32 / total_ms.max(1) as f32).min(1.0));
        }
        out_base += seg_b - seg_a;
    }

    let mut sink = sink.ok_or_else(|| anyhow!("no frames to export"))?;
    for (at, data) in &out_audio[audio_next..] {
        sink.write(Track::Audio, *at, true, data)?;
    }
    match sink {
        Sink::Mkv(w) => {
            w.finish(total_ms)?;
        }
        Sink::Mp4(w) => w.finish(out, total_ms)?,
    }
    if tmp_mp4.exists() {
        let _ = fs::remove_file(&tmp_mp4);
    }
    progress(1.0);
    Ok(VideoExportStats { duration_ms: total_ms, frames, segments: segs.len(), width: out_w, height: out_h })
}

/// Copy `screen.mkv` into an MP4 without re-encoding, for players (such as
/// the editor's WebView) that can't open Matroska. Takes about a second.
pub fn remux_to_mp4(mkv: &Path, out: &Path) -> Result<()> {
    let file = MatroskaFile::open(File::open(mkv).with_context(|| format!("opening {}", mkv.display()))?)?;
    let mut video_spec = None;
    let mut audio_spec = None;
    for t in file.tracks() {
        match t.codec_id() {
            "V_MPEG4/ISO/AVC" => {
                let v = t.video().context("video track without dimensions")?;
                video_spec = Some(VideoSpec {
                    width: v.pixel_width().get() as u32,
                    height: v.pixel_height().get() as u32,
                    avcc: t.codec_private().context("video track without avcC")?.to_vec(),
                });
            }
            "A_OPUS" => {
                let channels = t.audio().map_or(1, |a| a.channels().get() as u8);
                audio_spec = Some(AudioSpec { channels });
            }
            _ => {}
        }
    }
    let duration = file.info().duration().unwrap_or(0.0) as u64;
    drop(file);
    let (video, audio) = demux(mkv)?;
    let tmp = out.with_extension("mp4.part");
    let mut w = Mp4Writer::new(&tmp, video_spec, audio_spec)?;
    let (mut vi, mut ai) = (0, 0);
    // Merge by timestamp so the file is interleaved.
    while vi < video.len() || ai < audio.len() {
        let take_video = ai >= audio.len() || (vi < video.len() && video[vi].t <= audio[ai].t);
        if take_video {
            w.write(Track::Video, video[vi].t, video[vi].key, &video[vi].data)?;
            vi += 1;
        } else {
            w.write(Track::Audio, audio[ai].t, true, &audio[ai].data)?;
            ai += 1;
        }
    }
    let last = video.last().map(|p| p.t).max(audio.last().map(|p| p.t)).unwrap_or(0);
    w.finish(out, duration.max(last + 1))?;
    Ok(())
}

fn demux(path: &Path) -> Result<(Vec<Packet>, Vec<Packet>)> {
    let mut mkv = MatroskaFile::open(File::open(path).with_context(|| format!("opening {}", path.display()))?)?;
    let (mut video, mut audio) = (Vec::new(), Vec::new());
    let mut f = Frame::default();
    while mkv.next_frame(&mut f)? {
        let p = Packet { t: f.timestamp, key: f.is_keyframe == Some(true), data: std::mem::take(&mut f.data) };
        match f.track {
            1 => video.push(p),
            2 => audio.push(p),
            _ => {}
        }
    }
    video.sort_by_key(|p| p.t);
    audio.sort_by_key(|p| p.t);
    Ok((video, audio))
}

fn to_annexb(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut rest = data;
    while rest.len() >= 4 {
        let n = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        let Some(nal) = rest.get(4..4 + n) else { break };
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
        rest = &rest[4 + n..];
    }
    out
}

/// The classic arrow pointer, tip at (0, 0), in 1x pixels.
const ARROW: [(f32, f32); 7] = [(0.0, 0.0), (0.0, 17.0), (4.2, 13.2), (7.0, 19.6), (9.6, 18.5), (6.9, 12.2), (12.3, 12.2)];

fn draw_cursor(img: &mut RgbaImage, at: (f32, f32), scale: f32) {
    let pts: Vec<(f32, f32)> = ARROW.iter().map(|(x, y)| (at.0 + x * scale, at.1 + y * scale)).collect();
    // Black outline first, then the white body on top.
    for i in 0..pts.len() {
        stroke_segment(img, pts[i], pts[(i + 1) % pts.len()], 2.2 * scale, [0, 0, 0]);
    }
    fill_polygon(img, &pts, [255, 255, 255]);
}

fn draw_ripple(img: &mut RgbaImage, at: (f32, f32), progress: f32, scale: f32, color: [u8; 3]) {
    let r = (10.0 + 26.0 * progress) * scale;
    let alpha = (1.0 - progress).powf(1.5);
    let ring = 3.5 * scale;
    let reach = r + ring;
    let (w, h) = (img.width() as i32, img.height() as i32);
    for y in ((at.1 - reach) as i32).max(0)..=((at.1 + reach) as i32).min(h - 1) {
        for x in ((at.0 - reach) as i32).max(0)..=((at.0 + reach) as i32).min(w - 1) {
            let d = ((x as f32 + 0.5 - at.0).powi(2) + (y as f32 + 0.5 - at.1).powi(2)).sqrt();
            let a = ((ring / 2.0 - (d - r).abs() + 0.5).clamp(0.0, 1.0)) * alpha;
            if a > 0.0 {
                blend(img.get_pixel_mut(x as u32, y as u32), color, a);
            }
        }
    }
}

/// Even-odd polygon fill with 4x4 supersampling for smooth edges.
fn fill_polygon(img: &mut RgbaImage, pts: &[(f32, f32)], color: [u8; 3]) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let x0 = pts.iter().map(|p| p.0).fold(f32::MAX, f32::min).floor().max(0.0) as i32;
    let y0 = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor().max(0.0) as i32;
    let x1 = (pts.iter().map(|p| p.0).fold(f32::MIN, f32::max).ceil() as i32).min(w - 1);
    let y1 = (pts.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil() as i32).min(h - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let mut hits = 0;
            for sy in 0..4 {
                for sx in 0..4 {
                    if inside(pts, x as f32 + (sx as f32 + 0.5) / 4.0, y as f32 + (sy as f32 + 0.5) / 4.0) {
                        hits += 1;
                    }
                }
            }
            if hits > 0 {
                blend(img.get_pixel_mut(x as u32, y as u32), color, hits as f32 / 16.0);
            }
        }
    }
}

fn inside(pts: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut c = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            c = !c;
        }
        j = i;
    }
    c
}

fn blend(p: &mut image::Rgba<u8>, c: [u8; 3], a: f32) {
    for (ch, v) in p.0.iter_mut().zip(c) {
        *ch = (*ch as f32 * (1.0 - a) + v as f32 * a).round() as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openscrib_core::{Clip, Step, StepKind};

    #[test]
    fn segments_follow_step_order_or_whole_recording() {
        let mut m = Manifest::new("t");
        m.duration_ms = 60_000;
        assert_eq!(segments(&m), [(0, 60_000)]);
        let mut a = Step::new(1, 10_000, StepKind::Click);
        a.clip = Some(Clip { start_ms: 9_000, end_ms: 12_000 });
        let mut b = Step::new(2, 30_000, StepKind::Click);
        b.clip = Some(Clip { start_ms: 29_000, end_ms: 70_000 });
        let c = Step::new(3, 40_000, StepKind::Click);
        // Reordered in the editor: b before a.
        m.steps = vec![b, c, a];
        assert_eq!(segments(&m), [(29_000, 60_000), (9_000, 12_000)]);
    }

    #[test]
    fn pointer_interpolates_and_ripples() {
        let p = Pointer {
            moves: vec![(0, Point::new(0, 0)), (20, Point::new(20, 10)), (1000, Point::new(500, 500))],
            presses: vec![(100, Point::new(20, 10))],
        };
        assert_eq!(p.at(10), Some((10.0, 5.0)));
        // Long gap: hold, don't glide across.
        assert_eq!(p.at(500), Some((20.0, 10.0)));
        assert_eq!(p.ripples(99).count(), 0);
        let r: Vec<_> = p.ripples(325).collect();
        assert_eq!(r.len(), 1);
        assert!((r[0].1 - 0.5).abs() < 0.01);
        assert_eq!(p.ripples(600).count(), 0);
    }

    #[test]
    fn cursor_is_drawn_white_with_outline() {
        let mut img = RgbaImage::from_pixel(60, 60, image::Rgba([90, 90, 90, 255]));
        draw_cursor(&mut img, (10.0, 10.0), 2.0);
        assert_eq!(img.get_pixel(13, 30).0[..3], [255, 255, 255]);
        assert!(img.get_pixel(9, 20).0[0] < 60);
    }
}
