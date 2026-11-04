//! `project.json`: the manifest that ties every asset together by timestamp.
//!
//! Raw stage outputs (`transcript`, `ocr`, `vision`) stay separate from the
//! final `text`; re-running a stage must never overwrite a step whose
//! `text_edited` is set.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Bump when the schema changes and add a migration in `Manifest::migrate`.
pub const MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    pub fn offset_from(self, origin: Point) -> Point {
        Point::new(self.x - origin.x, self.y - origin.y)
    }

    pub fn dist2(self, other: Point) -> i64 {
        let dx = (self.x - other.x) as i64;
        let dy = (self.y - other.y) as i64;
        dx * dx + dy * dy
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn origin(&self) -> Point {
        Point::new(self.x, self.y)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x
            && p.y >= self.y
            && (p.x as i64) < self.x as i64 + self.w as i64
            && (p.y as i64) < self.y as i64 + self.h as i64
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    /// Physical pixels in virtual-desktop coordinates.
    pub rect: Rect,
    pub scale: f32,
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Visible frame bounds, screen coordinates.
    pub rect: Rect,
    /// Client area, screen coordinates. Used to crop away remote-desktop
    /// toolbars and borders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<Rect>,
    pub title: String,
    /// Executable name, e.g. `TeamViewer.exe`.
    pub process: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Click {
    /// Screen coordinates (physical pixels).
    pub x: i32,
    pub y: i32,
    pub button: MouseButton,
    #[serde(default, skip_serializing_if = "is_false")]
    pub double: bool,
    /// Relative to `Step::window.rect`, so markers survive scaled remote views.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_window: Option<Point>,
    /// Relative to the top-left of `Step::frame`.
    pub in_frame: Point,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepKind {
    Click,
    Typed,
    Shortcut,
    /// Added in the editor: text, narration and optionally an imported image.
    Note,
}

/// Highlighted box style for a step in written guides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Callout {
    Note,
    Tip,
    Warning,
    Important,
    Quote,
}

/// Pictographs that can be stuck onto a screenshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sticker {
    Warning,
    Important,
    Info,
    Tip,
    Check,
    Cross,
    Question,
    Quote,
}

/// One stretch of transcribed narration, assigned to a step by id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NarrationSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clip {
    pub start_ms: u64,
    pub end_ms: u64,
}

/// Annotations are in frame pixel coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Annotation {
    Marker { at: Point },
    Arrow { from: Point, to: Point },
    Box { rect: Rect },
    Blur {
        rect: Rect,
        /// Shared by copies carried forward to later steps, so deleting one
        /// can remove the rest.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<u32>,
    },
    /// `size` is the badge diameter in frame pixels.
    Sticker { at: Point, icon: Sticker, size: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub id: u32,
    pub t_ms: u64,
    pub kind: StepKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click: Option<Click>,
    /// Typed text or a shortcut such as `Ctrl+C`. Special keys are written in
    /// braces: `hello{Enter}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_frame: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// Label read under the click.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr: Option<String>,
    /// Text that appeared near the cursor before the click (tooltips).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<String>,
    pub text: String,
    #[serde(default)]
    pub text_edited: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<Clip>,
    /// Region of the frame to show in exported guides; `None` uses the
    /// export default (usually the target window).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<Rect>,
    /// Shared by crops carried forward to later steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop_group: Option<u32>,
    #[serde(default)]
    pub annotations: Vec<Annotation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callout: Option<Callout>,
    /// Hide the automatic click marker.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hide_marker: bool,
    /// Left out of the guide, video and voiceover, but kept in place: its
    /// screen time is still in the recording, so redactions may need setting
    /// on it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub deleted: bool,
}

impl Step {
    pub fn new(id: u32, t_ms: u64, kind: StepKind) -> Self {
        Self {
            id,
            t_ms,
            kind,
            click: None,
            keys: None,
            monitor: None,
            window: None,
            frame: None,
            pre_frame: None,
            transcript: None,
            ocr: None,
            tooltip: None,
            vision: None,
            text: String::new(),
            text_edited: false,
            clip: None,
            crop: None,
            crop_group: None,
            annotations: Vec::new(),
            callout: None,
            hide_marker: false,
            deleted: false,
        }
    }

