use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use openscrib_capture::SessionConfig;
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
    /// Record clicks and keystrokes until Ctrl+Shift+F12 (or Ctrl+C).
    Record {
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
    },
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
        Cmd::Record { out, no_keys, keep_dir, duration } => record(out, no_keys, keep_dir, duration),
        Cmd::Info { project } => print_info(&load(&project)?.read_manifest()?),
        Cmd::Markers { project, out } => markers(&project, &out),
        Cmd::Pack { dir, out } => {
            ProjectDir::open(dir)?.pack(&out)?;
            info!("wrote {}", out.display());
            Ok(())
        }
    }
}

fn record(out: Option<PathBuf>, no_keys: bool, keep_dir: bool, duration: Option<u64>) -> Result<()> {
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
    let manifest = openscrib_capture::record(dir.clone(), cfg, stop)?;
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

/// Open a project from a directory, or unpack a .scrib into a temp dir.
fn load(path: &Path) -> Result<ProjectDir> {
    if path.is_dir() {
        return Ok(ProjectDir::open(path)?);
    }
    let tmp = std::env::temp_dir().join(format!("openscrib-{}", std::process::id()));
    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }
    Ok(ProjectDir::unpack(path, tmp)?)
}

fn print_info(m: &Manifest) -> Result<()> {
    println!("version {} · {:.1}s · {} steps", m.version, m.duration_ms as f64 / 1000.0, m.steps.len());
    for mon in &m.monitors {
        println!("  monitor {} \"{}\" {:?} @{}x", mon.id, mon.name, mon.rect, mon.scale);
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
                draw_click(&mut img, c);
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
