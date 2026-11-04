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
use openscrib_core::{Annotation, Manifest, Point, ProjectDir, Rect, quality};

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
    /// Replace the narration with the step text read aloud.
    pub voiceover: Option<crate::voiceover::Voice>,
}

impl VideoExportOptions {
    pub fn new(openh264: PathBuf, format: VideoFormat) -> Self {
        Self { openh264, format, max_width: 1920, max_fps: 15, cursor: true, clicks: true, quality: quality::DEFAULT_QUALITY, voiceover: None }
    }
}

#[derive(Debug, Clone)]
pub struct VideoExportStats {
    pub duration_ms: u64,
    pub frames: u64,
    pub segments: usize,
    pub width: u32,
    pub height: u32,
    /// Captions written beside the video (`.srt` and `.vtt`).
    pub captions: usize,
}

/// Narration remapped onto the output timeline of `segs` (the stitched
/// clips), as `(start_ms, end_ms, text)`.
pub fn captions(manifest: &Manifest, segs: &[(u64, u64)]) -> Vec<(u64, u64, String)> {
    let mut out = Vec::new();
    let mut base = 0;
    for &(a, b) in segs {
        for n in &manifest.narration {
            let (s, e) = (n.start_ms.max(a), n.end_ms.min(b));
            if e > s + 200 {
                out.push((base + s - a, base + e - a, n.text.trim().to_string()));
            }
        }
        base += b - a;
    }
    out
}