    /// Whether the step's text should come from what was said rather than a
    /// name for the click target. The presenter knows what they clicked; the
    /// vision model only guesses from an icon, so its guess gives way to any
    /// narration. Names read as real text (OCR) still stand.
    pub fn narration_leads(&self) -> bool {
        self.kind == StepKind::Click
            && self.vision.as_deref().is_some_and(|v| !v.trim().is_empty())
            && self.transcript.as_deref().is_some_and(|t| !t.trim().is_empty())
    }

    /// Draft text without a language model: the label read under the click
    /// (OCR), then the tooltip, then the narration, then the window title.
    pub fn draft_text(&self) -> String {
        if self.narration_leads()
            && let Some(said) = self.transcript.as_deref().map(sentence)
        {
            return said;
        }
        let label = self
            .vision
            .as_deref()
            .or(self.ocr.as_deref())
            .or(self.tooltip.as_deref())
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let narration = self.transcript.as_deref().map(sentence).filter(|t| !t.is_empty());
        let window = self
            .window
            .as_ref()
            .filter(|w| !is_remote_viewer(&w.process))
            .map(|w| w.title.trim())
            .filter(|t| !t.is_empty());
        match self.kind {
            StepKind::Click => {
                let verb = match self.click.as_ref() {
                    Some(c) if c.double => "Double-click",
                    Some(Click { button: MouseButton::Right, .. }) => "Right-click",
                    Some(Click { button: MouseButton::Middle, .. }) => "Middle-click",
                    _ => "Click",
                };
                match (label, narration, window) {
                    (Some(l), _, _) => format!("{verb} {}.", target_phrase(l)),
                    (None, Some(n), _) => n,
                    (None, None, Some(t)) => format!("{verb} in **{t}**."),
                    (None, None, None) => format!("{verb} here."),
                }
            }
            StepKind::Typed => format!("Type `{}`.", self.keys.as_deref().unwrap_or("")),
            StepKind::Shortcut => format!("Press **{}**.", self.keys.as_deref().unwrap_or("")),
            StepKind::Note => narration.unwrap_or_default(),
        }
    }

    /// Replace `text` with a fresh draft unless the user has edited it.
    pub fn refresh_text(&mut self) {
        if !self.text_edited {
            self.text = self.draft_text();
        }
    }
}

/// The continuous screen recording. Block timestamps in `file` are on the
/// recording clock, so `Step::t_ms` is also the step's position in the video.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoTrack {
    pub file: String,
    pub monitor: u32,
    pub width: u32,
    pub height: u32,
    pub max_fps: u32,
    /// Frames actually encoded; unchanged frames are skipped.
    pub frames: u64,
    pub first_ms: u64,
}

