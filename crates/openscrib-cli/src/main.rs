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
    /// the stop key (default Ctrl+Shift+F12) or Ctrl+C.
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
    /// Export the guide (Markdown, HTML, PDF) and/or the video (MKV, MP4).
    Export {
        project: PathBuf,
        /// Formats to write.
        #[arg(short, long, value_delimiter = ',', default_value = "md,html,pdf")]
        format: Vec<GuideFormat>,
        /// Output folder. Defaults to `<project name>-export` beside the project.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Show the whole monitor instead of the clicked window.
        #[arg(long)]
        full_frame: bool,
        /// Also render each PDF page to page-NN.png (for previews).
        #[arg(long)]
        pdf_pages: bool,
        /// Video quality, 1-100.
        #[arg(long, default_value_t = openscrib_core::quality::DEFAULT_QUALITY, value_parser = clap::value_parser!(u8).range(1..=100))]
        quality: u8,
        /// Scale exported video down to at most this width (0 keeps the original).
        #[arg(long, default_value_t = 1920)]
        max_width: u32,
        /// Read the step text aloud instead of using the recorded narration
        /// (Piper voice id such as voice-en-us on Windows and Linux, or a
        /// macOS voice name such as Samantha; both are listed by `models`.)
        #[arg(long)]
        voiceover: Option<String>,
    },
    /// List, download or remove the optional AI models.
    Models {
        #[command(subcommand)]
        action: Option<ModelsCmd>,
    },
    /// Transcribe narration and read on-screen labels, then redraft step text.
    Process {
        project: PathBuf,
        /// Whisper model id (see `models`). Defaults to small.en, or any installed one.
        #[arg(long)]
        whisper: Option<String>,
    },
    /// First-run setup: download Cisco's OpenH264 for screen video.
    Setup,
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

