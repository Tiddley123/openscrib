//! Talks to the `openscrib-vision` helper process (llama.cpp + Qwen2.5-VL):
//! naming icon-only click targets from a crop, and writing step sentences
//! and the guide title from everything gathered so far.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use image::imageops;
use openscrib_core::markers::{MarkerStyle, draw_marker};
use openscrib_core::{Click, MouseButton, Point, Step, StepKind, is_remote_viewer};
use serde_json::json;

#[cfg(windows)]
const HELPER: &str = "openscrib-vision.exe";
#[cfg(not(windows))]
const HELPER: &str = "openscrib-vision";

/// Area shown to the model around a click, in source pixels.
const CROP: (u32, u32) = (640, 400);

/// The helper executable, next to the running one.
pub fn helper_path() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?.parent()?.join(HELPER);
    p.is_file().then_some(p)
}

pub struct Vision {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Vision {
    pub fn start(model: &Path, mmproj: &Path) -> Result<Self> {
        let exe = helper_path().with_context(|| format!("{HELPER} not found next to this program"))?;
        let mut cmd = Command::new(exe);
        cmd.arg("--model").arg(model).arg("--mmproj").arg(mmproj);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().context("starting the vision helper")?;
        let stdin = child.stdin.take().context("helper stdin")?;
        let mut stdout = BufReader::new(child.stdout.take().context("helper stdout")?);
        let mut first = String::new();
        stdout.read_line(&mut first)?;
        if !first.contains("\"ready\"") {
            let _ = child.kill();
            bail!("the vision model failed to load (is the download complete?)");
        }
        Ok(Self { child, stdin, stdout, next: 0 })
    }