/// Narration, stored as the Opus track of the same file as the video.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioTrack {
    pub file: String,
    pub device: String,
    /// Device rate before resampling to Opus's 48 kHz.
    pub device_rate: u32,
    pub channels: u8,
    pub first_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub app_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Highlight colour for markers, shapes and step badges, as `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    pub duration_ms: u64,
    pub monitors: Vec<MonitorInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoTrack>,
    /// The part of the screen exported videos show, in video pixels. `None`
    /// shows all of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_crop: Option<Rect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioTrack>,
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Every transcribed segment, assigned or not, for review and captions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub narration: Vec<NarrationSegment>,
}

impl Manifest {
    pub fn new(app_version: impl Into<String>) -> Self {
        Self {
            version: MANIFEST_VERSION,
            app_version: app_version.into(),
            title: None,
            accent: None,
            created: OffsetDateTime::now_utc(),
            duration_ms: 0,
            monitors: Vec::new(),
            video: None,
            video_crop: None,
            audio: None,
            steps: Vec::new(),
            warnings: Vec::new(),
            narration: Vec::new(),
        }
    }

    pub fn from_json(bytes: &[u8]) -> serde_json::Result<Self> {
        let mut m: Manifest = serde_json::from_slice(bytes)?;
        m.migrate();
        Ok(m)
    }

    pub fn to_json(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec_pretty(self)
    }

    fn migrate(&mut self) {
        // No migrations yet; version 1 is the first schema.
        self.version = MANIFEST_VERSION;
    }

    /// The project's highlight colour, falling back to the default orange.
    pub fn accent_rgb(&self) -> [u8; 3] {
        self.accent.as_deref().and_then(parse_hex).unwrap_or(DEFAULT_ACCENT)
    }

    pub fn accent_hex(&self) -> String {
        let [r, g, b] = self.accent_rgb();
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    /// Base name for exported files: the title, or "guide" when untitled.
    pub fn file_stem(&self) -> String {
        let s = self.title.as_deref().map(safe_name).unwrap_or_default();
        if s.is_empty() { "guide".into() } else { s }
    }

    /// Rebuild each step's `transcript` from the narration assigned to it.
    pub fn sync_transcripts(&mut self) {
        for step in &mut self.steps {
            let text: Vec<&str> = self
                .narration
                .iter()
                .filter(|s| s.step == Some(step.id))
                .map(|s| s.text.trim())
                .collect();
            step.transcript = (!text.is_empty()).then(|| text.join(" "));
        }
    }

    /// Next unused step id, for steps added in the editor.
    pub fn next_step_id(&self) -> u32 {
        self.steps.iter().map(|s| s.id).max().unwrap_or(0) + 1
    }

    pub fn monitor(&self, id: u32) -> Option<&MonitorInfo> {
        self.monitors.iter().find(|m| m.id == id)
    }
}

pub const DEFAULT_ACCENT: [u8; 3] = [255, 92, 53];

/// Parse `#rgb` or `#rrggbb`.
pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().strip_prefix('#')?;
    let v = u32::from_str_radix(h, 16).ok()?;
    match h.len() {
        6 => Some([(v >> 16) as u8, (v >> 8) as u8, v as u8]),
        3 => Some([((v >> 8) & 0xF) as u8 * 17, ((v >> 4) & 0xF) as u8 * 17, (v & 0xF) as u8 * 17]),
        _ => None,
    }
}

/// A title made safe as a Windows file or folder name (empty if nothing
/// usable is left).
pub fn safe_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| if r#"<>:"/|?*"#.contains(c) || c.is_control() { '-' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']);
    cleaned.chars().take(80).collect::<String>().trim().to_string()
}

/// "Start button" reads better as "the **Start** button": keep the kind of
/// control outside the bold name.
pub fn target_phrase(label: &str) -> String {
    const KINDS: [&str; 10] = ["button", "icon", "drive", "tab", "menu", "field", "folder", "link", "checkbox", "option"];
    let label = label.trim();
    if let Some((name, kind)) = label.rsplit_once(' ')
        && KINDS.contains(&kind.to_ascii_lowercase().as_str())
        && !name.is_empty()
    {
        let name = name.strip_prefix("the ").or_else(|| name.strip_prefix("The ")).unwrap_or(name);
        return format!("the **{name}** {}", kind.to_ascii_lowercase());
    }
    format!("**{label}**")
}

/// The OS shell's own strip of app buttons: the Windows taskbar (a window
/// owned by Explorer) or the macOS Dock. A click there says nothing on its
/// own, so step text is named after the app it opens instead.
pub fn is_shell_bar(process: &str) -> bool {
    process.eq_ignore_ascii_case("explorer.exe") || process.eq_ignore_ascii_case("Dock")
}