#[derive(Subcommand)]
enum ModelsCmd {
    /// Download a model by id.
    Download { id: String },
    /// Delete a downloaded model.
    Remove { id: String },
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum GuideFormat {
    Md,
    Html,
    Pdf,
    Mkv,
    Mp4,
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
    /// Key combination that stops recording, e.g. `Ctrl+Shift+F12` or `Pause`.
    #[arg(long, default_value = "Ctrl+Shift+F12")]
    stop_key: String,
    /// Video quality, 1-100 (higher is sharper and larger).
    #[arg(long, default_value_t = openscrib_core::quality::DEFAULT_QUALITY, value_parser = clap::value_parser!(u8).range(1..=100))]
    quality: u8,
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
        Cmd::Export { project, format, out, full_frame, pdf_pages, quality, max_width, voiceover } => {
            export(&project, &format, out, full_frame, pdf_pages, quality, max_width, voiceover.as_deref())
        }
        Cmd::Models { action } => models(action),
        Cmd::Process { project, whisper } => process(&project, whisper.as_deref()),
        Cmd::Setup => {
            match video::locate_openh264() {
                Some(p) => info!("OpenH264 already installed: {}", p.display()),
                None => info!("installed {}", video::download_openh264()?.display()),
            }
            Ok(())
        }
        Cmd::Devices => devices(),
        Cmd::Probe { project, frame } => probe(&*load(&project)?, frame.as_deref()),
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
    let RecordArgs { out, no_keys, keep_dir, duration, no_video, no_audio, monitor, fps, mic, quality, stop_key } = args;
    let stop_key = openscrib_capture::Hotkey::parse(&stop_key)
        .with_context(|| format!("can't use \"{stop_key}\" as the stop key; try Ctrl+Shift+F12 or Pause"))?;
    if fps == 0 || fps > 60 {
        bail!("--fps must be between 1 and 60");
    }
    let mut media = MediaConfig::default();
    if !no_video {
        match video::locate_openh264() {
            Some(openh264) => media.video = Some(VideoConfig { monitor, fps, openh264, quality }),
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

    let cfg = SessionConfig { capture_keys: !no_keys, stop_key, ..Default::default() };
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
    let base = openscrib_core::paths::projects_dir();
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let stamp = now.format(time::macros::format_description!(
        "[year][month][day]-[hour][minute][second]"
    ))?;
    Ok(base.join(format!("recording-{stamp}.{PROJECT_EXT}")))
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
/// A project opened for reading. A packed `.scrib` is unpacked into a temp
/// folder that is deleted when this is dropped.
struct Loaded {
    dir: ProjectDir,
    _tmp: Option<tempfile::TempDir>,
}

impl std::ops::Deref for Loaded {
    type Target = ProjectDir;
    fn deref(&self) -> &ProjectDir {
        &self.dir
    }
}

fn load(path: &Path) -> Result<Loaded> {
    let resolved = resolve_project(path);
    let path = resolved.as_path();
    if path.is_dir() {
        return Ok(Loaded { dir: ProjectDir::open(path)?, _tmp: None });
    }
    let tmp = tempfile::Builder::new().prefix("openscrib-").tempdir()?;
    let dir = ProjectDir::unpack(path, tmp.path())?;
    Ok(Loaded { dir, _tmp: Some(tmp) })
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

#[allow(clippy::too_many_arguments)] // one per CLI flag
fn export(
    project: &Path,
    formats: &[GuideFormat],
    out: Option<PathBuf>,
    full_frame: bool,
    pdf_pages: bool,
    quality: u8,
    max_width: u32,
    voiceover: Option<&str>,
) -> Result<()> {
    use openscrib_export::{CropMode, Guide, RenderOptions};

    let resolved = resolve_project(project);
    let dir = load(&resolved)?;
    let manifest = dir.read_manifest()?;
    let stem = manifest.file_stem();
    let out = out.unwrap_or_else(|| {
        if manifest.title.is_some() {
            return resolved.with_file_name(&stem);
        }
        let stem = resolved.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "guide".into());
        resolved.with_file_name(format!("{stem}-export"))
    });
    fs::create_dir_all(&out)?;

    let opts = RenderOptions { crop: if full_frame { CropMode::Full } else { CropMode::Window }, ..Default::default() };
    // Rendering step images is only needed for the document formats.
    let mut guide_cache: Option<Guide> = None;
    for f in formats {
        let path = match f {
            GuideFormat::Md => {
                ensure_guide(&mut guide_cache, &dir, &manifest, &opts)?.write_markdown(&out, &stem)?;
                out.join(format!("{stem}.md"))
            }
            GuideFormat::Html => {
                let p = out.join(format!("{stem}.html"));
                ensure_guide(&mut guide_cache, &dir, &manifest, &opts)?.write_html(&p)?;
                p
            }
            GuideFormat::Mkv | GuideFormat::Mp4 => {
                let dll = video::locate_openh264().context("video export needs the OpenH264 DLL")?;
                let (fmt, name) = if *f == GuideFormat::Mkv {
                    (openscrib_export::video::VideoFormat::Mkv, format!("{stem}.mkv"))
                } else {
                    (openscrib_export::video::VideoFormat::Mp4, format!("{stem}.mp4"))
                };
                let p = out.join(&name);
                let mut opts = openscrib_export::video::VideoExportOptions::new(dll, fmt);
                opts.quality = quality;
                opts.max_width = if max_width == 0 { u32::MAX } else { max_width };
                if let Some(v) = voiceover {
                    opts.voiceover = Some(voice(v)?);
                }
                let started = std::time::Instant::now();
                let mut shown = 0;
                let stats = openscrib_export::video::export_video(&dir, &manifest, &opts, &p, |k| {
                    let pct = (k * 100.0) as u32;
                    if pct >= shown + 10 {
                        shown = pct;
                        eprint!("\r  {name}: {pct}%   ");
                    }
                })?;
                eprintln!();
                info!(
                    "{name}: {:.1}s, {} frames, {}x{}, {} segment(s) in {:.1}s",
                    stats.duration_ms as f64 / 1000.0,
                    stats.frames,
                    stats.width,
                    stats.height,
                    stats.segments,
                    started.elapsed().as_secs_f64()
                );
                p
            }
            GuideFormat::Pdf => {
                let p = out.join(format!("{stem}.pdf"));
                let g = ensure_guide(&mut guide_cache, &dir, &manifest, &opts)?;
                g.write_pdf(&p)?;
                if pdf_pages {
                    for (i, png) in g.pdf_page_pngs(1.5)?.iter().enumerate() {
                        fs::write(out.join(format!("page-{:02}.png", i + 1)), png)?;
                    }
                }
                p
            }
        };
        info!("wrote {}", path.display());
    }
    Ok(())
}

fn ensure_guide<'a>(
    cache: &'a mut Option<openscrib_export::Guide>,
    dir: &ProjectDir,
    manifest: &Manifest,
    opts: &openscrib_export::RenderOptions,
) -> Result<&'a openscrib_export::Guide> {
    if cache.is_none() {
        *cache = Some(openscrib_export::Guide::build(dir, manifest, opts)?);
    }
    Ok(cache.as_ref().expect("just built"))
}

/// The voice `--voiceover` asked for. Windows and Linux need Piper and a
/// voice downloaded; macOS speaks through the synthesiser it already ships,
/// so a name it doesn't know just means the user's own default voice.
#[cfg(not(target_os = "macos"))]
fn voice(id: &str) -> Result<openscrib_export::voiceover::Voice> {
    let (exe, model) = openscrib_process::models::tts_paths(Some(id))
        .context("voiceover needs Piper and a voice: openscrib models download piper && openscrib models download voice-en-us")?;
    Ok(openscrib_export::voiceover::Voice::Piper { exe, model })
}

#[cfg(target_os = "macos")]
fn voice(name: &str) -> Result<openscrib_export::voiceover::Voice> {
    let v = openscrib_export::voiceover::Voice::system(Some(name));
    if matches!(&v, openscrib_export::voiceover::Voice::System { name: None }) && !name.is_empty() {
        tracing::warn!("no voice called \"{name}\" here, using the system default; run `openscrib models` for the list");
    }
    Ok(v)
}

fn models(action: Option<ModelsCmd>) -> Result<()> {
    use openscrib_process::models::{self, CATALOG};

    match action {
        None => {
            println!("Models folder: {}", models::models_dir().display());
            for m in CATALOG {
                let state = if models::installed(m) { "installed" } else { "-" };
                println!("  {:<16} {:>6} MB  {:<10} {} ({})", m.id, m.size_mb, state, m.label, m.note);
            }
            let voices = openscrib_export::voiceover::system_voices();
            if !voices.is_empty() {
                println!("Voiceover voices built into this system (nothing to download; use with --voiceover):");
                for v in &voices {
                    println!("  {:<28} {}", v.name, v.locale);
                }
            }
        }
        Some(ModelsCmd::Download { id }) => {
            let m = models::find(&id).with_context(|| format!("unknown model {id}; run `openscrib models`"))?;
            let mut shown = 0;
            let path = models::download(m, |done, total| {
                let pct = total.map_or(0, |t| done * 100 / t.max(1));
                if pct >= shown + 5 {
                    shown = pct;
                    eprint!("\r  {id}: {pct}% of {} MB   ", m.size_mb);
                }
            })?;
            eprintln!();
            info!("installed {}", path.display());
        }
        Some(ModelsCmd::Remove { id }) => {
            let m = models::find(&id).with_context(|| format!("unknown model {id}"))?;
            models::remove(m)?;
            info!("removed {id}");
        }
    }
    Ok(())
}

/// Process a packed project in place: unpack, run, write the manifest, repack.
fn process(project: &Path, whisper: Option<&str>) -> Result<()> {
    use openscrib_process::{ProcessOptions, process};

    let resolved = resolve_project(project);
    let dir = load(&resolved)?;
    let mut manifest = dir.read_manifest()?;
    let opts = ProcessOptions::installed(whisper);
    let started = std::time::Instant::now();
    let report = process(&dir, &mut manifest, &opts, |p| info!("[{}] {}", p.stage, p.message))?;
    dir.write_manifest(&manifest)?;
    if resolved.is_file() {
        dir.pack(&resolved)?;
    }
    info!(
        "{} narration segments over {} steps, {} labels, {} tooltips, {} steps redrafted in {:.1}s",
        report.segments,
        report.steps_with_narration,
        report.labels,
        report.tooltips,
        report.redrafted,
        started.elapsed().as_secs_f64()
    );
    for s in &report.skipped {
        tracing::warn!("skipped: {s}");
    }
    print_info(&manifest)
}

fn devices() -> Result<()> {
    openscrib_capture::platform::init_dpi_awareness();
    let perms = openscrib_capture::permissions::status();
    if perms.required {
        println!("Permissions:");
        for (name, granted, what) in [
            ("Screen Recording", perms.screen, "screenshots, screen video and window titles"),
            ("Accessibility", perms.input, "the click and keystroke hook"),
        ] {
            let mark = if granted { "granted" } else { "NOT granted" };
            println!("  {name:<18} {mark:<12} {what}");
        }
        if !perms.all_granted() {
            println!("  Grant them in System Settings -> Privacy & Security.");
        }
    }
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
    // Live capture checks: these come back empty when a permission is
    // missing, which is the usual reason a recording looks blank. A window
    // with no title means Screen Recording in particular is missing.
    match openscrib_capture::platform::foreground_window() {
        Some(w) => println!("Front window: \"{}\" ({}) {:?}", w.title, w.process, w.rect),
        None => println!("Front window: none found"),
    }
    if let Some(at) = openscrib_capture::platform::cursor_pos() {
        print!("Cursor: {},{}", at.x, at.y);
        match openscrib_capture::platform::window_at(at) {
            Some(w) => println!(" over \"{}\" ({}) {:?}", w.title, w.process, w.rect),
            None => println!(" over no window"),
        }
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
