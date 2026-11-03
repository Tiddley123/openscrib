//! The live recording loop: global input hooks on one thread, step logic on
//! the caller's thread, PNG encoding on a writer thread so a click never
//! waits on compression.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Result, bail};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use image::RgbaImage;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use openscrib_core::project::SCREEN_FILE;
use openscrib_core::{
    AudioTrack, Clock, Manifest, MonitorInfo, MouseButton, Point, ProjectDir, VideoTrack, WindowInfo,
};
use tracing::{error, info, warn};

use crate::audio::{self, AudioConfig, Narration};
use crate::media::Muxer;
use crate::session::{Capture, FrameSink, Input, Session, SessionConfig, TimedInput};
use crate::video::{ScreenVideo, VideoConfig};
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

/// Which continuous streams to record alongside the steps. Each is optional;
/// one that fails to start is skipped with a manifest warning.
#[derive(Debug, Clone, Default)]
pub struct MediaConfig {
    pub video: Option<VideoConfig>,
    pub audio: Option<AudioConfig>,
}

/// Record until the stop hotkey is pressed or `stop` is set.
pub fn record(
    dir: ProjectDir,
    cfg: SessionConfig,
    media: MediaConfig,
    stop: Arc<AtomicBool>,
) -> Result<Manifest> {
    platform::init_dpi_awareness();
    let monitors = screen::monitors()?;
    for m in &monitors {
        info!("monitor {} \"{}\" at {:?} scale {}", m.id, m.name, m.rect, m.scale);
    }

    let clock = Clock::start();
    let mut warnings = Vec::new();
    let muxer = match (&media.video, &media.audio) {
        (None, None) => None,
        (v, a) => Some(Muxer::create(&dir.path(SCREEN_FILE), v.is_some(), a.as_ref().map(|_| audio::spec()))?),
    };
    let video = match (&media.video, &muxer) {
        (Some(vc), Some(mux)) => match ScreenVideo::start(vc, clock, mux.clone()) {
            Ok(v) => Some(v),
            Err(e) => {
                warnings.push(format!("Screen video disabled: {e:#}"));
                mux.start_without_video()?;
                None
            }
        },
        _ => None,
    };
    let narration = match (&media.audio, &muxer) {
        (Some(ac), Some(mux)) => match Narration::start(ac, clock, mux.clone()) {
            Ok(n) => Some(n),
            Err(e) => {
                warnings.push(format!("Narration disabled: {e:#}"));
                None
            }
        },
        _ => None,
    };
    for w in &warnings {
        warn!("{w}");
    }

    let rx = subscribe_input(clock)?;

    let writer = PngWriter::spawn(dir.clone());
    let cursor = platform::cursor_pos().unwrap_or(Point::new(0, 0));
    let stop_label = cfg.stop_key.to_string();
    let mut session = Session::new(dir.clone(), cfg, LiveCapture, &writer, monitors, cursor)?;
    info!("recording; press {stop_label} to stop");

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

    unsubscribe_input();
    let end_ms = clock.now_ms();
    // Cut both streams at the moment the user stopped, then let them wind
    // down in parallel while the step data is flushed.
    if let Some(mux) = &muxer {
        mux.close_at(end_ms);
    }
    if let Some(v) = &video {
        v.request_stop();
    }
    if let Some(n) = &narration {
        n.request_stop();
    }
    let mut manifest = session.finish(end_ms)?;
    writer.finish();

    // Stop media after the session so the video covers the final step.
    if let Some(v) = video {
        match v.stop() {
            Ok(s) => {
                info!("video: {} frames encoded", s.frames);
                manifest.video = s.first_ms.map(|first_ms| VideoTrack {
                    file: SCREEN_FILE.into(),
                    monitor: s.monitor.id,
                    width: s.width,
                    height: s.height,
                    max_fps: media.video.as_ref().map_or(0, |c| c.fps),
                    frames: s.frames,
                    first_ms,
                });
            }
            Err(e) => warnings.push(format!("Screen video stopped early: {e:#}")),
        }
    }
    if let Some(n) = narration {
        match n.stop() {
            Ok(s) => {
                info!("audio: {} packets ({:.1}s)", s.packets, s.packets as f64 * 0.02);
                manifest.audio = s.first_ms.map(|first_ms| AudioTrack {
                    file: SCREEN_FILE.into(),
                    device: s.device,
                    device_rate: s.device_rate,
                    channels: 1,
                    first_ms,
                });
            }
            Err(e) => warnings.push(format!("Narration stopped early: {e:#}")),
        }
    }
    if let Some(mux) = muxer {
        mux.finish(end_ms)?;
    }
    manifest.warnings.extend(warnings);
    dir.write_manifest(&manifest)?;
    Ok(manifest)
}

/// The current recording's input sink. The OS hook is installed once per
/// process (rdev cannot remove it), and each recording swaps in its own
/// channel and clock here.
static INPUT_SINK: Mutex<Option<(Sender<TimedInput>, Clock)>> = Mutex::new(None);
static HOOK: OnceLock<std::result::Result<(), String>> = OnceLock::new();

fn subscribe_input(clock: Clock) -> Result<Receiver<TimedInput>> {
    let (tx, rx) = unbounded::<TimedInput>();
    *INPUT_SINK.lock().expect("input sink lock") = Some((tx, clock));
    let started = HOOK.get_or_init(|| {
        thread::Builder::new()
            .name("input-hook".into())
            .spawn(|| {
                let res = rdev::listen(|e| {
                    let guard = INPUT_SINK.lock().expect("input sink lock");
                    if let Some((tx, clock)) = guard.as_ref() {
                        let t_ms = clock.now_ms();
                        if let Some(input) = convert(e) {
                            let _ = tx.send(TimedInput { t_ms, input });
                        }
                    }
                });
                if let Err(e) = res {
                    error!("input hook failed: {e:?}");
                }
                // Wake any recording waiting on input so it can report this.
                INPUT_SINK.lock().expect("input sink lock").take();
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    started.clone().map_err(|e| anyhow::anyhow!("starting input hook: {e}"))?;
    Ok(rx)
}

fn unsubscribe_input() {
    INPUT_SINK.lock().expect("input sink lock").take();
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