fn caption_time(ms: u64, sep: char) -> String {
    format!("{:02}:{:02}:{:02}{sep}{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// Write `<stem>.srt` and `<stem>.vtt` next to `video`; returns how many.
pub fn write_captions(video: &Path, caps: &[(u64, u64, String)]) -> Result<usize> {
    if caps.is_empty() {
        return Ok(0);
    }
    let mut srt = String::new();
    let mut vtt = String::from("WEBVTT

");
    for (i, (s, e, text)) in caps.iter().enumerate() {
        srt.push_str(&format!("{}
{} --> {}
{text}

", i + 1, caption_time(*s, ','), caption_time(*e, ',')));
        vtt.push_str(&format!("{} --> {}
{text}

", caption_time(*s, '.'), caption_time(*e, '.')));
    }
    fs::write(video.with_extension("srt"), srt)?;
    fs::write(video.with_extension("vtt"), vtt)?;
    Ok(caps.len())
}

/// The source ranges to stitch, in output order: each step's clip in step
/// order, or the whole recording when no step has one.
pub fn segments(manifest: &Manifest) -> Vec<(u64, u64)> {
    let clips: Vec<_> = manifest
        .steps
        .iter()
        .filter(|s| !s.deleted)
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

/// Smallest video crop honoured, in pixels. Anything tinier is a slip of the
/// mouse rather than a video anyone wants.
const MIN_VIEWPORT: u32 = 16;

/// The part of a `w`x`h` recording the export shows: `video_crop` kept inside
/// the frame, or the whole frame without a usable one.
fn viewport(manifest: &Manifest, w: u32, h: u32) -> Rect {
    let full = Rect { x: 0, y: 0, w, h };
    let Some(crop) = manifest.video_crop else { return full };
    let x = crop.x.clamp(0, w as i32 - 1);
    let y = crop.y.clamp(0, h as i32 - 1);
    let view = Rect { x, y, w: crop.w.min(w - x as u32), h: crop.h.min(h - y as u32) };
    if view.w < MIN_VIEWPORT || view.h < MIN_VIEWPORT { full } else { view }
}

/// Output size and scale for a viewport: never upscaled, even-sized for H.264.
fn output_size(view: Rect, max_width: u32) -> (u32, u32, f32) {
    let scale = (max_width as f32 / view.w as f32).min(1.0);
    (((view.w as f32 * scale) as u32) & !1, ((view.h as f32 * scale) as u32) & !1, scale)
}

/// Rough size of an export in bytes, or `None` without a screen video. Screen
/// recordings compress by how much changes rather than by length, so this
/// scales the recording's own size: by the share of it kept, by pixels
/// (sub-linearly, since scaled text compresses worse) and by quantiser, every
/// six steps of which halve the bits.
pub fn estimate_size(dir: &ProjectDir, manifest: &Manifest, quality: u8, max_width: u32) -> Option<u64> {
    let video = manifest.video.as_ref()?;
    let source = fs::metadata(dir.path(SCREEN_FILE)).ok()?.len() as f64;
    let total = manifest.duration_ms.max(1) as f64;
    let kept: u64 = segments(manifest).iter().map(|(a, b)| b - a).sum();
    let kept_share = (kept as f64 / total).min(1.0);
    let audio = if manifest.audio.is_some() { AUDIO_BYTES_PER_MS * total } else { 0.0 };

    let (out_w, out_h, _) = output_size(viewport(manifest, video.width, video.height), max_width);
    let pixels = (out_w as f64 * out_h as f64) / (video.width as f64 * video.height as f64).max(1.0);
    let qp = |q: u8| quality::qp_range(q).1 as f64;
    let sharpness = 2f64.powf((qp(quality::DEFAULT_QUALITY) - qp(quality)) / 6.0);

    let frames = (source - audio).max(0.0) * kept_share * pixels.powf(0.75) * sharpness;
    Some((frames + audio * kept_share) as u64)
}

/// Opus narration at the bitrate recordings use.
const AUDIO_BYTES_PER_MS: f64 = 4.0;

/// What an output frame shows: source frame, rounded cursor position, and
/// whether a click ripple is animating. Unchanged keys are not re-encoded.
type FrameKey = (Option<usize>, Option<(i32, i32)>, bool, usize);

/// Redactions in force over time: each step's redactions apply from its
/// click until the next step (the first step's also cover the lead-in).
/// Only steps captured on the recorded monitor count, since redaction
/// rectangles are in that monitor's pixels.
struct Redactions(Vec<(u64, Vec<Rect>)>);

impl Redactions {
    fn new(manifest: &Manifest, monitor: u32) -> Self {
        let mut by_time: Vec<(u64, Vec<Rect>)> = manifest
            .steps
            .iter()
            .map(|s| {
                let rects = if s.monitor == Some(monitor) {
                    s.annotations
                        .iter()
                        .filter_map(|a| if let Annotation::Blur { rect, .. } = a { Some(*rect) } else { None })
                        .collect()
                } else {
                    Vec::new()
                };
                (s.t_ms, rects)
            })
            .collect();
        by_time.sort_by_key(|(t, _)| *t);
        Self(by_time)
    }

    /// Index of the step in force at `t` and its redactions.
    fn at(&self, t: u64) -> (usize, &[Rect]) {
        let i = self.0.partition_point(|(st, _)| *st <= t).saturating_sub(1);
        (i, self.0.get(i).map_or(&[][..], |(_, r)| r.as_slice()))
    }
}

/// A redaction in output pixels, cut to the viewport, or `None` when it falls
/// outside it. Rounded outward, so scaling can't leave an edge showing.
fn redaction_in_view(r: Rect, view: Rect, scale: f32) -> Option<Rect> {
    let x0 = r.x.max(view.x);
    let y0 = r.y.max(view.y);
    let x1 = (r.x + r.w as i32).min(view.x + view.w as i32);
    let y1 = (r.y + r.h as i32).min(view.y + view.h as i32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(Rect {
        x: ((x0 - view.x) as f32 * scale).floor() as i32,
        y: ((y0 - view.y) as f32 * scale).floor() as i32,
        w: ((x1 - x0) as f32 * scale).ceil() as u32 + 1,
        h: ((y1 - y0) as f32 * scale).ceil() as u32 + 1,
    })
}

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
    let redactions = Redactions::new(manifest, video_info.monitor);

    let (src_w, src_h) = (video_info.width, video_info.height);
    let view = viewport(manifest, src_w, src_h);
    let cropped = view.w != src_w || view.h != src_h;
    let (out_w, out_h, scale) = output_size(view, opts.max_width);
    // Screen position to output pixels.
    let to_out = move |x: f32, y: f32| ((x - view.x as f32) * scale, (y - view.y as f32) * scale);

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

    // Audio packets remapped onto the output timeline, written as video
    // catches up so the file stays interleaved.
    let mut out_audio: Vec<(u64, Vec<u8>)> = Vec::new();
    let mut offset = 0u64;
    for &(a, b) in &segs {
        for p in audio.iter().filter(|p| p.t >= a && p.t < b) {
            out_audio.push((offset + p.t - a, p.data.clone()));
        }
        offset += b - a;
    }
    let total_ms = offset;
    // Voiceover replaces the recorded narration (and its captions).
    let mut spoken_captions = None;
    if let Some(voice) = &opts.voiceover {
        let track = crate::voiceover::build(manifest, &segs, voice, |_| {})?;
        out_audio = crate::voiceover::encode(&track.pcm)?.into_iter().enumerate().map(|(i, p)| (i as u64 * 20, p)).collect();
        spoken_captions = Some(track.captions);
    }
    let audio_spec = (!out_audio.is_empty()).then_some(AudioSpec { channels: 1 });
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
                pointer.at(t).map(|(x, y)| to_out(x - origin.x as f32, y - origin.y as f32))
            } else {
                None
            };
            let rippling = opts.clicks && pointer.ripples(t).next().is_some();
            let (active, hidden) = redactions.at(t);
            let key = (cur, cursor.map(|(x, y)| (x.round() as i32, y.round() as i32)), rippling, active);
            let changed = last_emit.as_ref() != Some(&key) || rippling;

            if (changed || first_in_seg)
                && let Some(src) = &src_rgba
            {
                let visible = cropped.then(|| imageops::crop_imm(src, view.x as u32, view.y as u32, view.w, view.h).to_image());
                let visible = visible.as_ref().unwrap_or(src);
                let mut frame = if scale < 1.0 {
                    let mut dst = RgbaImage::new(out_w, out_h);
                    resizer.resize(visible, &mut dst, &resize_opts).context("scaling frame")?;
                    dst
                } else {
                    imageops::crop_imm(visible, 0, 0, out_w, out_h).to_image()
                };
                for r in hidden {
                    if let Some(r) = redaction_in_view(*r, view, scale) {
                        crate::render::redact(&mut frame, r);
                    }
                }
                if opts.clicks {
                    for (p, k) in pointer.ripples(t) {
                        let at = to_out((p.x - origin.x) as f32, (p.y - origin.y) as f32);
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
    let captions = write_captions(out, &spoken_captions.unwrap_or_else(|| captions(manifest, &segs)))?;
    Ok(VideoExportStats { duration_ms: total_ms, frames, segments: segs.len(), width: out_w, height: out_h, captions })
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
    fn deleted_steps_lose_their_clip_but_keep_their_redactions() {
        let mut m = Manifest::new("t");
        m.duration_ms = 60_000;
        let r = Rect { x: 10, y: 10, w: 50, h: 20 };
        let mut a = Step::new(1, 5_000, StepKind::Click);
        a.clip = Some(Clip { start_ms: 4_000, end_ms: 8_000 });
        let mut b = Step::new(2, 9_000, StepKind::Click);
        b.clip = Some(Clip { start_ms: 8_000, end_ms: 12_000 });
        b.monitor = Some(7);
        b.annotations.push(Annotation::Blur { rect: r, group: None });
        b.deleted = true;
        m.steps = vec![a, b];
        assert_eq!(segments(&m), [(4_000, 8_000)]);
        assert_eq!(Redactions::new(&m, 7).at(10_000).1, [r]);
    }

    #[test]
    fn viewport_stays_inside_the_frame() {
        let mut m = Manifest::new("t");
        let full = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        assert_eq!(viewport(&m, 1920, 1080), full);
        m.video_crop = Some(Rect { x: 100, y: 200, w: 800, h: 600 });
        assert_eq!(viewport(&m, 1920, 1080), Rect { x: 100, y: 200, w: 800, h: 600 });
        m.video_crop = Some(Rect { x: 1800, y: 1000, w: 800, h: 600 });
        assert_eq!(viewport(&m, 1920, 1080), Rect { x: 1800, y: 1000, w: 120, h: 80 });
        m.video_crop = Some(Rect { x: 5000, y: 0, w: 800, h: 600 });
        assert_eq!(viewport(&m, 1920, 1080), full);
        m.video_crop = Some(Rect { x: 10, y: 10, w: 3, h: 3 });
        assert_eq!(viewport(&m, 1920, 1080), full);
    }

    #[test]
    fn output_is_even_and_never_upscaled() {
        assert_eq!(output_size(Rect { x: 0, y: 0, w: 801, h: 601 }, 1920), (800, 600, 1.0));
        let (w, h, scale) = output_size(Rect { x: 0, y: 0, w: 3440, h: 1440 }, 1720);
        assert_eq!((w, h, scale), (1720, 720, 0.5));
    }

    #[test]
    fn redactions_are_cut_to_the_viewport() {
        let view = Rect { x: 100, y: 100, w: 400, h: 300 };
        // Hangs over the left edge: only the part inside is painted.
        assert_eq!(
            redaction_in_view(Rect { x: 50, y: 150, w: 100, h: 20 }, view, 1.0),
            Some(Rect { x: 0, y: 50, w: 51, h: 21 })
        );
        assert_eq!(redaction_in_view(Rect { x: 0, y: 0, w: 50, h: 50 }, view, 1.0), None);
    }

    #[test]
    fn redactions_follow_the_active_step() {
        let mut m = Manifest::new("t");
        let r = Rect { x: 10, y: 10, w: 50, h: 20 };
        let mut a = Step::new(1, 5_000, StepKind::Click);
        a.monitor = Some(7);
        a.annotations.push(Annotation::Blur { rect: r, group: None });
        let mut b = Step::new(2, 9_000, StepKind::Click);
        b.monitor = Some(7);
        let mut other = Step::new(3, 12_000, StepKind::Click);
        other.monitor = Some(8);
        other.annotations.push(Annotation::Blur { rect: r, group: None });
        m.steps = vec![b, a, other];
        let red = Redactions::new(&m, 7);
        assert_eq!(red.at(0).1, [r]); // lead-in uses the first step
        assert_eq!(red.at(6_000).1, [r]);
        assert!(red.at(9_500).1.is_empty());
        assert!(red.at(20_000).1.is_empty()); // other monitor's rects don't apply
    }

    #[test]
    fn captions_follow_the_stitched_timeline() {
        let mut m = Manifest::new("t");
        m.narration = vec![
            openscrib_core::NarrationSegment { start_ms: 1_000, end_ms: 3_000, text: "intro".into(), step: None },
            openscrib_core::NarrationSegment { start_ms: 10_500, end_ms: 12_000, text: "second clip".into(), step: None },
            openscrib_core::NarrationSegment { start_ms: 20_000, end_ms: 21_000, text: "cut".into(), step: None },
        ];
        let caps = captions(&m, &[(10_000, 13_000), (0, 4_000)]);
        assert_eq!(caps, [(500, 2_000, "second clip".into()), (4_000, 6_000, "intro".into())]);
        assert_eq!(caption_time(3_723_456, ','), "01:02:03,456");
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
