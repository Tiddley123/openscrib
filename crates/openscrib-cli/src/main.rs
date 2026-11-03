use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use openscrib_capture::audio::AudioConfig;
use openscrib_capture::video::{self, VideoConfig};
use openscrib_capture::{MediaConfig, SessionConfig};
use openscrib_core::markers::draw_click;
use openscrib_core::{Manifest, PROJECT_EXT, ProjectDir, StepKind};
use tracing::info;

#[derive(Parser)]
#[command(name = "openscrib", version, about = "Record once, export twice.")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Record clicks, keystrokes, screen video and narration until
    /// Ctrl+Shift+F12 (or Ctrl+C).
    Record(RecordArgs),
    /// Print a project's steps.
    Info { project: PathBuf },
    /// Render click markers onto every frame, plus a guide.md preview.
    Markers {
        project: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Pack a working directory (e.g. one left by a crash) into a .scrib file.
    Pack { dir: PathBuf, out: PathBuf },
    /// List monitors and microphones.
    Devices,
    /// Read a project's screen.mkv back, decode every video frame, and
    /// report per-track timing and size.
    Probe {
        project: PathBuf,
        /// Save the last decoded video frame here as a PNG.
        #[arg(long)]
        frame: Option<PathBuf>,
    },
}

#[derive(Args)]
struct RecordArgs {
    /// Output .scrib file. Defaults to projects/ beside the executable.
    #[arg(short, long)]
    out: Option<PathBuf>,
    /// Record typed characters as dots instead of their values.
    #[arg(long)]
    no_keys: bool,
    /// Keep the unpacked working directory after packing.
    #[arg(long)]
    keep_dir: bool,
    /// Stop automatically after this many seconds.
    #[arg(long)]
    duration: Option<u64>,
    /// Skip the continuous screen video.
    #[arg(long)]
    no_video: bool,
    /// Skip narration.
    #[arg(long)]
    no_audio: bool,
    /// Monitor id to record (see `devices`). Defaults to the primary monitor.
    #[arg(long)]
    monitor: Option<u32>,
    /// Maximum video frame rate. Unchanged frames are skipped regardless.
    #[arg(long, default_value_t = 12)]
    fps: u32,
    /// Microphone to use: part of its name (see `devices`).
    #[arg(long)]
    mic: Option<String>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();

    match Cli::parse().cmd {
        Cmd::Record(args) => record(args),
        Cmd::Devices => devices(),
        Cmd::Probe { project, frame } => probe(&load(&project)?, frame.as_deref()),
        Cmd::Info { project } => print_info(&load(&project)?.read_manifest()?),
        Cmd::Markers { project, out } => markers(&project, &out),
        Cmd::Pack { dir, out } => {
            ProjectDir::open(dir)?.pack(&out)?;
            info!("wrote {}", out.display());
            Ok(())
        }
    }
}

fn record(args: RecordArgs) -> Result<()> {
    let RecordArgs { out, no_keys, keep_dir, duration, no_video, no_audio, monitor, fps, mic } = args;
    if fps == 0 || fps > 60 {
        bail!("--fps must be between 1 and 60");
    }
    let mut media = MediaConfig::default();
    if !no_video {
        match video::locate_openh264() {
            Some(openh264) => media.video = Some(VideoConfig { monitor, fps, openh264 }),
            None => tracing::warn!(
                "{} not found beside the executable or in runtime/; recording without video. \
                 Download it from http://ciscobinary.openh264.org/{}.bz2",
                video::OPENH264_DLL,
                video::OPENH264_DLL
            ),
        }
    }
    if !no_audio {
        media.audio = Some(AudioConfig { device: mic });
    }

    let out = match out {
        Some(p) => p,
        None => default_project_path()?,
    };
    if out.exists() {
        bail!("{} already exists", out.display());
    }
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    let work = out.with_extension(format!("{PROJECT_EXT}.recording"));
    let dir = ProjectDir::create(&work)?;

    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed))?;
    if let Some(secs) = duration {
        let flag = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            flag.store(true, Ordering::Relaxed);
        });
    }

    let cfg = SessionConfig { capture_keys: !no_keys };
    let manifest = openscrib_capture::record(dir.clone(), cfg, media, stop)?;
    dir.pack(&out)?;
    if !keep_dir {
        fs::remove_dir_all(&work)?;
    }

    info!(
        "{} steps in {:.1}s -> {}",
        manifest.steps.len(),
        manifest.duration_ms as f64 / 1000.0,
        out.display()
    );
    for w in &manifest.warnings {
        tracing::warn!("{w}");
    }
    Ok(())
}