    pub fn ask(&mut self, prompt: &str, image: Option<&Path>, max_tokens: usize) -> Result<String> {
        self.next += 1;
        let id = self.next.to_string();
        let req = json!({
            "id": id,
            "prompt": prompt,
            "image": image.map(|p| p.to_string_lossy().replace('\\', "/")),
            "max_tokens": max_tokens,
        });
        writeln!(self.stdin, "{req}")?;
        self.stdin.flush()?;
        let mut line = String::new();
        if self.stdout.read_line(&mut line)? == 0 {
            bail!("the vision helper stopped unexpectedly");
        }
        let v: serde_json::Value = serde_json::from_str(&line).context("reading helper reply")?;
        if let Some(e) = v["error"].as_str() {
            return Err(anyhow!("vision model: {e}"));
        }
        Ok(v["text"].as_str().unwrap_or_default().to_string())
    }
}

impl Drop for Vision {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// OCR results that are probably not the thing that was clicked: nothing,
/// or a size/date/count read from a subtitle line.
pub fn label_is_weak(label: Option<&str>) -> bool {
    let Some(l) = label else { return true };
    let digits = l.chars().filter(|c| c.is_ascii_digit()).count();
    let letters = l.chars().filter(|c| c.is_alphabetic()).count();
    let lower = l.to_ascii_lowercase();
    digits * 2 > letters || [" gb", " mb", " kb", " free", " items", " am", " pm"].iter().any(|u| lower.contains(u))
}

/// Save a crop around the click (with the click ring drawn) for the model.
pub fn click_crop(frame: &image::RgbaImage, click: &Click, out: &Path) -> Result<()> {
    let (fw, fh) = frame.dimensions();
    let (w, h) = (CROP.0.min(fw), CROP.1.min(fh));
    let x = (click.in_frame.x - w as i32 / 2).clamp(0, (fw - w) as i32);
    let y = (click.in_frame.y - h as i32 / 2).clamp(0, (fh - h) as i32);
    let mut crop = imageops::crop_imm(frame, x as u32, y as u32, w, h).to_image();
    let at = Point::new(click.in_frame.x - x, click.in_frame.y - y);
    draw_marker(&mut crop, at, MarkerStyle { radius: 26.0, ring: 4.0, color: [255, 92, 53], fill_alpha: 0.0 });
    crop.save(out)?;
    Ok(())
}

fn verb(click: Option<&Click>) -> &'static str {
    match click {
        Some(c) if c.double => "double-clicked",
        Some(Click { button: MouseButton::Right, .. }) => "right-clicked",
        _ => "clicked",
    }
}

pub fn identify_prompt(step: &Step) -> String {
    let app = step
        .window
        .as_ref()
        .filter(|w| !is_remote_viewer(&w.process) && !w.title.trim().is_empty())
        .map(|w| format!(" in the window \"{}\"", w.title.trim()))
        .unwrap_or_default();
    format!(
        "This is part of a screenshot{app}. The user {} the item inside the orange circle. \
         What is that item? Reply with its visible label, or if it has none a short name such as \
         \"Start button\", \"Settings gear icon\" or \"Local Disk (C:) drive\". Reply with 2 to 6 words only.",
        verb(step.click.as_ref())
    )
}

/// Rewrite an accurate draft using the presenter's narration. The draft
/// carries the facts (action and target), so the model only adds the why.
pub fn polish_prompt(step: &Step, draft: &str) -> String {
    let said = step.transcript.as_deref().unwrap_or_default().trim();
    let what = if step.kind == StepKind::Note {
        "Turn what the presenter said into one or two clear sentences for a how-to guide.".to_string()
    } else {
        format!(
            "Improve a how-to guide step with what the presenter said.\n\n\
             Example:\n\
             Step: Click **Settings**.\n\
             The presenter said: \"so now we hit settings, that's where the backup options are\"\n\
             Improved: Click **Settings**, where the backup options are.\n\n\
             Now do the same. Keep the action word and the **bold** name exactly. Add the reason or detail \
             the presenter gives; leave out filler like \"okay\" or \"so now\". At most two short sentences.\n\
             Step: {draft}"
        )
    };
    format!("{what}\nThe presenter said: \"{said}\"\nImproved:")
}

pub fn title_prompt(steps: &[String]) -> String {
    let list: Vec<String> = steps.iter().take(15).enumerate().map(|(i, s)| format!("{}. {s}", i + 1)).collect();
    format!(
        "Here are the steps of a how-to guide:\n{}\n\nWrite a title for the guide, at most 8 words, like \
         \"Open the Firebird quick start guide\". Reply with the title only.",
        list.join("\n")
    )
}

/// Model replies sometimes come quoted, prefixed or with trailing chatter.
pub fn clean_reply(s: &str, max_chars: usize) -> Option<String> {
    let first = s.lines().map(str::trim).find(|l| !l.is_empty())?;
    let first = first
        .trim_start_matches(|c: char| c == '-' || c == '*' || c.is_ascii_digit() || c == '.' || c == ' ')
        .trim_matches(|c| c == '"' || c == '\'' || c == '“' || c == '”');
    let first = ["Title:", "Step:", "Improved:"].iter().find_map(|p| first.strip_prefix(p)).unwrap_or(first).trim();
    (!first.is_empty() && first.chars().count() <= max_chars).then(|| first.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weak_labels() {
        assert!(label_is_weak(None));
        assert!(label_is_weak(Some("15.9 GB free of 126 GB")));
        assert!(label_is_weak(Some("27/10/2026 2:49 PM")));
        assert!(!label_is_weak(Some("This PC")));
        assert!(!label_is_weak(Some("Firebird-2.5-QuickStart.pdf")));
    }

    #[test]
    fn cleans_replies() {
        assert_eq!(clean_reply("\"Start button\"\n", 40), Some("Start button".into()));
        assert_eq!(clean_reply("Title: Open the Firebird guide", 80), Some("Open the Firebird guide".into()));
        assert_eq!(clean_reply("1. Click **Start**.", 200), Some("Click **Start**.".into()));
        assert_eq!(clean_reply("   ", 10), None);
    }
}
