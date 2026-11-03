//! Pictographs: stickers drawn onto screenshots, and the matching SVG icons
//! for callout boxes in HTML and PDF. All vector, so they stay crisp at any
//! size and need no emoji font.

use image::{Rgba, RgbaImage};
use openscrib_core::{Callout, Sticker};

use crate::render::stroke_segment;

const WHITE: [u8; 3] = [255, 255, 255];
const AMBER: [u8; 3] = [245, 165, 36];
const RED: [u8; 3] = [229, 72, 77];
const BLUE: [u8; 3] = [10, 132, 255];
const GREEN: [u8; 3] = [48, 164, 108];
const PURPLE: [u8; 3] = [110, 86, 207];

pub fn sticker_color(icon: Sticker, accent: [u8; 3]) -> [u8; 3] {
    match icon {
        Sticker::Warning => AMBER,
        Sticker::Important | Sticker::Cross => RED,
        Sticker::Info => BLUE,
        Sticker::Tip | Sticker::Check => GREEN,
        Sticker::Question => PURPLE,
        Sticker::Quote => accent,
    }
}

/// Draw a sticker badge centred on `c` with diameter `d` (output pixels).
pub fn draw_sticker(img: &mut RgbaImage, c: (f32, f32), d: f32, icon: Sticker, accent: [u8; 3]) {
    let r = d / 2.0;
    let color = sticker_color(icon, accent);
    // Soft shadow, white rim, coloured body.
    if icon == Sticker::Warning {
        let tri = |grow: f32, dy: f32| {
            let h = r * 1.9 + grow;
            [(c.0, c.1 - h * 0.55 + dy), (c.0 + h * 0.58, c.1 + h * 0.45 + dy), (c.0 - h * 0.58, c.1 + h * 0.45 + dy)]
        };
        fill_polygon(img, &tri(6.0, r * 0.08), [0, 0, 0], 0.25);
        fill_polygon(img, &tri(6.0, 0.0), WHITE, 1.0);
        fill_polygon(img, &tri(0.0, 0.0), color, 1.0);
    } else {
        fill_circle(img, (c.0, c.1 + r * 0.08), r + 3.0, [0, 0, 0], 0.25);
        fill_circle(img, c, r + 3.0, WHITE, 1.0);
        fill_circle(img, c, r, color, 1.0);
    }

    let w = (d * 0.11).max(2.5);
    let at = |x: f32, y: f32| (c.0 + x * r, c.1 + y * r);
    match icon {
        Sticker::Warning | Sticker::Important => {
            let top = if icon == Sticker::Warning { -0.35 } else { -0.5 };
            stroke_segment(img, at(0.0, top), at(0.0, 0.18), w * 1.15, WHITE);
            fill_circle(img, at(0.0, 0.48), w * 0.72, WHITE, 1.0);
        }
        Sticker::Info => {
            fill_circle(img, at(0.0, -0.45), w * 0.72, WHITE, 1.0);
            stroke_segment(img, at(0.0, -0.1), at(0.0, 0.5), w * 1.15, WHITE);
        }
        Sticker::Check => {
            stroke_segment(img, at(-0.42, 0.02), at(-0.12, 0.34), w * 1.1, WHITE);
            stroke_segment(img, at(-0.12, 0.34), at(0.45, -0.32), w * 1.1, WHITE);
        }
        Sticker::Cross => {
            stroke_segment(img, at(-0.34, -0.34), at(0.34, 0.34), w * 1.1, WHITE);
            stroke_segment(img, at(0.34, -0.34), at(-0.34, 0.34), w * 1.1, WHITE);
        }
        Sticker::Question => {
            // A hook drawn as a short polyline arc, then the stem and dot.
            let pts: Vec<(f32, f32)> = (0..=10)
                .map(|i| {
                    let a = std::f32::consts::PI * (1.05 - 1.35 * i as f32 / 10.0);
                    at(0.27 * a.cos(), -0.2 - 0.27 * a.sin())
                })
                .collect();
            for p in pts.windows(2) {
                stroke_segment(img, p[0], p[1], w, WHITE);
            }
            let end = *pts.last().expect("arc points");
            stroke_segment(img, end, at(0.0, 0.18), w, WHITE);
            fill_circle(img, at(0.0, 0.47), w * 0.7, WHITE, 1.0);
        }
        Sticker::Tip => {
            let star: Vec<(f32, f32)> = (0..10)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                    let k = if i % 2 == 0 { 0.6 } else { 0.25 };
                    at(k * a.cos(), k * a.sin() + 0.04)
                })
                .collect();
            fill_polygon(img, &star, WHITE, 1.0);
        }
        Sticker::Quote => {
            for dx in [-0.24, 0.24] {
                fill_circle(img, at(dx, 0.05), r * 0.17, WHITE, 1.0);
                fill_polygon(img, &[at(dx - 0.17, 0.05), at(dx + 0.05, -0.45), at(dx + 0.15, -0.36), at(dx + 0.12, 0.05)], WHITE, 1.0);
            }
        }
    }
}

