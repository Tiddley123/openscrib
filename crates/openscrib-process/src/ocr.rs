//! Reading the label under a click (and any tooltip shown before it) with
//! the pure-Rust `ocrs` engine. Only a small region around the cursor is
//! read, upscaled so small UI fonts survive recognition.

use std::path::Path;

use anyhow::{Context, Result};
use image::imageops::{self, FilterType};
use image::RgbaImage;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use openscrib_core::{Point, Rect};
use rten::Model;

/// Region read around a click, in source pixels.
const LABEL_REGION: (u32, u32) = (700, 220);
/// Tooltips appear below and to the right of the pointer.
const TOOLTIP_REGION: (i32, i32, u32, u32) = (-120, -10, 560, 150);
const UPSCALE: u32 = 2;

#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    /// Frame coordinates.
    pub rect: Rect,
    pub text: String,
}

pub struct Ocr {
    engine: OcrEngine,
}

impl Ocr {
    pub fn load(detection: &Path, recognition: &Path) -> Result<Self> {
        let engine = OcrEngine::new(OcrEngineParams {
            detection_model: Some(Model::load_file(detection).context("loading OCR detection model")?),
            recognition_model: Some(Model::load_file(recognition).context("loading OCR recognition model")?),
            ..Default::default()
        })?;
        Ok(Self { engine })
    }

    /// Lines of words found in `region` of `img`, in frame coordinates.
    pub fn read(&self, img: &RgbaImage, region: Rect) -> Result<Vec<Vec<Word>>> {
        let r = clamp(region, img.dimensions());
        if r.w < 8 || r.h < 8 {
            return Ok(Vec::new());
        }
        let crop = imageops::crop_imm(img, r.x as u32, r.y as u32, r.w, r.h).to_image();
        let big = imageops::resize(&crop, r.w * UPSCALE, r.h * UPSCALE, FilterType::CatmullRom);
        let rgb: Vec<u8> = big.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
        let input = self.engine.prepare_input(ImageSource::from_bytes(&rgb, big.dimensions())?)?;
        let words = self.engine.detect_words(&input)?;
        let lines = self.engine.find_text_lines(&input, &words);
        let texts = self.engine.recognize_text(&input, &lines)?;

        let mut out = Vec::new();
        for line in texts.into_iter().flatten() {
            let mut ws = Vec::new();
            for w in line.words() {
                let text = w.to_string().trim().to_string();
                if text.is_empty() {
                    continue;
                }
                let b = w.bounding_rect();
                ws.push(Word {
                    rect: Rect {
                        x: r.x + b.left() / UPSCALE as i32,
                        y: r.y + b.top() / UPSCALE as i32,
                        w: (b.width().max(1) as u32).div_ceil(UPSCALE),
                        h: (b.height().max(1) as u32).div_ceil(UPSCALE),
                    },
                    text,
                });
            }
            if !ws.is_empty() {
                out.push(ws);
            }
        }
        Ok(out)
    }

    /// The label under (or nearest to) the click.
    pub fn label_at(&self, img: &RgbaImage, at: Point) -> Result<Option<String>> {
        let region = Rect {
            x: at.x - LABEL_REGION.0 as i32 / 2,
            y: at.y - LABEL_REGION.1 as i32 / 2,
            w: LABEL_REGION.0,
            h: LABEL_REGION.1,
        };
        Ok(pick_label(&self.read(img, region)?, at))
    }

    /// Text visible near the pointer before the click that is gone (or
    /// different) at the click: typically a tooltip.
    pub fn tooltip(&self, before: &RgbaImage, at_click: &RgbaImage, at: Point) -> Result<Option<String>> {
        let (dx, dy, w, h) = TOOLTIP_REGION;
        let region = Rect { x: at.x + dx, y: at.y + dy, w, h };
        let before_lines = joined(&self.read(before, region)?);
        let after_lines = joined(&self.read(at_click, region)?);
        let fresh: Vec<String> = before_lines.into_iter().filter(|l| !after_lines.contains(l)).collect();
        let text = fresh.join(" ");
        Ok(clean(&text))
    }
}