/// `projects/recording-YYYYMMDD-HHMMSS.scrib` beside the executable, so the
/// app folder stays self-contained.
fn default_project_path() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let base = exe.parent().context("executable has no parent dir")?;
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let stamp = now.format(time::macros::format_description!(
        "[year][month][day]-[hour][minute][second]"
    ))?;
    Ok(base.join("projects").join(format!("recording-{stamp}.{PROJECT_EXT}")))
}

/// A relative path that doesn't exist here is also tried inside the default
/// `projects/` folder beside the executable, so `info recording-….scrib`
/// works with the name `record` printed.
fn resolve_project(path: &Path) -> PathBuf {
    if path.exists() || path.is_absolute() {
        return path.to_path_buf();
    }
    let projects = default_project_path()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    if let Some(dir) = projects {
        let file_name = path.file_name().map(PathBuf::from).unwrap_or_default();
        for candidate in [dir.join(path), dir.join(file_name)] {
            if candidate.exists() {
                return candidate;
            }
        }
    }
    path.to_path_buf()
}

/// Open a project from a directory, or unpack a .scrib into a temp dir.
fn load(path: &Path) -> Result<ProjectDir> {
    let resolved = resolve_project(path);
    let path = resolved.as_path();
    if path.is_dir() {
        return Ok(ProjectDir::open(path)?);
    }
    let tmp = std::env::temp_dir().join(format!("openscrib-{}", std::process::id()));
    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }
    Ok(ProjectDir::unpack(path, tmp)?)
}

fn probe(dir: &ProjectDir, frame_out: Option<&Path>) -> Result<()> {
    use matroska_demuxer::{Frame, MatroskaFile};
    use openh264::decoder::{Decoder, DecoderConfig};
    use openh264::formats::YUVSource;

    let mut decoder = match video::locate_openh264() {
        Some(dll) => Some(Decoder::with_api_config(openh264::OpenH264API::from_blob_path(dll)?, DecoderConfig::new())?),
        None => None,
    };
    let mut decoded = 0u64;
    let mut decode_errors = 0u64;
    let mut last_rgba: Option<(Vec<u8>, usize, usize)> = None;

    let path = dir.path(openscrib_core::project::SCREEN_FILE);
    let size = fs::metadata(&path).with_context(|| format!("no {}", path.display()))?.len();
    let mut mkv = MatroskaFile::open(fs::File::open(&path)?)?;
    let duration = mkv.info().duration().unwrap_or(0.0);
    println!("{} · {:.2} MB · duration {:.2}s", path.display(), size as f64 / 1e6, duration / 1000.0);
    for t in mkv.tracks() {
        println!("  track {} {:?} {}", t.track_number(), t.track_type(), t.codec_id());
    }

    #[derive(Default)]
    struct Stats {
        n: u64,
        keys: u64,
        bytes: u64,
        first: Option<u64>,
        last: u64,
        max_gap: u64,
    }
    let mut stats: [Stats; 3] = Default::default();
    let mut frame = Frame::default();
    while mkv.next_frame(&mut frame)? {
        let s = &mut stats[(frame.track as usize).min(2)];
        if let Some(_first) = s.first {
            s.max_gap = s.max_gap.max(frame.timestamp - s.last);
        } else {
            s.first = Some(frame.timestamp);
        }
        s.n += 1;
        s.keys += u64::from(frame.is_keyframe == Some(true));
        s.bytes += frame.data.len() as u64;
        s.last = frame.timestamp;

        if frame.track == 1
            && let Some(dec) = decoder.as_mut()
        {
            match dec.decode(&length_prefixed_to_annexb(&frame.data)) {
                Ok(Some(yuv)) => {
                    decoded += 1;
                    if frame_out.is_some() {
                        let (w, h) = yuv.dimensions();
                        let mut buf = vec![0u8; w * h * 4];
                        yuv.write_rgba8(&mut buf);
                        last_rgba = Some((buf, w, h));
                    }
                }
                Ok(None) => {}
                Err(_) => decode_errors += 1,
            }
        }
    }
    if decoder.is_some() {
        println!("  decoded {decoded} video frames, {decode_errors} errors");
    }
    if let (Some(out), Some((buf, w, h))) = (frame_out, last_rgba) {
        image::RgbaImage::from_raw(w as u32, h as u32, buf).context("frame buffer")?.save(out)?;
        println!("  last frame -> {}", out.display());
    }
    for (i, name) in [(1, "video"), (2, "audio")] {
        let s = &stats[i];
        let Some(first) = s.first else { continue };
        let span = s.last - first;
        println!(
            "  {name}: {} blocks ({} key), {:.2} MB, {:.2}s..{:.2}s, max gap {} ms",
            s.n,
            s.keys,
            s.bytes as f64 / 1e6,
            first as f64 / 1000.0,
            s.last as f64 / 1000.0,
            s.max_gap
        );
        if i == 2 {
            // 20 ms packets: any shortfall means dropped or late audio.
            let expected = span / 20 + 1;
            println!("         {} packets expected for that span ({:+})", expected, s.n as i64 - expected as i64);
        }
    }
    Ok(())
}