pub fn fill_circle(img: &mut RgbaImage, c: (f32, f32), r: f32, color: [u8; 3], alpha: f32) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    for y in ((c.1 - r - 1.0) as i32).max(0)..=((c.1 + r + 1.0) as i32).min(h - 1) {
        for x in ((c.0 - r - 1.0) as i32).max(0)..=((c.0 + r + 1.0) as i32).min(w - 1) {
            let d = ((x as f32 + 0.5 - c.0).powi(2) + (y as f32 + 0.5 - c.1).powi(2)).sqrt();
            let a = (r - d + 0.5).clamp(0.0, 1.0) * alpha;
            if a > 0.0 {
                blend(img.get_pixel_mut(x as u32, y as u32), color, a);
            }
        }
    }
}

/// Even-odd polygon fill with 4x4 supersampling.
pub fn fill_polygon(img: &mut RgbaImage, pts: &[(f32, f32)], color: [u8; 3], alpha: f32) {
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
                blend(img.get_pixel_mut(x as u32, y as u32), color, alpha * hits as f32 / 16.0);
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

pub fn blend(p: &mut Rgba<u8>, c: [u8; 3], a: f32) {
    for (ch, v) in p.0.iter_mut().zip(c) {
        *ch = (*ch as f32 * (1.0 - a) + v as f32 * a).round() as u8;
    }
}

/// Colour (`#rrggbb`) and inline SVG icon for a callout box.
pub fn callout_style(kind: Callout, accent: &str) -> (String, &'static str, &'static str) {
    let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
    match kind {
        Callout::Note => (hex(BLUE), "Note", SVG_INFO),
        Callout::Tip => (hex(GREEN), "Tip", SVG_STAR),
        Callout::Warning => (hex(AMBER), "Warning", SVG_WARNING),
        Callout::Important => (hex(RED), "Important", SVG_IMPORTANT),
        Callout::Quote => (accent.to_string(), "", SVG_QUOTE),
    }
}

// 48x48 icons; `currentColor` is the callout colour.
const SVG_INFO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><circle cx="24" cy="24" r="22" fill="currentColor"/><circle cx="24" cy="14" r="3.2" fill="#fff"/><rect x="21" y="20" width="6" height="16" rx="3" fill="#fff"/></svg>"##;
const SVG_STAR: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><circle cx="24" cy="24" r="22" fill="currentColor"/><path d="M24 10l4.1 8.6 9.4 1.2-6.9 6.5 1.8 9.3L24 31.1l-8.4 4.5 1.8-9.3-6.9-6.5 9.4-1.2z" fill="#fff"/></svg>"##;
const SVG_WARNING: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><path d="M24 3L46 42H2z" fill="currentColor" stroke="currentColor" stroke-width="3" stroke-linejoin="round"/><rect x="21" y="16" width="6" height="14" rx="3" fill="#fff"/><circle cx="24" cy="35.5" r="3.2" fill="#fff"/></svg>"##;
const SVG_IMPORTANT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><circle cx="24" cy="24" r="22" fill="currentColor"/><rect x="21" y="10" width="6" height="18" rx="3" fill="#fff"/><circle cx="24" cy="35" r="3.4" fill="#fff"/></svg>"##;
const SVG_QUOTE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><rect x="2" y="2" width="44" height="44" rx="12" fill="currentColor"/><path d="M13 31a5 5 0 1 1 5-5c0-6 3-10 7-12l1 2c-3 2-4 5-4 8a5 5 0 0 1-4 7zm14 0a5 5 0 1 1 5-5c0-6 3-10 7-12l1 2c-3 2-4 5-4 8a5 5 0 0 1-4 7z" fill="#fff"/></svg>"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stickers_draw_inside_their_badge() {
        for icon in [
            Sticker::Warning,
            Sticker::Important,
            Sticker::Info,
            Sticker::Tip,
            Sticker::Check,
            Sticker::Cross,
            Sticker::Question,
            Sticker::Quote,
        ] {
            let mut img = RgbaImage::from_pixel(120, 120, Rgba([0, 0, 0, 255]));
            draw_sticker(&mut img, (60.0, 60.0), 64.0, icon, [255, 92, 53]);
            assert_ne!(img.get_pixel(60, 40).0, [0, 0, 0, 255], "{icon:?} drew nothing near the top");
            assert_eq!(img.get_pixel(2, 2).0, [0, 0, 0, 255], "{icon:?} spilled into the corner");
        }
    }

    #[test]
    fn callout_svgs_are_standalone() {
        for k in [Callout::Note, Callout::Tip, Callout::Warning, Callout::Important, Callout::Quote] {
            let (color, _, svg) = callout_style(k, "#ff5c35");
            assert!(color.starts_with('#'));
            assert!(svg.starts_with("<svg") && svg.contains("currentColor"));
        }
    }
}
