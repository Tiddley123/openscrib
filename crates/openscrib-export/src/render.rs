//! Turns a step's raw frame into the image a guide shows: redactions are
//! applied to the full-resolution pixels first, then the frame is cropped
//! and scaled, and markers and shapes are drawn last so they keep the same
//! on-page size whatever the source resolution.

use anyhow::{Context, Result};
use image::imageops::{self, FilterType};
use image::{Rgba, RgbaImage};
use openscrib_core::markers::{MarkerStyle, draw_marker};
use openscrib_core::{Annotation, Manifest, Point, ProjectDir, Rect, Step, StepKind};

/// Redactions are painted solid: blurs and pixelation can leak large text.
const REDACT: [u8; 3] = [32, 32, 36];
/// Window crops smaller than this get padded so a lone menu or dialog still
/// has some surrounding context.
const MIN_CROP: (u32, u32) = (900, 560);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CropMode {
    /// The whole captured monitor.
    Full,
    /// The window that was clicked (or focused), padded to a minimum size.
    Window,
}

#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub crop: CropMode,
    /// Output images are scaled down to at most this width.
    pub max_width: u32,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self { crop: CropMode::Window, max_width: 1600 }
    }
}

/// Render one step's guide image, or `None` if the step has no frame.
pub fn render_step(dir: &ProjectDir, manifest: &Manifest, step: &Step, opts: &RenderOptions) -> Result<Option<RgbaImage>> {
    let Some(rel) = &step.frame else { return Ok(None) };
    let mut img = image::open(dir.path(rel)).with_context(|| format!("reading {rel}"))?.to_rgba8();
    let scale_factor = step
        .monitor
        .and_then(|id| manifest.monitor(id))
        .map_or(1.0, |m| m.scale);

    for a in &step.annotations {
        if let Annotation::Blur { rect } = a {
            redact(&mut img, *rect);
        }
    }

    let crop = step.crop.unwrap_or_else(|| default_crop(manifest, step, opts.crop, img.dimensions()));
    let crop = clamp_rect(crop, img.dimensions());
    let mut out = imageops::crop_imm(&img, crop.x as u32, crop.y as u32, crop.w, crop.h).to_image();
    let s = if out.width() > opts.max_width { opts.max_width as f32 / out.width() as f32 } else { 1.0 };
    if s < 1.0 {
        let h = (out.height() as f32 * s).round().max(1.0) as u32;
        out = imageops::resize(&out, opts.max_width, h, FilterType::Lanczos3);
    }

    let accent = manifest.accent_rgb();
    let map = |p: Point| ((p.x - crop.x) as f32 * s, (p.y - crop.y) as f32 * s);
    // Shapes and markers keep the size they would have at 1:1 on the
    // source monitor's DPI, not shrink with the crop's downscale.
    let ui = scale_factor.max(1.0);
    for a in &step.annotations {
        match a {
            Annotation::Blur { .. } => {}
            Annotation::Box { rect } => {
                let (x0, y0) = map(rect.origin());
                let (x1, y1) = map(Point::new(rect.x + rect.w as i32, rect.y + rect.h as i32));
                stroke_rect(&mut out, (x0, y0), (x1, y1), 4.0 * ui, accent);
            }
            Annotation::Arrow { from, to } => draw_arrow(&mut out, map(*from), map(*to), 5.0 * ui, accent),
            Annotation::Marker { at } => {
                let (x, y) = map(*at);
                draw_marker(&mut out, Point::new(x.round() as i32, y.round() as i32), scaled(MarkerStyle { color: accent, ..MarkerStyle::default() }, ui));
            }
        }
    }
    if let (StepKind::Click, Some(c)) = (step.kind, &step.click) {
        let (x, y) = map(c.in_frame);
        let at = Point::new(x.round() as i32, y.round() as i32);
        let mut style = scaled(MarkerStyle::for_click(c, 1.0), ui);
        if c.button != openscrib_core::MouseButton::Right {
            style.color = accent;
        }
        draw_marker(&mut out, at, style);
        if c.double {
            draw_marker(&mut out, at, MarkerStyle { radius: style.radius + style.ring * 2.5, ..style });
        }
    }
    Ok(Some(out))
}

fn scaled(mut s: MarkerStyle, k: f32) -> MarkerStyle {
    s.radius *= k;
    s.ring *= k;
    s
}

/// The crop used when the step doesn't set one.
pub fn default_crop(manifest: &Manifest, step: &Step, mode: CropMode, (fw, fh): (u32, u32)) -> Rect {
    let full = Rect { x: 0, y: 0, w: fw, h: fh };
    if mode == CropMode::Full {
        return full;
    }
    let origin = step
        .monitor
        .and_then(|id| manifest.monitor(id))
        .map_or(Point::new(0, 0), |m| m.rect.origin());
    let Some(win) = &step.window else { return full };
    let r = win.client.unwrap_or(win.rect);
    let mut crop = Rect { x: r.x - origin.x, y: r.y - origin.y, w: r.w, h: r.h };

    // Keep the click in view even if the window lookup was off.
    if let Some(c) = &step.click {
        crop = union_point(crop, c.in_frame, 40);
    }
    let crop = pad_to_min(clamp_rect(crop, (fw, fh)), (fw, fh));
    let focus = step.click.as_ref().map_or_else(|| centre(crop), |c| c.in_frame);
    clamp_aspect(crop, focus, (fw, fh))
}

