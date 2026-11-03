//! One user-facing quality setting (1..=100) mapped onto H.264 encoder
//! limits, shared by recording and export so the slider means the same thing
//! in both.

pub const DEFAULT_QUALITY: u8 = 75;

/// Quantiser range: lower is sharper. 0..=51 per H.264.
pub fn qp_range(quality: u8) -> (u8, u8) {
    let q = quality.clamp(1, 100) as f32;
    let max = (44.0 - q * 0.2).round() as u8; // 44 at the bottom, 24 at the top
    (max.saturating_sub(12).max(10), max)
}

/// Bit budget for moving content. Static screens cost almost nothing either
/// way because unchanged frames are skipped.
pub fn bitrate(quality: u8, width: u32, height: u32, fps: u32) -> u32 {
    let q = quality.clamp(1, 100) as f64 / 100.0;
    let bits_per_pixel = 0.02 + 0.28 * q * q;
    let bps = width as f64 * height as f64 * fps.max(1) as f64 * bits_per_pixel;
    bps.clamp(500_000.0, 40_000_000.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn higher_quality_means_lower_qp_and_more_bits() {
        let (lo_min, lo_max) = qp_range(10);
        let (hi_min, hi_max) = qp_range(100);
        assert!(hi_max < lo_max && hi_min <= lo_min);
        assert_eq!(qp_range(100), (12, 24));
        assert!(lo_max <= 51);
        assert!(bitrate(90, 1920, 1080, 15) > bitrate(30, 1920, 1080, 15));
        assert_eq!(qp_range(0), qp_range(1));
    }
}
