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
    Blur { rect: Rect },
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<String>,
    pub text: String,
    #[serde(default)]
    pub text_edited: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<Clip>,
    #[serde(default)]
    pub annotations: Vec<Annotation>,
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
            vision: None,
            text: String::new(),
            text_edited: false,
            clip: None,
            annotations: Vec::new(),
        }
    }

    /// Draft text from capture data alone (the zero-model core tier).
    pub fn draft_text(&self) -> String {
        let target = self
            .window
            .as_ref()
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
                match target {
                    Some(t) => format!("{verb} in **{t}**."),
                    None => format!("{verb} here."),
                }
            }
            StepKind::Typed => format!("Type `{}`.", self.keys.as_deref().unwrap_or("")),
            StepKind::Shortcut => format!("Press **{}**.", self.keys.as_deref().unwrap_or("")),
        }
    }

    /// Replace `text` with a fresh draft unless the user has edited it.
    pub fn refresh_text(&mut self) {
        if !self.text_edited {
            self.text = self.draft_text();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub app_version: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    pub duration_ms: u64,
    pub monitors: Vec<MonitorInfo>,
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl Manifest {
    pub fn new(app_version: impl Into<String>) -> Self {
        Self {
            version: MANIFEST_VERSION,
            app_version: app_version.into(),
            created: OffsetDateTime::now_utc(),
            duration_ms: 0,
            monitors: Vec::new(),
            steps: Vec::new(),
            warnings: Vec::new(),
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

    pub fn monitor(&self, id: u32) -> Option<&MonitorInfo> {
        self.monitors.iter().find(|m| m.id == id)
    }
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
        assert_eq!(s.text, "Click in **Settings**.");
        s.text = "Open the cog menu.".into();
        s.text_edited = true;
        s.refresh_text();
        assert_eq!(s.text, "Open the cog menu.");
    }

    #[test]
    fn omits_empty_stage_outputs() {
        let json = String::from_utf8(sample().to_json().unwrap()).unwrap();
        assert!(!json.contains("transcript"));
        assert!(!json.contains("\"double\""));
    }
}
