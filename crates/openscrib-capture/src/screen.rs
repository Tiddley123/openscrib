//! Full-monitor still capture.

use anyhow::{Context, Result};
use image::RgbaImage;
use openscrib_core::{MonitorInfo, Point, Rect};
use xcap::Monitor;

pub fn monitors() -> Result<Vec<MonitorInfo>> {
    Monitor::all()
        .context("listing monitors")?
        .iter()
        .map(info)
        .collect()
}

pub(crate) fn info(m: &Monitor) -> Result<MonitorInfo> {
    Ok(MonitorInfo {
        id: m.id()?,
        name: m.friendly_name().or_else(|_| m.name())?,
        rect: Rect { x: m.x()?, y: m.y()?, w: m.width()?, h: m.height()? },
        scale: m.scale_factor().unwrap_or(1.0),
        primary: m.is_primary().unwrap_or(false),
    })
}

/// Capture the whole monitor containing `p`.
pub fn capture_at(p: Point) -> Result<(MonitorInfo, RgbaImage)> {
    let m = Monitor::from_point(p.x, p.y)
        .or_else(|_| {
            // Points on a gap between monitors: fall back to the primary.
            Monitor::all()?
                .into_iter()
                .find(|m| m.is_primary().unwrap_or(false))
                .ok_or_else(|| xcap::XCapError::new("no primary monitor"))
        })
        .context("finding monitor")?;
    let img = m.capture_image().context("capturing monitor")?;
    Ok((info(&m)?, img))
}

/// True when the frame is (near) a single colour, e.g. a black frame from a
/// remote session that blocks capture by policy. Samples a grid, so it is
/// cheap enough to run on every capture.
pub fn looks_blank(img: &RgbaImage) -> bool {
    const GRID: u32 = 24;
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return true;
    }
    let first = img.get_pixel(w / (GRID * 2), h / (GRID * 2)).0;
    for gy in 0..GRID {
        for gx in 0..GRID {
            let px = img.get_pixel(gx * w / GRID + w / (GRID * 2), gy * h / GRID + h / (GRID * 2)).0;
            let diff: u32 = (0..3).map(|i| px[i].abs_diff(first[i]) as u32).sum();
            if diff > 24 {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn blank_detection() {
        let mut img = RgbaImage::from_pixel(640, 480, Rgba([3, 3, 3, 255]));
        assert!(looks_blank(&img));
        for x in 300..340 {
            for y in 220..260 {
                img.put_pixel(x, y, Rgba([240, 240, 240, 255]));
            }
        }
        assert!(!looks_blank(&img));
    }
}