fn devices() -> Result<()> {
    openscrib_capture::platform::init_dpi_awareness();
    println!("Monitors:");
    for m in openscrib_capture::screen::monitors()? {
        let primary = if m.primary { " (primary)" } else { "" };
        println!("  {:>10}  {:<24} {}x{} at {},{}{primary}", m.id, m.name, m.rect.w, m.rect.h, m.rect.x, m.rect.y);
    }
    println!("Microphones:");
    for (name, default) in openscrib_capture::audio::input_devices()? {
        println!("  {name}{}", if default { " (default)" } else { "" });
    }
    match video::locate_openh264() {
        Some(p) => println!("OpenH264: {}", p.display()),
        None => println!("OpenH264: {} not found (video disabled)", video::OPENH264_DLL),
    }
    Ok(())
}

fn print_info(m: &Manifest) -> Result<()> {
    println!("version {} · {:.1}s · {} steps", m.version, m.duration_ms as f64 / 1000.0, m.steps.len());
    for mon in &m.monitors {
        println!("  monitor {} \"{}\" {:?} @{}x", mon.id, mon.name, mon.rect, mon.scale);
    }
    if let Some(v) = &m.video {
        println!(
            "  video {} monitor {} {}x{} ≤{} fps, {} frames from {:.2}s",
            v.file, v.monitor, v.width, v.height, v.max_fps, v.frames, v.first_ms as f64 / 1000.0
        );
    }
    if let Some(a) = &m.audio {
        println!("  audio {} \"{}\" {} Hz from {:.2}s", a.file, a.device, a.device_rate, a.first_ms as f64 / 1000.0);
    }
    for s in &m.steps {
        let at = s
            .click
            .as_ref()
            .map(|c| format!(" ({},{}) frame({},{})", c.x, c.y, c.in_frame.x, c.in_frame.y))
            .unwrap_or_default();
        let pre = if s.pre_frame.is_some() { " +pre" } else { "" };
        println!("{:>4} {:>8.2}s {:<8}{at}{pre}  {}", s.id, s.t_ms as f64 / 1000.0, format!("{:?}", s.kind), s.text);
    }
    for w in &m.warnings {
        println!("warning: {w}");
    }
    Ok(())
}

fn markers(project: &Path, out: &Path) -> Result<()> {
    let dir = load(project)?;
    let m = dir.read_manifest()?;
    fs::create_dir_all(out)?;

    let mut md = String::from("# Recorded guide\n\n");
    for (n, s) in m.steps.iter().enumerate() {
        md.push_str(&format!("{}. {}\n", n + 1, s.text));
        for (rel, suffix) in [(&s.frame, ""), (&s.pre_frame, "_pre")] {
            let Some(rel) = rel else { continue };
            let mut img = image::open(dir.path(rel))
                .with_context(|| format!("reading {rel}"))?
                .to_rgba8();
            if let (StepKind::Click, Some(c)) = (s.kind, &s.click) {
                let scale = s.monitor.and_then(|id| m.monitor(id)).map_or(1.0, |mon| mon.scale);
                draw_click(&mut img, c, scale);
            }
            let name = format!("{:04}{suffix}.png", s.id);
            img.save(out.join(&name))?;
            if suffix.is_empty() {
                md.push_str(&format!("\n   ![Step {}]({name})\n\n", n + 1));
            }
        }
    }
    fs::write(out.join("guide.md"), md)?;
    info!("rendered {} steps to {}", m.steps.len(), out.display());
    Ok(())
}

fn length_prefixed_to_annexb(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut rest = data;
    while rest.len() >= 4 {
        let n = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        let Some(nal) = rest.get(4..4 + n) else { break };
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
        rest = &rest[4 + n..];
    }
    out
}