/// Widest and tallest crop shapes we allow; a full-width taskbar or a thin
/// sidebar would otherwise become an unreadable strip on the page.
const MAX_ASPECT: f32 = 2.4;
const MIN_ASPECT: f32 = 0.45;

fn clamp_aspect(r: Rect, focus: Point, frame: (u32, u32)) -> Rect {
    let aspect = r.w as f32 / r.h as f32;
    let (w, h) = if aspect > MAX_ASPECT {
        (((r.h as f32 * MAX_ASPECT) as u32).max(MIN_CROP.0.min(r.w)), r.h)
    } else if aspect < MIN_ASPECT {
        (r.w, ((r.w as f32 / MIN_ASPECT) as u32).max(MIN_CROP.1.min(r.h)))
    } else {
        return r;
    };
    // Centre on the focus point, but stay within the original crop.
    let x = (focus.x - w as i32 / 2).clamp(r.x, r.x + (r.w - w) as i32);
    let y = (focus.y - h as i32 / 2).clamp(r.y, r.y + (r.h - h) as i32);
    clamp_rect(Rect { x, y, w, h }, frame)
}

fn centre(r: Rect) -> Point {
    Point::new(r.x + r.w as i32 / 2, r.y + r.h as i32 / 2)
}

fn union_point(r: Rect, p: Point, margin: i32) -> Rect {
    let x0 = r.x.min(p.x - margin);
    let y0 = r.y.min(p.y - margin);
    let x1 = (r.x + r.w as i32).max(p.x + margin);
    let y1 = (r.y + r.h as i32).max(p.y + margin);
    Rect { x: x0, y: y0, w: (x1 - x0) as u32, h: (y1 - y0) as u32 }
}

fn pad_to_min(r: Rect, (fw, fh): (u32, u32)) -> Rect {
    let w = r.w.max(MIN_CROP.0).min(fw);
    let h = r.h.max(MIN_CROP.1).min(fh);
    let cx = r.x + r.w as i32 / 2;
    let cy = r.y + r.h as i32 / 2;
    clamp_rect(Rect { x: cx - w as i32 / 2, y: cy - h as i32 / 2, w, h }, (fw, fh))
}

/// Shift (then shrink if needed) `r` to lie inside the frame, never empty.
pub fn clamp_rect(r: Rect, (fw, fh): (u32, u32)) -> Rect {
    let w = r.w.clamp(1, fw.max(1));
    let h = r.h.clamp(1, fh.max(1));
    let x = r.x.clamp(0, (fw - w) as i32);
    let y = r.y.clamp(0, (fh - h) as i32);
    Rect { x, y, w, h }
}

/// Paint `rect` solid so nothing underneath survives in the export.
pub fn redact(img: &mut RgbaImage, rect: Rect) {
    let r = clamp_rect(rect, img.dimensions());
    for y in r.y as u32..r.y as u32 + r.h {
        for x in r.x as u32..r.x as u32 + r.w {
            img.put_pixel(x, y, Rgba([REDACT[0], REDACT[1], REDACT[2], 255]));
        }
    }
}

fn stroke_rect(img: &mut RgbaImage, a: (f32, f32), b: (f32, f32), width: f32, color: [u8; 3]) {
    let (x0, y0, x1, y1) = (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1));
    for (p, q) in [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x1, y1), (x0, y1)), ((x0, y1), (x0, y0))] {
        stroke_segment(img, p, q, width, color);
    }
}

fn draw_arrow(img: &mut RgbaImage, from: (f32, f32), to: (f32, f32), width: f32, color: [u8; 3]) {
    stroke_segment(img, from, to, width, color);
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return;
    }
    let head = (width * 4.0).min(len * 0.6);
    let (ux, uy) = (dx / len, dy / len);
    for angle in [0.5f32, -0.5] {
        let (s, c) = angle.sin_cos();
        let (rx, ry) = (ux * c - uy * s, ux * s + uy * c);
        stroke_segment(img, to, (to.0 - rx * head, to.1 - ry * head), width, color);
    }
}

