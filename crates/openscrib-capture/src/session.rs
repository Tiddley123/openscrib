//! Turns a timed stream of input events into steps. Pure logic: screen and
//! window access go through [`Capture`] and PNG writes through [`FrameSink`],
//! so the rules here are testable without hooks or a display.

use std::fs::File;
use std::io::{BufWriter, Write};

use anyhow::Result;
use image::RgbaImage;
use openscrib_core::{
    Click, Manifest, MonitorInfo, MouseButton, Point, ProjectDir, Step, StepKind, WindowInfo,
};
use rdev::Key;
use serde_json::json;
use tracing::warn;

use crate::screen::looks_blank;

/// Mouse must rest this long before a click for a tooltip frame to be taken.
pub const PAUSE_MS: u64 = 500;
/// Second press within this window (and radius) turns a click into a double-click.
pub const DOUBLE_CLICK_MS: u64 = 500;
pub const DOUBLE_CLICK_RADIUS: i32 = 4;
/// A typing burst ends after this much silence.
pub const BURST_IDLE_MS: u64 = 3000;
/// Ignore pre-click frames older than this.
pub const PRE_FRAME_MAX_AGE_MS: u64 = 10_000;
const MOVE_LOG_INTERVAL_MS: u64 = 16;

#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Move(Point),
    Press(MouseButton),
    Release(MouseButton),
    /// `text` is what the key produced under the active layout, if anything.
    KeyDown { key: Key, text: Option<String> },
    KeyUp(Key),
    Wheel { dx: i64, dy: i64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimedInput {
    pub t_ms: u64,
    pub input: Input,
}

pub trait Capture {
    fn grab(&mut self, at: Point) -> Result<(MonitorInfo, RgbaImage)>;
    fn window_at(&mut self, p: Point) -> Option<WindowInfo>;
    fn foreground(&mut self) -> Option<WindowInfo>;
}

pub trait FrameSink {
    /// `rel` is the manifest-relative path, e.g. `frames/0003.png`.
    fn save(&mut self, rel: String, img: RgbaImage);
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// When false, typed characters are recorded as `•` and not logged.
    pub capture_keys: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self { capture_keys: true }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct Mods {
    ctrl: bool,
    alt: bool,
    shift: bool,
    meta: bool,
}

struct Paused {
    t_ms: u64,
    at: Point,
    monitor: u32,
    image: RgbaImage,
}

struct Burst {
    start_ms: u64,
    last_ms: u64,
    text: String,
    window: Option<WindowInfo>,
}

pub struct Session<C: Capture, S: FrameSink> {
    cap: C,
    sink: S,
    dir: ProjectDir,
    events: BufWriter<File>,
    cfg: SessionConfig,
    manifest: Manifest,

    cursor: Point,
    quiet_since_ms: u64,
    last_move_logged_ms: Option<u64>,
    paused: Option<Paused>,
    pause_tried: bool,
    mods: Mods,
    burst: Option<Burst>,
    next_id: u32,
    blank_warned: bool,
    stopped: bool,
}

impl<C: Capture, S: FrameSink> Session<C, S> {
    pub fn new(
        dir: ProjectDir,
        cfg: SessionConfig,
        cap: C,
        sink: S,
        monitors: Vec<MonitorInfo>,
        cursor: Point,
    ) -> Result<Self> {
        let mut manifest = Manifest::new(env!("CARGO_PKG_VERSION"));
        manifest.monitors = monitors;
        dir.write_manifest(&manifest)?;
        Ok(Self {
            events: dir.events_writer()?,
            cap,
            sink,
            dir,
            cfg,
            manifest,
            cursor,
            quiet_since_ms: 0,
            last_move_logged_ms: None,
            paused: None,
            pause_tried: false,
            mods: Mods::default(),
            burst: None,
            next_id: 1,
            blank_warned: false,
            stopped: false,
        })
    }

    pub fn stopped(&self) -> bool {
        self.stopped
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The next time `tick` has work to do, if any.
    pub fn next_deadline(&self) -> Option<u64> {
        let pause = (!self.pause_tried).then_some(self.quiet_since_ms + PAUSE_MS);
        let burst = self.burst.as_ref().map(|b| b.last_ms + BURST_IDLE_MS);
        pause.into_iter().chain(burst).min()
    }

    pub fn handle(&mut self, ev: TimedInput) -> Result<()> {
        let t = ev.t_ms;
        self.log(&ev)?;
        match ev.input {
            Input::Move(p) => {
                self.cursor = p;
                self.quiet_since_ms = t;
                self.paused = None;
                self.pause_tried = false;
            }
            Input::Press(button) => self.on_press(t, button)?,
            Input::Release(_) | Input::Wheel { .. } => {}
            Input::KeyDown { key, text } => self.on_key_down(t, key, text)?,
            Input::KeyUp(key) => {
                self.set_mod(key, false);
            }
        }
        Ok(())
    }

    pub fn tick(&mut self, now_ms: u64) -> Result<()> {
        if !self.pause_tried && now_ms >= self.quiet_since_ms + PAUSE_MS {
            self.pause_tried = true;
            match self.cap.grab(self.cursor) {
                Ok((mon, image)) => {
                    self.paused = Some(Paused { t_ms: now_ms, at: self.cursor, monitor: mon.id, image })
                }
                Err(e) => warn!("pre-click capture failed: {e:#}"),
            }
        }
        if self.burst.as_ref().is_some_and(|b| now_ms >= b.last_ms + BURST_IDLE_MS) {
            self.flush_burst()?;
        }
        Ok(())
    }

    pub fn finish(mut self, now_ms: u64) -> Result<Manifest> {
        self.flush_burst()?;
        self.manifest.duration_ms = now_ms;
        self.dir.write_manifest(&self.manifest)?;
        self.events.flush()?;
        Ok(self.manifest)
    }

    fn on_press(&mut self, t: u64, button: MouseButton) -> Result<()> {
        self.flush_burst()?;
        let at = self.cursor;

        if let Some(prev) = self.manifest.steps.last_mut()
            && let Some(c) = prev.click.as_mut()
            && button == MouseButton::Left
            && c.button == MouseButton::Left
            && !c.double
            && t.saturating_sub(prev.t_ms) <= DOUBLE_CLICK_MS
            && Point::new(c.x, c.y).dist2(at) <= (DOUBLE_CLICK_RADIUS * DOUBLE_CLICK_RADIUS) as i64
        {
            c.double = true;
            prev.refresh_text();
            return self.save_manifest();
        }

        let id = self.take_id();
        let mut step = Step::new(id, t, StepKind::Click);
        step.window = self.cap.window_at(at);
        let origin = self.attach_frame(&mut step, at);
        step.click = Some(Click {
            x: at.x,
            y: at.y,
            button,
            double: false,
            in_window: step.window.as_ref().map(|w| at.offset_from(w.rect.origin())),
            in_frame: at.offset_from(origin.unwrap_or(Point::new(0, 0))),
        });

        if let Some(p) = self.paused.take()
            && p.at == at
            && Some(p.monitor) == step.monitor
            && t.saturating_sub(p.t_ms) <= PRE_FRAME_MAX_AGE_MS
        {
            let rel = ProjectDir::frame_name(id, "_pre");
            self.sink.save(rel.clone(), p.image);
            step.pre_frame = Some(rel);
        }
        // Next pause frame should show the result of this click, not its press.
        self.quiet_since_ms = t;
        self.pause_tried = false;

        self.push(step)
    }

    fn on_key_down(&mut self, t: u64, key: Key, text: Option<String>) -> Result<()> {
        if self.set_mod(key, true) {
            return Ok(());
        }
        let m = self.mods;
        if key == Key::F12 && m.ctrl && m.shift {
            self.stopped = true;
            return Ok(());
        }

        let printable = text.filter(|s| !s.is_empty() && !s.chars().any(char::is_control));
        // AltGr arrives as Ctrl+Alt but produces text; treat it as typing.
        let altgr_text = m.ctrl && m.alt && printable.is_some();
        if (m.ctrl || m.alt || m.meta) && !altgr_text {
            return self.shortcut(t, key);
        }

        match key {
            Key::Return | Key::KpReturn => {
                self.type_token(t, "{Enter}");
                self.flush_burst()
            }
            Key::Escape if self.burst.is_some() => {
                self.type_token(t, "{Esc}");
                self.flush_burst()
            }
            Key::Tab => {
                self.type_token(t, "{Tab}");
                Ok(())
            }
            Key::Backspace => {
                if let Some(b) = self.burst.as_mut() {
                    b.last_ms = t;
                    if b.text.ends_with('}') || b.text.is_empty() {
                        b.text.push_str("{Backspace}");
                    } else {
                        b.text.pop();
                    }
                }
                Ok(())
            }
            _ => {
                if let Some(s) = printable {
                    let s = if self.cfg.capture_keys { s } else { "•".repeat(s.chars().count()) };
                    self.type_token(t, &s);
                }
                Ok(())
            }
        }
    }

    fn type_token(&mut self, t: u64, s: &str) {
        if self.burst.is_none() {
            let window = self.cap.foreground();
            self.burst = Some(Burst { start_ms: t, last_ms: t, text: String::new(), window });
        }
        let b = self.burst.as_mut().expect("burst just ensured");
        b.last_ms = t;
        b.text.push_str(s);
    }

    fn shortcut(&mut self, t: u64, key: Key) -> Result<()> {
        self.flush_burst()?;
        let m = self.mods;
        let mut combo = Vec::new();
        for (on, name) in [(m.ctrl, "Ctrl"), (m.alt, "Alt"), (m.shift, "Shift"), (m.meta, "Win")] {
            if on {
                combo.push(name.to_string());
            }
        }
        combo.push(key_label(key));

        let id = self.take_id();
        let mut step = Step::new(id, t, StepKind::Shortcut);
        step.keys = Some(combo.join("+"));
        step.window = self.cap.foreground();
        let at = step.window.as_ref().map(centre).unwrap_or(self.cursor);
        self.attach_frame(&mut step, at);
        self.push(step)
    }

    fn flush_burst(&mut self) -> Result<()> {
        let Some(b) = self.burst.take() else { return Ok(()) };
        let id = self.take_id();
        let mut step = Step::new(id, b.start_ms, StepKind::Typed);
        step.keys = Some(b.text);
        let at = b.window.as_ref().map(centre).unwrap_or(self.cursor);
        step.window = b.window;
        // Captured at the end of the burst, so the frame shows what was typed.
        self.attach_frame(&mut step, at);
        self.push(step)
    }

    /// Grab the monitor at `at`, queue it as this step's frame, and return the
    /// frame's screen origin.
    fn attach_frame(&mut self, step: &mut Step, at: Point) -> Option<Point> {
        match self.cap.grab(at) {
            Ok((mon, img)) => {
                if !self.blank_warned && looks_blank(&img) {
                    self.blank_warned = true;
                    let msg = format!(
                        "Step {} captured a blank frame on monitor \"{}\". Screen capture may be \
                         blocked by the remote session's policy.",
                        step.id, mon.name
                    );
                    warn!("{msg}");
                    self.manifest.warnings.push(msg);
                }
                let rel = ProjectDir::frame_name(step.id, "");
                self.sink.save(rel.clone(), img);
                step.frame = Some(rel);
                step.monitor = Some(mon.id);
                let origin = mon.rect.origin();
                if self.manifest.monitor(mon.id).is_none() {
                    self.manifest.monitors.push(mon);
                }
                Some(origin)
            }
            Err(e) => {
                let msg = format!("Step {}: screen capture failed: {e:#}", step.id);
                warn!("{msg}");
                self.manifest.warnings.push(msg);
                None
            }
        }
    }

    fn push(&mut self, mut step: Step) -> Result<()> {
        step.refresh_text();
        self.manifest.steps.push(step);
        self.save_manifest()
    }

    fn save_manifest(&mut self) -> Result<()> {
        self.events.flush()?;
        self.dir.write_manifest(&self.manifest)?;
        Ok(())
    }

    fn take_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Returns true if `key` is a modifier.
    fn set_mod(&mut self, key: Key, down: bool) -> bool {
        let slot = match key {
            Key::ControlLeft | Key::ControlRight => &mut self.mods.ctrl,
            Key::Alt | Key::AltGr => &mut self.mods.alt,
            Key::ShiftLeft | Key::ShiftRight => &mut self.mods.shift,
            Key::MetaLeft | Key::MetaRight => &mut self.mods.meta,
            _ => return false,
        };
        *slot = down;
        true
    }

    fn log(&mut self, ev: &TimedInput) -> Result<()> {
        let t = ev.t_ms;
        let line = match &ev.input {
            Input::Move(p) => {
                if self.last_move_logged_ms.is_some_and(|last| t < last + MOVE_LOG_INTERVAL_MS) {
                    return Ok(());
                }
                self.last_move_logged_ms = Some(t);
                json!({"t": t, "ev": "move", "x": p.x, "y": p.y})
            }
            Input::Press(b) => json!({"t": t, "ev": "press", "button": b}),
            Input::Release(b) => json!({"t": t, "ev": "release", "button": b}),
            Input::Wheel { dx, dy } => json!({"t": t, "ev": "wheel", "dx": dx, "dy": dy}),
            Input::KeyDown { key, .. } if self.cfg.capture_keys => {
                json!({"t": t, "ev": "key_down", "key": format!("{key:?}")})
            }
            Input::KeyUp(key) if self.cfg.capture_keys => {
                json!({"t": t, "ev": "key_up", "key": format!("{key:?}")})
            }
            Input::KeyDown { .. } => json!({"t": t, "ev": "key_down"}),
            Input::KeyUp(_) => json!({"t": t, "ev": "key_up"}),
        };
        serde_json::to_writer(&mut self.events, &line)?;
        self.events.write_all(b"\n")?;
        Ok(())
    }
}

fn centre(w: &WindowInfo) -> Point {
    Point::new(w.rect.x + (w.rect.w / 2) as i32, w.rect.y + (w.rect.h / 2) as i32)
}

pub fn key_label(key: Key) -> String {
    let name = format!("{key:?}");
    let short = match key {
        Key::Return | Key::KpReturn => "Enter",
        Key::Escape => "Esc",
        Key::Delete => "Del",
        Key::UpArrow => "Up",
        Key::DownArrow => "Down",
        Key::LeftArrow => "Left",
        Key::RightArrow => "Right",
        Key::Comma => ",",
        Key::Dot => ".",
        Key::Slash => "/",
        Key::Minus => "-",
        Key::Equal => "=",
        Key::SemiColon => ";",
        Key::Quote => "'",
        Key::BackQuote => "`",
        Key::LeftBracket => "[",
        Key::RightBracket => "]",
        Key::BackSlash | Key::IntlBackslash => "\\",
        _ => {
            return name
                .strip_prefix("Key")
                .or_else(|| name.strip_prefix("Num").filter(|r| r.len() == 1))
                .unwrap_or(&name)
                .to_string();
        }
    };
    short.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    use openscrib_core::Rect;

    const MON: Rect = Rect { x: -1920, y: 0, w: 1920, h: 1080 };

    struct FakeCapture;

    impl Capture for FakeCapture {
        fn grab(&mut self, _at: Point) -> Result<(MonitorInfo, RgbaImage)> {
            let mut img = RgbaImage::from_pixel(8, 8, Rgba([20, 20, 20, 255]));
            img.put_pixel(4, 4, Rgba([250, 250, 250, 255]));
            let mon = MonitorInfo { id: 7, name: "Left".into(), rect: MON, scale: 1.0, primary: false };
            Ok((mon, img))
        }
        fn window_at(&mut self, _p: Point) -> Option<WindowInfo> {
            Some(WindowInfo {
                rect: Rect { x: -1800, y: 100, w: 1200, h: 800 },
                client: None,
                title: "remote-pc - TeamViewer".into(),
                process: "TeamViewer.exe".into(),
            })
        }
        fn foreground(&mut self) -> Option<WindowInfo> {
            self.window_at(Point::new(0, 0))
        }
    }

    #[derive(Default)]
    struct Saved(Vec<String>);
    impl FrameSink for &mut Saved {
        fn save(&mut self, rel: String, _img: RgbaImage) {
            self.0.push(rel);
        }
    }

    fn ev(t_ms: u64, input: Input) -> TimedInput {
        TimedInput { t_ms, input }
    }

    fn key(t: u64, k: Key, s: &str) -> TimedInput {
        ev(t, Input::KeyDown { key: k, text: Some(s.into()).filter(|s: &String| !s.is_empty()) })
    }

    fn run(events: Vec<TimedInput>) -> (Manifest, Vec<String>) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = ProjectDir::create(tmp.path()).unwrap();
        let mut saved = Saved::default();
        let mut s = Session::new(
            dir,
            SessionConfig::default(),
            FakeCapture,
            &mut saved,
            vec![],
            Point::new(0, 0),
        )
        .unwrap();
        let mut last = 0;
        for e in events {
            // Fire any timers due before this event, like the live loop does.
            while let Some(d) = s.next_deadline().filter(|d| *d <= e.t_ms) {
                s.tick(d).unwrap();
            }
            last = e.t_ms;
            s.handle(e).unwrap();
            s.tick(last).unwrap();
        }
        let m = s.finish(last + 1).unwrap();
        (m, saved.0)
    }

    #[test]
    fn click_records_frame_and_window_relative_coords() {
        let (m, saved) = run(vec![
            ev(100, Input::Move(Point::new(-1000, 500))),
            ev(1000, Input::Press(MouseButton::Left)),
        ]);
        assert_eq!(m.steps.len(), 1);
        let s = &m.steps[0];
        let c = s.click.as_ref().unwrap();
        assert_eq!((c.x, c.y), (-1000, 500));
        assert_eq!(c.in_frame, Point::new(920, 500));
        assert_eq!(c.in_window, Some(Point::new(800, 400)));
        assert_eq!(s.monitor, Some(7));
        assert_eq!(s.pre_frame.as_deref(), Some("frames/0001_pre.png"));
        assert_eq!(s.text, "Click in **remote-pc - TeamViewer**.");
        assert_eq!(saved, ["frames/0001.png", "frames/0001_pre.png"]);
        assert_eq!(m.monitors.len(), 1);
    }

    #[test]
    fn no_pre_frame_when_mouse_was_moving() {
        let (m, _) = run(vec![
            ev(100, Input::Move(Point::new(10, 10))),
            ev(300, Input::Move(Point::new(12, 10))),
            ev(400, Input::Press(MouseButton::Left)),
        ]);
        assert_eq!(m.steps[0].pre_frame, None);
    }

    #[test]
    fn double_click_merges() {
        let (m, saved) = run(vec![
            ev(100, Input::Move(Point::new(10, 10))),
            ev(200, Input::Press(MouseButton::Left)),
            ev(250, Input::Release(MouseButton::Left)),
            ev(400, Input::Press(MouseButton::Left)),
        ]);
        assert_eq!(m.steps.len(), 1);
        assert!(m.steps[0].click.as_ref().unwrap().double);
        assert!(m.steps[0].text.starts_with("Double-click"));
        assert_eq!(saved.len(), 1);
    }

    #[test]
    fn typing_collapses_into_one_step() {
        let (m, _) = run(vec![
            ev(100, Input::Press(MouseButton::Left)),
            key(1000, Key::KeyH, "h"),
            key(1100, Key::KeyI, "i"),
            key(1200, Key::KeyX, "x"),
            key(1300, Key::Backspace, ""),
            key(1400, Key::ShiftLeft, ""),
            key(1450, Key::Num1, "!"),
            ev(1500, Input::KeyUp(Key::ShiftLeft)),
            key(1600, Key::Return, ""),
        ]);
        assert_eq!(m.steps.len(), 2);
        let s = &m.steps[1];
        assert_eq!(s.kind, StepKind::Typed);
        assert_eq!(s.t_ms, 1000);
        assert_eq!(s.keys.as_deref(), Some("hi!{Enter}"));
    }

    #[test]
    fn burst_ends_after_idle() {
        let (m, _) = run(vec![key(0, Key::KeyA, "a"), key(5000, Key::KeyB, "b")]);
        let keys: Vec<_> = m.steps.iter().map(|s| s.keys.clone().unwrap()).collect();
        assert_eq!(keys, ["a", "b"]);
    }

    #[test]
    fn shortcut_and_stop_hotkey() {
        let (m, _) = run(vec![
            key(0, Key::KeyA, "a"),
            key(100, Key::ControlLeft, ""),
            key(150, Key::KeyS, "\u{13}"),
            ev(200, Input::KeyUp(Key::ControlLeft)),
            key(300, Key::ControlLeft, ""),
            key(310, Key::ShiftLeft, ""),
            key(320, Key::F12, ""),
        ]);
        let kinds: Vec<_> = m.steps.iter().map(|s| (s.kind, s.keys.clone().unwrap())).collect();
        assert_eq!(kinds, [(StepKind::Typed, "a".into()), (StepKind::Shortcut, "Ctrl+S".into())]);
        assert_eq!(m.steps[1].text, "Press **Ctrl+S**.");
    }

    #[test]
    fn altgr_types_text() {
        let (m, _) = run(vec![
            key(0, Key::ControlLeft, ""),
            key(0, Key::AltGr, ""),
            key(10, Key::KeyE, "€"),
        ]);
        assert_eq!(m.steps[0].keys.as_deref(), Some("€"));
    }

    #[test]
    fn redacted_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = ProjectDir::create(tmp.path()).unwrap();
        let mut saved = Saved::default();
        let cfg = SessionConfig { capture_keys: false };
        let mut s = Session::new(dir.clone(), cfg, FakeCapture, &mut saved, vec![], Point::new(0, 0)).unwrap();
        s.handle(key(0, Key::KeyP, "p")).unwrap();
        s.handle(key(10, Key::KeyW, "w")).unwrap();
        let m = s.finish(20).unwrap();
        assert_eq!(m.steps[0].keys.as_deref(), Some("••"));
        let log = std::fs::read_to_string(dir.path("events.jsonl")).unwrap();
        assert!(!log.contains("KeyP"));
    }

    #[test]
    fn key_labels() {
        assert_eq!(key_label(Key::KeyS), "S");
        assert_eq!(key_label(Key::Num1), "1");
        assert_eq!(key_label(Key::F12), "F12");
        assert_eq!(key_label(Key::NumLock), "NumLock");
        assert_eq!(key_label(Key::Delete), "Del");
    }
}