fn joined(lines: &[Vec<Word>]) -> Vec<String> {
    lines.iter().map(|l| l.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")).collect()
}

/// Choose the words that make up the clicked control: the line under the
/// point (or the closest within reach), cut down to the cluster of words
/// around the point so neighbouring columns or menu items aren't included.
pub fn pick_label(lines: &[Vec<Word>], at: Point) -> Option<String> {
    const REACH: i32 = 70;
    let dist = |r: &Rect| {
        let dx = (r.x - at.x).max(0).max(at.x - (r.x + r.w as i32));
        let dy = (r.y - at.y).max(0).max(at.y - (r.y + r.h as i32));
        // Vertical distance matters more: labels sit on the clicked row.
        dx + dy * 3
    };
    let (line, word) = lines
        .iter()
        .flat_map(|l| l.iter().enumerate().map(move |(i, w)| (l, i, w)))
        .map(|(l, i, w)| (dist(&w.rect), l, i))
        .filter(|(d, ..)| *d <= REACH)
        .min_by_key(|(d, ..)| *d)
        .map(|(_, l, i)| (l, i))?;

    // Grow left and right while the gap to the next word looks like a space
    // within one label rather than a gap between columns.
    let h = line[word].rect.h.max(8) as i32;
    let gap = |a: &Word, b: &Word| b.rect.x - (a.rect.x + a.rect.w as i32);
    let (mut lo, mut hi) = (word, word);
    while lo > 0 && gap(&line[lo - 1], &line[lo]) < h {
        lo -= 1;
    }
    while hi + 1 < line.len() && gap(&line[hi], &line[hi + 1]) < h {
        hi += 1;
    }
    let text = line[lo..=hi].iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
    clean(&text)
}

/// Drop OCR noise: stray symbols, too-short or implausibly long results.
fn clean(s: &str) -> Option<String> {
    let t = s.trim_matches(|c: char| !c.is_alphanumeric() && c != ')' && c != '.').trim();
    let letters = t.chars().filter(|c| c.is_alphanumeric()).count();
    (letters >= 2 && t.chars().count() <= 60).then(|| t.to_string())
}

fn clamp(r: Rect, (fw, fh): (u32, u32)) -> Rect {
    let x0 = r.x.clamp(0, fw as i32);
    let y0 = r.y.clamp(0, fh as i32);
    let x1 = (r.x + r.w as i32).clamp(0, fw as i32);
    let y1 = (r.y + r.h as i32).clamp(0, fh as i32);
    Rect { x: x0, y: y0, w: (x1 - x0) as u32, h: (y1 - y0) as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(x: i32, y: i32, wd: u32, t: &str) -> Word {
        Word { rect: Rect { x, y, w: wd, h: 14 }, text: t.into() }
    }

    #[test]
    fn picks_the_clicked_label_not_the_whole_row() {
        let lines = vec![
            vec![w(10, 100, 60, "Documents")],
            vec![w(10, 130, 30, "This"), w(44, 130, 20, "PC"), w(300, 130, 90, "27/10/2026"), w(400, 130, 40, "2:49")],
            vec![w(10, 160, 50, "Music")],
        ];
        assert_eq!(pick_label(&lines, Point::new(50, 137)), Some("This PC".into()));
        assert_eq!(pick_label(&lines, Point::new(420, 136)), Some("27/10/2026 2:49".into()));
        // Nothing within reach.
        assert_eq!(pick_label(&lines, Point::new(900, 600)), None);
    }

    #[test]
    fn cleans_noise() {
        assert_eq!(clean("  » Settings  "), Some("Settings".into()));
        assert_eq!(clean("|"), None);
        assert_eq!(clean("Help (F1)"), Some("Help (F1)".into()));
    }
}
