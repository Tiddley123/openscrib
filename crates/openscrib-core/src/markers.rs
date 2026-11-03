//! Click markers drawn onto frames. Markers are never baked into the stored
//! PNGs; they are rendered at export (and by `openscrib markers` for checking
//! capture accuracy).

use image::{Rgba, RgbaImage};

use crate::manifest::{Click, MouseButton, Point};

#[derive(Debug, Clone, Copy)]
pub struct MarkerStyle {
    pub radius: f32,
    pub ring: f32,
    pub color: [u8; 3],
    pub fill_alpha: f32,
}

impl Default for MarkerStyle {
    fn default() -> Self {
        // Wide enough to frame a 32 px toolbar icon with clear space inside, so
        // the target stays readable; no fill for the same reason.
        Self { radius: 30.0, ring: 4.0, color: [255, 92, 53], fill_alpha: 0.0 }
    }
}

impl MarkerStyle {
    /// `scale` is the monitor scale factor, so markers keep their size on HiDPI screens.
    pub fn for_click(click: &Click, scale: f32) -> Self {
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let mut s = Self::default();
        s.radius *= scale;
        s.ring *= scale;
        if click.button == MouseButton::Right {
            s.color = [53, 132, 255];
        }
        s
    }
}

/// Draw an anti-aliased ring (optionally filled) with a thin white halo so the
/// marker reads on both light and dark UIs.
pub fn draw_marker(img: &mut RgbaImage, at: Point, style: MarkerStyle) {
    let halo = style.ring + 2.0;
    let reach = style.radius + halo;
    let (w, h) = (img.width() as i32, img.height() as i32);
    let x0 = (at.x as f32 - reach).floor().max(0.0) as i32;
    let y0 = (at.y as f32 - reach).floor().max(0.0) as i32;
    let x1 = ((at.x as f32 + reach).ceil() as i32).min(w - 1);
    let y1 = ((at.y as f32 + reach).ceil() as i32).min(h - 1);

    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - (at.x as f32 + 0.5);
            let dy = y as f32 + 0.5 - (at.y as f32 + 0.5);
            let d = (dx * dx + dy * dy).sqrt();
            let off = (d - style.radius).abs();

            let halo_a = coverage(halo / 2.0 - off) * 0.9;
            let ring_a = coverage(style.ring / 2.0 - off);
            let fill_a = coverage(style.radius - d) * style.fill_alpha;

            let px = img.get_pixel_mut(x as u32, y as u32);
            blend(px, [255, 255, 255], halo_a);
            blend(px, style.color, fill_a.max(ring_a));
        }
    }
}

pub fn draw_click(img: &mut RgbaImage, click: &Click, scale: f32) {
    let style = MarkerStyle::for_click(click, scale);
    draw_marker(img, click.in_frame, style);
    if click.double {
        draw_marker(img, click.in_frame, MarkerStyle { radius: style.radius + style.ring * 2.5, ..style });
    }
}

fn coverage(signed: f32) -> f32 {
    (signed + 0.5).clamp(0.0, 1.0)
}

fn blend(px: &mut Rgba<u8>, c: [u8; 3], a: f32) {
    if a <= 0.0 {
        return;
    }
    for (ch, target) in px.0.iter_mut().zip(c) {
        *ch = (*ch as f32 * (1.0 - a) + target as f32 * a).round() as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_lands_on_click_and_clips_at_edges() {
        let mut img = RgbaImage::from_pixel(100, 100, Rgba([0, 0, 0, 255]));
        draw_marker(&mut img, Point::new(50, 50), MarkerStyle::default());
        let r = MarkerStyle::default().radius as u32;
        // On the ring, directly right of centre.
        assert!(img.get_pixel(50 + r, 50).0[0] > 200);
        // The clicked target itself stays untouched.
        assert_eq!(img.get_pixel(50, 50).0, [0, 0, 0, 255]);
        assert_eq!(img.get_pixel(50 + r / 2, 50).0, [0, 0, 0, 255]);
        // Far corner untouched.
        assert_eq!(img.get_pixel(0, 0).0, [0, 0, 0, 255]);
        // Must not panic near or past edges.
        draw_marker(&mut img, Point::new(0, 99), MarkerStyle::default());
        draw_marker(&mut img, Point::new(-30, 500), MarkerStyle::default());
    }
}
