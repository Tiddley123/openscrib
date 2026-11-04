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

/// Area shown to the model around a click, in source pixels, before zooming.
const CROP: (u32, u32) = (300, 190);
const ZOOM: u32 = 2;

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

/// Save a crop around the click, zoomed 2x so taskbar-sized icons are big
/// enough to recognise, with the click ring drawn on it.
pub fn click_crop(frame: &image::RgbaImage, click: &Click, out: &Path) -> Result<()> {
    let (fw, fh) = frame.dimensions();
    let (w, h) = (CROP.0.min(fw), CROP.1.min(fh));
    let x = (click.in_frame.x - w as i32 / 2).clamp(0, (fw - w) as i32);
    let y = (click.in_frame.y - h as i32 / 2).clamp(0, (fh - h) as i32);
    let crop = imageops::crop_imm(frame, x as u32, y as u32, w, h).to_image();
    let mut crop = imageops::resize(&crop, w * ZOOM, h * ZOOM, imageops::FilterType::CatmullRom);
    let at = Point::new((click.in_frame.x - x) * ZOOM as i32, (click.in_frame.y - y) * ZOOM as i32);
    draw_marker(&mut crop, at, MarkerStyle { radius: 44.0, ring: 5.0, color: [255, 92, 53], fill_alpha: 0.0 });
    crop.save(out)?;
    Ok(())
}

/// Whether OCR and the model name the same thing ("This PC" vs "This PC
/// icon"), so the exact OCR text can be kept.
pub fn same_label(ocr: &str, vision: &str) -> bool {
    let norm = |s: &str| s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let (o, v) = (norm(ocr), norm(vision));
    !o.is_empty() && (v.contains(&o) || o.contains(&v))
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
    // No example names and no OCR hint: a 3B model repeats whatever it is
    // shown ("Local Disk (C:) drive", "RSFOC") instead of looking.
    format!(
        "This is a zoomed-in part of a screenshot{app}. The user {} the item inside the orange circle. \
         What exactly is inside the circle? If it has a visible text label, reply with that text. If it is an \
         icon, reply with what it is, using the program's name for program icons. Reply with 1 to 5 words only.",
        verb(step.click.as_ref())
    )
}

/// Rewrite an accurate draft using the presenter's narration. No examples:
/// the model copies them. Its answer is checked by [`accept_polish`].
pub fn polish_prompt(step: &Step, draft: &str) -> String {
    let said = step.transcript.as_deref().unwrap_or_default().trim();
    if step.kind == StepKind::Note || step.narration_leads() {
        return format!(
            "Turn what a presenter said into one or two clear sentences for a how-to guide. Use only their words \
             and meaning; drop filler like \"okay\" or \"so now\".\nThe presenter said: \"{said}\"\nSentences:"
        );
    }
    format!(
        "A how-to guide step reads: {draft}\nWhile doing it, the presenter said: \"{said}\"\n\n\
         Rewrite the step to include any useful instruction or reason from what the presenter said. Keep the \
         action word and the **bold** name exactly. Use only what the presenter said; add nothing else. Drop \
         filler like \"okay\" or \"so now\" and anything that just repeats the step. At most two short \
         sentences.\nRewritten step:"
    )
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 4)
        .map(str::to_string)
        .collect()
}

/// A rewrite is kept only if it keeps the bold name and every longer word it
/// adds comes from the draft or the narration, so nothing is invented.
pub fn accept_polish(draft: &str, narration: &str, rewrite: &str) -> bool {
    let bold = draft.split("**").nth(1);
    if bold.is_some_and(|b| !rewrite.contains(&format!("**{b}**"))) {
        return false;
    }
    let known: Vec<String> = words(draft).into_iter().chain(words(narration)).collect();
    // Allow small inflections: "install" said, "installed" written.
    // Joining words a rewrite may add without changing the meaning.
    const GLUE: [&str; 16] = [
        "then", "where", "which", "when", "after", "before", "until", "while", "this", "that", "your", "with", "into",
        "from", "there", "will",
    ];
    words(rewrite)
        .iter()
        .filter(|w| !GLUE.contains(&w.as_str()))
        .all(|w| known.iter().any(|k| k == w || (k.len() >= 5 && w.len() >= 5 && k[..5] == w[..5])))
}

/// Without a usable rewrite: the draft, then the presenter's sentences that
/// add something (dropping the one that just restates the action).
pub fn combine(draft: &str, narration: &str) -> String {
    let label = draft.split("**").nth(1).map(str::to_lowercase);
    let extra: Vec<String> = narration
        .split_inclusive(['.', '!', '?'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| label.as_ref().is_none_or(|l| !s.to_lowercase().contains(l.as_str())))
        .map(openscrib_core::sentence)
        .collect();
    if extra.is_empty() { draft.to_string() } else { format!("{draft} {}", extra.join(" ")) }
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
    fn rejects_invented_rewrites() {
        let draft = "Click **Firefox**.";
        let said = "I'm going to open up Firefox. Wait for the updates to install.";
        assert!(accept_polish(draft, said, "Click **Firefox**, then wait for the updates to install."));
        assert!(!accept_polish(draft, said, "Click **Firefox**, then wait for it to finish installing."));
        assert!(!accept_polish(draft, said, "Click **Start**, then wait for the updates."));
        assert_eq!(combine(draft, said), "Click **Firefox**. Wait for the updates to install.");
    }

    #[test]
    fn cleans_replies() {
        assert_eq!(clean_reply("\"Start button\"\n", 40), Some("Start button".into()));
        assert_eq!(clean_reply("Title: Open the Firebird guide", 80), Some("Open the Firebird guide".into()));
        assert_eq!(clean_reply("1. Click **Start**.", 200), Some("Click **Start**.".into()));
        assert_eq!(clean_reply("   ", 10), None);
    }
}
