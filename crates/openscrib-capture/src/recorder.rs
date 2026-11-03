//! The live recording loop: global input hooks on one thread, step logic on
//! the caller's thread, PNG encoding on a writer thread so a click never
//! waits on compression.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Result, bail};
use crossbeam_channel::{RecvTimeoutError, Sender, unbounded};
use image::RgbaImage;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use openscrib_core::{Clock, Manifest, MonitorInfo, MouseButton, Point, ProjectDir, WindowInfo};
use tracing::{error, info};

use crate::session::{Capture, FrameSink, Input, Session, SessionConfig, TimedInput};
use crate::{platform, screen};

/// Upper bound on how long the loop sleeps, so the stop flag is noticed.
const IDLE_POLL: Duration = Duration::from_millis(200);

pub struct LiveCapture;

impl Capture for LiveCapture {
    fn grab(&mut self, at: Point) -> Result<(MonitorInfo, RgbaImage)> {
        screen::capture_at(at)
    }
    fn window_at(&mut self, p: Point) -> Option<WindowInfo> {
        platform::window_at(p)
    }
    fn foreground(&mut self) -> Option<WindowInfo> {
        platform::foreground_window()
    }
}

/// Encodes PNGs off the input thread.
pub struct PngWriter {
    tx: Option<Sender<(PathBuf, RgbaImage)>>,
    handle: Option<JoinHandle<()>>,
    root: ProjectDir,
}

impl PngWriter {
    pub fn spawn(root: ProjectDir) -> Self {
        let (tx, rx) = unbounded::<(PathBuf, RgbaImage)>();
        let handle = thread::Builder::new()
            .name("png-writer".into())
            .spawn(move || {
                for (path, img) in rx {
                    if let Err(e) = write_png(&path, &img) {
                        error!("writing {}: {e:#}", path.display());
                    }
                }
            })
            .expect("spawn png writer");
        Self { tx: Some(tx), handle: Some(handle), root }
    }

    /// Wait for every queued frame to hit disk.
    pub fn finish(mut self) {
        self.tx.take();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl FrameSink for &PngWriter {
    fn save(&mut self, rel: String, img: RgbaImage) {
        if let Some(tx) = &self.tx {
            let _ = tx.send((self.root.path(&rel), img));
        }
    }
}

fn write_png(path: &std::path::Path, img: &RgbaImage) -> Result<()> {
    // Fast compression while recording; export can recompress if it matters.
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let enc = PngEncoder::new_with_quality(file, CompressionType::Fast, FilterType::Adaptive);
    img.write_with_encoder(enc)?;
    Ok(())
}

/// Record until Ctrl+Shift+F12 is pressed or `stop` is set.
pub fn record(dir: ProjectDir, cfg: SessionConfig, stop: Arc<AtomicBool>) -> Result<Manifest> {
    platform::init_dpi_awareness();
    let monitors = screen::monitors()?;
    for m in &monitors {
        info!("monitor {} \"{}\" at {:?} scale {}", m.id, m.name, m.rect, m.scale);
    }

    let clock = Clock::start();
    let (tx, rx) = unbounded::<TimedInput>();
    thread::Builder::new().name("input-hook".into()).spawn(move || {
        let res = rdev::listen(move |e| {
            let t_ms = clock.now_ms();
            if let Some(input) = convert(e) {
                let _ = tx.send(TimedInput { t_ms, input });
            }
        });
        if let Err(e) = res {
            error!("input hook failed: {e:?}");
        }
    })?;

    let writer = PngWriter::spawn(dir.clone());
    let cursor = platform::cursor_pos().unwrap_or(Point::new(0, 0));
    let mut session = Session::new(dir, cfg, LiveCapture, &writer, monitors, cursor)?;
    info!("recording; press Ctrl+Shift+F12 to stop");

    while !session.stopped() && !stop.load(Ordering::Relaxed) {
        let wait = session
            .next_deadline()
            .map(|d| Duration::from_millis(d.saturating_sub(clock.now_ms())))
            .unwrap_or(IDLE_POLL)
            .min(IDLE_POLL);
        match rx.recv_timeout(wait) {
            Ok(ev) => {
                session.handle(ev)?;
                // Drain anything queued behind it before running timers.
                while let Ok(ev) = rx.try_recv() {
                    session.handle(ev)?;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => bail!("input hook stopped unexpectedly"),
        }
        session.tick(clock.now_ms())?;
    }

    let manifest = session.finish(clock.now_ms())?;
    writer.finish();
    // The hook thread blocks inside the OS message loop for the life of the
    // process; rdev offers no way to unhook, so it is left to exit with us.
    Ok(manifest)
}

fn convert(e: rdev::Event) -> Option<Input> {
    use rdev::EventType as E;
    Some(match e.event_type {
        E::MouseMove { x, y } => Input::Move(Point::new(x.round() as i32, y.round() as i32)),
        E::ButtonPress(b) => Input::Press(button(b)),
        E::ButtonRelease(b) => Input::Release(button(b)),
        E::KeyPress(key) => Input::KeyDown { key, text: e.name },
        E::KeyRelease(key) => Input::KeyUp(key),
        E::Wheel { delta_x, delta_y } => Input::Wheel { dx: delta_x, dy: delta_y },
    })
}

fn button(b: rdev::Button) -> MouseButton {
    match b {
        rdev::Button::Left => MouseButton::Left,
        rdev::Button::Right => MouseButton::Right,
        rdev::Button::Middle => MouseButton::Middle,
        rdev::Button::Unknown(_) => MouseButton::Other,
    }
}