/// Anti-aliased thick line with round caps.
pub(crate) fn stroke_segment(img: &mut RgbaImage, a: (f32, f32), b: (f32, f32), width: f32, color: [u8; 3]) {
    let r = width / 2.0;
    let (w, h) = (img.width() as i32, img.height() as i32);
    let x0 = ((a.0.min(b.0) - r - 1.0).floor() as i32).max(0);
    let y0 = ((a.1.min(b.1) - r - 1.0).floor() as i32).max(0);
    let x1 = ((a.0.max(b.0) + r + 1.0).ceil() as i32).min(w - 1);
    let y1 = ((a.1.max(b.1) + r + 1.0).ceil() as i32).min(h - 1);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = (dx * dx + dy * dy).max(1e-6);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let t = (((px - a.0) * dx + (py - a.1) * dy) / len2).clamp(0.0, 1.0);
            let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
            let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
            let alpha = (r - d + 0.5).clamp(0.0, 1.0);
            if alpha > 0.0 {
                let p = img.get_pixel_mut(x as u32, y as u32);
                for (ch, c) in p.0.iter_mut().zip(color) {
                    *ch = (*ch as f32 * (1.0 - alpha) + c as f32 * alpha).round() as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openscrib_core::{Click, MonitorInfo, MouseButton, WindowInfo};

    fn manifest_and_step() -> (Manifest, Step) {
        let mut m = Manifest::new("t");
        m.monitors.push(MonitorInfo {
            id: 1,
            name: "m".into(),
            rect: Rect { x: -1920, y: 0, w: 1920, h: 1080 },
            scale: 1.0,
            primary: false,
        });
        let mut s = Step::new(1, 0, StepKind::Click);
        s.monitor = Some(1);
        s.window = Some(WindowInfo {
            rect: Rect { x: -1800, y: 100, w: 1000, h: 700 },
            client: Some(Rect { x: -1790, y: 140, w: 980, h: 650 }),
            title: "w".into(),
            process: "p".into(),
        });
        s.click = Some(Click {
            x: -1500,
            y: 300,
            button: MouseButton::Left,
            double: false,
            in_window: None,
            in_frame: Point::new(420, 300),
        });
        (m, s)
    }

    #[test]
    fn window_crop_uses_client_area_in_frame_coords() {
        let (m, s) = manifest_and_step();
        let c = default_crop(&m, &s, CropMode::Window, (1920, 1080));
        assert_eq!(c, Rect { x: 130, y: 140, w: 980, h: 650 });
        assert_eq!(default_crop(&m, &s, CropMode::Full, (1920, 1080)), Rect { x: 0, y: 0, w: 1920, h: 1080 });
    }

    #[test]
    fn small_windows_are_padded_and_clamped() {
        let (m, mut s) = manifest_and_step();
        s.window.as_mut().unwrap().client = Some(Rect { x: -1920, y: 0, w: 200, h: 120 });
        s.click.as_mut().unwrap().in_frame = Point::new(50, 50);
        let c = default_crop(&m, &s, CropMode::Window, (1920, 1080));
        assert_eq!((c.x, c.y, c.w, c.h), (0, 0, 900, 560));
    }

    #[test]
    fn taskbar_clicks_get_a_readable_crop() {
        let (mut m, mut s) = manifest_and_step();
        m.monitors[0].rect = Rect { x: 0, y: 0, w: 3440, h: 1440 };
        let bar = Rect { x: 0, y: 1392, w: 3440, h: 48 };
        s.window = Some(WindowInfo { rect: bar, client: Some(bar), title: String::new(), process: "explorer.exe".into() });
        s.click.as_mut().unwrap().in_frame = Point::new(893, 1394);
        let c = default_crop(&m, &s, CropMode::Window, (3440, 1440));
        assert_eq!((c.w, c.h), (1344, 560));
        assert!(c.x <= 893 && 893 < c.x + c.w as i32);
        assert_eq!(c.y + c.h as i32, 1440);
    }

    #[test]
    fn redaction_is_solid() {
        let mut img = RgbaImage::from_fn(28, 28, |x, _| if x % 2 == 0 { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) });
        redact(&mut img, Rect { x: 4, y: 4, w: 10, h: 10 });
        for (x, y) in [(4, 4), (5, 9), (13, 13)] {
            assert_eq!(img.get_pixel(x, y).0, [32, 32, 36, 255]);
        }
        assert_eq!(img.get_pixel(15, 4).0, [255, 255, 255, 255]);
    }

    #[test]
    fn renders_crop_and_marker() {
        let (m, s) = manifest_and_step();
        let tmp = tempfile::tempdir().unwrap();
        let dir = ProjectDir::create(tmp.path()).unwrap();
        let rel = ProjectDir::frame_name(1, "");
        RgbaImage::from_pixel(1920, 1080, Rgba([10, 10, 10, 255])).save(dir.path(&rel)).unwrap();
        let mut s = s;
        s.frame = Some(rel);
        s.annotations.push(Annotation::Arrow { from: Point::new(200, 200), to: Point::new(400, 290) });
        let out = render_step(&dir, &m, &s, &RenderOptions::default()).unwrap().unwrap();
        assert_eq!(out.dimensions(), (980, 650));
        // Marker ring 30 px right of the click (click is at 290,160 in crop).
        assert!(out.get_pixel(320, 160).0[0] > 200);
        // Untouched background far from everything.
        assert_eq!(out.get_pixel(900, 600).0, [10, 10, 10, 255]);
    }
}