/// Window titles of remote-desktop viewers describe the connection, not
/// what was clicked, so they make poor step text.
pub fn is_remote_viewer(process: &str) -> bool {
    const VIEWERS: [&str; 8] = [
        "teamviewer", "mstsc", "anydesk", "rustdesk", "vncviewer", "parsec", "darnellcloud", "msrdc",
    ];
    let p = process.to_ascii_lowercase();
    VIEWERS.iter().any(|v| p.contains(v))
}

/// Tidy a transcript snippet into one sentence: trimmed, capitalised and
/// ending in punctuation.
pub fn sentence(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return String::new() };
    let mut out: String = first.to_uppercase().chain(chars).collect();
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        let mut m = Manifest::new("0.1.0");
        let mut s = Step::new(1, 12450, StepKind::Click);
        s.click = Some(Click {
            x: 812,
            y: 344,
            button: MouseButton::Left,
            double: false,
            in_window: Some(Point::new(812, 304)),
            in_frame: Point::new(812, 344),
        });
        s.window = Some(WindowInfo {
            rect: Rect { x: 0, y: 40, w: 1920, h: 1040 },
            client: None,
            title: "Settings".into(),
            process: "TeamViewer.exe".into(),
        });
        s.frame = Some("frames/0001.png".into());
        s.refresh_text();
        m.steps.push(s);
        m
    }

    #[test]
    fn round_trips() {
        let m = sample();
        let back = Manifest::from_json(&m.to_json().unwrap()).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn edited_text_survives_refresh() {
        let mut s = sample().steps.remove(0);
        assert_eq!(s.text, "Click here.");
        s.text = "Open the cog menu.".into();
        s.text_edited = true;
        s.refresh_text();
        assert_eq!(s.text, "Open the cog menu.");
    }

    #[test]
    fn draft_prefers_ocr_then_narration() {
        let mut s = sample().steps.remove(0);
        s.window.as_mut().unwrap().title = "Rovamp - Remote Desktop".into();
        assert_eq!(s.draft_text(), "Click here.");
        s.transcript = Some(" now open this pc ".into());
        assert_eq!(s.draft_text(), "Now open this pc.");
        s.ocr = Some("This PC".into());
        assert_eq!(s.draft_text(), "Click **This PC**.");
        s.window.as_mut().unwrap().process = "notepad.exe".into();
        s.ocr = None;
        s.transcript = None;
        assert_eq!(s.draft_text(), "Click in **Rovamp - Remote Desktop**.");
    }

    #[test]
    fn control_kinds_move_outside_the_bold() {
        assert_eq!(target_phrase("Start button"), "the **Start** button");
        assert_eq!(target_phrase("Local Disk (C:) drive"), "the **Local Disk (C:)** drive");
        assert_eq!(target_phrase("This PC"), "**This PC**");
        assert_eq!(target_phrase("button"), "**button**");
    }

    #[test]
    fn narration_beats_an_icon_guess_but_not_real_text() {
        let mut s = sample().steps.remove(0);
        s.vision = Some("Instagram".into());
        assert_eq!(s.draft_text(), "Click **Instagram**.");
        s.transcript = Some("open darnellcloud connect".into());
        assert_eq!(s.draft_text(), "Open darnellcloud connect.");
        s.vision = None;
        s.ocr = Some("This PC".into());
        assert_eq!(s.draft_text(), "Click **This PC**.");
    }

    #[test]
    fn accent_colours() {
        let mut m = Manifest::new("t");
        assert_eq!(m.accent_hex(), "#ff5c35");
        m.accent = Some("#0a84FF".into());
        assert_eq!(m.accent_rgb(), [10, 132, 255]);
        m.accent = Some("#f00".into());
        assert_eq!(m.accent_rgb(), [255, 0, 0]);
        m.accent = Some("teal".into());
        assert_eq!(m.accent_rgb(), DEFAULT_ACCENT);
    }

    #[test]
    fn omits_empty_stage_outputs() {
        let json = String::from_utf8(sample().to_json().unwrap()).unwrap();
        assert!(!json.contains("transcript"));
        assert!(!json.contains("\"double\""));
    }
}
