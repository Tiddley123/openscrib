//! Tauri backend for the OpenSCRIB editor: opens and saves `.scrib`
//! projects, runs recordings and exports, and hands the WebView file paths
//! it can load through the asset protocol.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use openscrib_capture::audio::AudioConfig;
use openscrib_capture::video::{self, VideoConfig};
use openscrib_capture::{MediaConfig, SessionConfig};
use openscrib_core::project::{SCREEN_FILE, read_packed_manifest};
use openscrib_core::{Manifest, PROJECT_EXT, ProjectDir, Rect};
use openscrib_export::render::{CropMode, RenderOptions, default_crop};
use openscrib_export::video::{VideoExportOptions, VideoFormat, export_video, remux_to_mp4};
use openscrib_export::Guide;
use openscrib_process::{ProcessOptions, models};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

struct OpenProject {
    source: PathBuf,
    dir: ProjectDir,
}

impl OpenProject {
    /// Remove the unpacked copy (and its preview) from the temp folder. The
    /// .scrib itself is untouched; unsaved edits are already gone by now.
    fn discard_work_dir(&self) {
        if let Some(root) = self.dir.root().parent() {
            let _ = fs::remove_dir_all(root);
        }
    }
}

struct Recording {
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
struct AppState {
    project: Mutex<Option<OpenProject>>,
    recording: Mutex<Option<Recording>>,
}

#[derive(Serialize)]
struct ProjectView {
    source: String,
    work_dir: String,
    manifest: Manifest,
    /// The crop each step gets when it doesn't set its own, in frame pixels.
    default_crops: Vec<Option<Rect>>,
    /// Frame size of each step (the captured monitor), if it has a frame.
    frame_sizes: Vec<Option<(u32, u32)>>,
    preview_video: Option<String>,
}

#[derive(Serialize)]
struct RecentItem {
    path: String,
    name: String,
    title: Option<String>,
    steps: usize,
    duration_ms: u64,
    modified: u64,
}

/// Recordings live in `projects/` beside the executable, like the CLI.
fn projects_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join("projects")))
        .unwrap_or_else(|| PathBuf::from("projects"))
}

#[tauri::command]
fn list_recordings() -> CmdResult<Vec<RecentItem>> {
    let dir = projects_dir();
    let mut items = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else { return Ok(items) };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some(PROJECT_EXT) {
            continue;
        }
        let Ok(m) = read_packed_manifest(&path) else { continue };
        let modified = e
            .metadata()
            .and_then(|md| md.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        items.push(RecentItem {
            name: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            path: path.to_string_lossy().into_owned(),
            title: m.title.clone(),
            steps: m.steps.len(),
            duration_ms: m.duration_ms,
            modified,
        });
    }
    items.sort_by_key(|i| std::cmp::Reverse(i.modified));
    Ok(items)
}

fn view(source: &Path, dir: &ProjectDir, preview: Option<PathBuf>) -> CmdResult<ProjectView> {
    let manifest = dir.read_manifest().map_err(err)?;
    let mut default_crops = Vec::new();
    let mut frame_sizes = Vec::new();
    for s in &manifest.steps {
        let size = s.frame.as_ref().and(s.monitor).and_then(|id| manifest.monitor(id)).map(|m| (m.rect.w, m.rect.h));
        default_crops.push(size.map(|sz| default_crop(&manifest, s, CropMode::Window, sz)));
        frame_sizes.push(size);
    }
    Ok(ProjectView {
        source: source.to_string_lossy().into_owned(),
        work_dir: dir.root().to_string_lossy().into_owned(),
        manifest,
        default_crops,
        frame_sizes,
        preview_video: preview.map(|p| p.to_string_lossy().into_owned()),
    })
}

#[tauri::command]
fn open_project(app: AppHandle, state: State<AppState>, path: String) -> CmdResult<ProjectView> {
    let source = PathBuf::from(&path);
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
    let root = std::env::temp_dir().join("openscrib-edit").join(format!("{stem}-{stamp}"));
    let dir = ProjectDir::unpack(&source, root.join("project")).map_err(err)?;
    app.asset_protocol_scope().allow_directory(&root, true).map_err(err)?;

    // The WebView can't play Matroska; give it an MP4 copy (outside the
    // project folder so it is never packed into the .scrib).
    let mkv = dir.path(SCREEN_FILE);
    let preview = if mkv.exists() {
        let out = root.join("preview.mp4");
        match remux_to_mp4(&mkv, &out) {
            Ok(()) => Some(out),
            Err(e) => {
                tracing::warn!("preview remux failed: {e:#}");
                None
            }
        }
    } else {
        None
    };

    let v = view(&source, &dir, preview)?;
    let previous = state.project.lock().map_err(err)?.replace(OpenProject { source, dir });
    if let Some(p) = previous {
        p.discard_work_dir();
    }
    Ok(v)
}

fn with_project<T>(state: &State<AppState>, f: impl FnOnce(&OpenProject) -> CmdResult<T>) -> CmdResult<T> {
    let guard = state.project.lock().map_err(err)?;
    let p = guard.as_ref().ok_or("no project is open")?;
    f(p)
}

#[tauri::command]
fn save_project(state: State<AppState>, manifest: Manifest) -> CmdResult<()> {
    with_project(&state, |p| {
        p.dir.write_manifest(&manifest).map_err(err)?;
        p.dir.pack(&p.source).map_err(err)
    })
}

#[derive(Deserialize)]
struct ExportRequest {
    manifest: Manifest,
    formats: Vec<String>,
    out_dir: String,
    full_frame: bool,
    quality: u8,
    /// 0 keeps the recording's own width.
    max_width: u32,
    /// Piper voice id to read the steps aloud with, replacing the narration.
    #[serde(default)]
    voiceover: Option<String>,
}

#[derive(Clone, Serialize)]
struct ExportProgress {
    format: String,
    progress: f32,
}

#[derive(Clone, Serialize)]
struct ExportDone {
    out_dir: String,
    files: Vec<String>,
}

#[tauri::command]
fn export_project(app: AppHandle, state: State<AppState>, req: ExportRequest) -> CmdResult<()> {
    // Exporting saves first, so the .scrib always matches what was exported.
    let dir = with_project(&state, |p| {
        p.dir.write_manifest(&req.manifest).map_err(err)?;
        p.dir.pack(&p.source).map_err(err)?;
        Ok(p.dir.clone())
    })?;
    std::thread::spawn(move || {
        let result = run_export(&app, &dir, req);
        match result {
            Ok(done) => {
                let _ = app.emit("export-done", done);
            }
            Err(e) => {
                let _ = app.emit("export-failed", err(e));
            }
        }
    });
    Ok(())
}

fn run_export(app: &AppHandle, dir: &ProjectDir, req: ExportRequest) -> anyhow::Result<ExportDone> {
    let out = PathBuf::from(&req.out_dir);
    fs::create_dir_all(&out)?;
    let manifest = req.manifest;
    let stem = manifest.file_stem();
    let opts = RenderOptions { crop: if req.full_frame { CropMode::Full } else { CropMode::Window }, ..Default::default() };
    let progress = |format: &str, progress: f32| {
        let _ = app.emit("export-progress", ExportProgress { format: format.into(), progress });
    };

    let mut guide: Option<Guide> = None;
    let mut files = Vec::new();
    for f in &req.formats {
        progress(f, 0.0);
        let path = match f.as_str() {
            "md" | "html" | "pdf" => {
                if guide.is_none() {
                    guide = Some(Guide::build(dir, &manifest, &opts)?);
                }
                let g = guide.as_ref().expect("built");
                match f.as_str() {
                    "md" => {
                        g.write_markdown(&out, &stem)?;
                        out.join(format!("{stem}.md"))
                    }
                    "html" => {
                        let p = out.join(format!("{stem}.html"));
                        g.write_html(&p)?;
                        p
                    }
                    _ => {
                        let p = out.join(format!("{stem}.pdf"));
                        g.write_pdf(&p)?;
                        p
                    }
                }
            }
            "mkv" | "mp4" => {
                let dll = video::locate_openh264()
                    .ok_or_else(|| anyhow::anyhow!("video export needs {}", video::OPENH264_DLL))?;
                let (fmt, name) = if f == "mkv" { (VideoFormat::Mkv, format!("{stem}.mkv")) } else { (VideoFormat::Mp4, format!("{stem}.mp4")) };
                let p = out.join(name);
                let mut opts = VideoExportOptions::new(dll, fmt);
                opts.quality = req.quality.clamp(1, 100);
                opts.max_width = if req.max_width == 0 { u32::MAX } else { req.max_width };
                if let Some(voice) = &req.voiceover {
                    let (piper, model) = models::tts_paths(Some(voice))
                        .ok_or_else(|| anyhow::anyhow!("voiceover needs the Piper engine and a voice (Settings -> Models)"))?;
                    opts.voiceover = Some(openscrib_export::voiceover::Voice { piper, model });
                }
                export_video(dir, &manifest, &opts, &p, |k| progress(f, k))?;
                p
            }
            other => anyhow::bail!("unknown format {other}"),
        };
        progress(f, 1.0);
        files.push(path.to_string_lossy().into_owned());
    }
    Ok(ExportDone { out_dir: out.to_string_lossy().into_owned(), files })
}

#[derive(Deserialize)]
struct RecordRequest {
    video: bool,
    audio: bool,
    monitor: Option<u32>,
    mic: Option<String>,
    quality: u8,
    #[serde(default)]
    stop_key: Option<String>,
}

#[tauri::command]
fn start_recording(app: AppHandle, state: State<AppState>, req: RecordRequest) -> CmdResult<String> {
    let mut rec = state.recording.lock().map_err(err)?;
    if rec.is_some() {
        return Err("already recording".into());
    }
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let stamp = now
        .format(time::macros::format_description!("[year][month][day]-[hour][minute][second]"))
        .map_err(err)?;
    let dir_out = projects_dir();
    fs::create_dir_all(&dir_out).map_err(err)?;
    let out = dir_out.join(format!("recording-{stamp}.{PROJECT_EXT}"));
    let work = out.with_extension(format!("{PROJECT_EXT}.recording"));
    let dir = ProjectDir::create(&work).map_err(err)?;

    let mut media = MediaConfig::default();
    if req.video
        && let Some(openh264) = video::locate_openh264()
    {
        media.video = Some(VideoConfig { monitor: req.monitor, fps: 12, openh264, quality: req.quality.clamp(1, 100) });
    }
    if req.audio {
        media.audio = Some(AudioConfig { device: req.mic });
    }
    let own_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    let stop_key = req.stop_key.as_deref().and_then(openscrib_capture::Hotkey::parse).unwrap_or_default();
    let cfg = SessionConfig { capture_keys: true, ignore_process: own_exe, stop_key };

    let stop = Arc::new(AtomicBool::new(false));
    *rec = Some(Recording { stop: stop.clone() });
    let out_s = out.to_string_lossy().into_owned();
    std::thread::spawn(move || {
        let result = openscrib_capture::record(dir.clone(), cfg, media, stop).and_then(|_| {
            dir.pack(&out)?;
            fs::remove_dir_all(&work)?;
            Ok(())
        });
        if let Some(state) = app.try_state::<AppState>()
            && let Ok(mut r) = state.recording.lock()
        {
            r.take();
        }
        match result {
            Ok(()) => {
                let _ = app.emit("recording-finished", out.to_string_lossy().into_owned());
            }
            Err(e) => {
                let _ = app.emit("recording-failed", err(e));
            }
        }
    });
    Ok(out_s)
}

#[tauri::command]
fn stop_recording(state: State<AppState>) -> CmdResult<()> {
    if let Some(r) = state.recording.lock().map_err(err)?.as_ref() {
        r.stop.store(true, Ordering::Relaxed);
    }
    Ok(())
}

#[derive(Serialize)]
struct Devices {
    monitors: Vec<openscrib_core::MonitorInfo>,
    mics: Vec<(String, bool)>,
    video_available: bool,
}

#[tauri::command]
fn list_devices() -> CmdResult<Devices> {
    openscrib_capture::platform::init_dpi_awareness();
    Ok(Devices {
        monitors: openscrib_capture::screen::monitors().map_err(err)?,
        mics: openscrib_capture::audio::input_devices().unwrap_or_default(),
        video_available: video::locate_openh264().is_some(),
    })
}

#[derive(Serialize)]
struct ModelStatus {
    #[serde(flatten)]
    info: models::ModelInfo,
    installed: bool,
}

#[tauri::command]
fn list_models() -> Vec<ModelStatus> {
    models::CATALOG
        .iter()
        .map(|m| ModelStatus { info: *m, installed: models::installed(m) })
        .collect()
}

#[derive(Clone, Serialize)]
struct ModelProgress {
    id: String,
    done: u64,
    total: Option<u64>,
}

#[derive(Clone, Serialize)]
struct ModelFailed {
    id: String,
    error: String,
}

#[tauri::command]
fn download_model(app: AppHandle, id: String) -> CmdResult<()> {
    let m = models::find(&id).ok_or_else(|| format!("unknown model {id}"))?;
    std::thread::spawn(move || {
        let mut last = 0u64;
        let result = models::download(m, |done, total| {
            // About one event per percent is plenty for a progress bar.
            let step = total.map_or(1 << 20, |t| (t / 100).max(1));
            if done - last >= step || Some(done) == total {
                last = done;
                let _ = app.emit("model-progress", ModelProgress { id: id.clone(), done, total });
            }
        });
        match result {
            Ok(_) => {
                let _ = app.emit("model-done", id);
            }
            Err(e) => {
                let _ = app.emit("model-failed", ModelFailed { id, error: err(e) });
            }
        }
    });
    Ok(())
}

#[tauri::command]
fn remove_model(id: String) -> CmdResult<()> {
    let m = models::find(&id).ok_or_else(|| format!("unknown model {id}"))?;
    models::remove(m).map_err(err)
}

#[derive(Clone, Serialize)]
struct ProcessDone {
    manifest: Manifest,
    report: openscrib_process::Report,
}

/// Run narration and OCR over the open project's current (unsaved) state;
/// the processed manifest comes back in a `process-done` event.
#[tauri::command]
fn process_project(app: AppHandle, state: State<AppState>, manifest: Manifest, whisper: Option<String>) -> CmdResult<()> {
    let dir = with_project(&state, |p| Ok(p.dir.clone()))?;
    std::thread::spawn(move || {
        let mut manifest = manifest;
        let opts = ProcessOptions::installed(whisper.as_deref());
        let result = openscrib_process::process(&dir, &mut manifest, &opts, |p| {
            let _ = app.emit("process-progress", p);
        });
        match result {
            Ok(report) => {
                let _ = app.emit("process-done", ProcessDone { manifest, report });
            }
            Err(e) => {
                let _ = app.emit("process-failed", err(e));
            }
        }
    });
    Ok(())
}

/// The automatic text for a step, for "Reset to draft" in the editor.
#[tauri::command]
fn draft_text(step: openscrib_core::Step) -> String {
    step.draft_text()
}

/// Fetch Cisco's OpenH264 binary next to the app (first-run setup).
#[tauri::command]
async fn download_openh264() -> CmdResult<String> {
    tauri::async_runtime::spawn_blocking(|| video::download_openh264().map(|p| p.to_string_lossy().into_owned()))
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command]
fn default_export_dir(state: State<AppState>, title: Option<String>) -> CmdResult<String> {
    with_project(&state, |p| {
        // Name the folder after the guide; fall back to the recording's name.
        let name = title
            .as_deref()
            .map(openscrib_core::safe_name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| {
                let stem = p.source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "guide".into());
                format!("{stem}-export")
            });
        Ok(p.source.with_file_name(name).to_string_lossy().into_owned())
    })
}

#[derive(Serialize)]
struct ImportedImage {
    rel: String,
    width: u32,
    height: u32,
}

/// Copy an image file into the open project (as PNG) for a step added in the
/// editor.
#[tauri::command]
fn import_image(state: State<AppState>, path: String, step_id: u32) -> CmdResult<ImportedImage> {
    with_project(&state, |p| {
        let img = image::open(&path).map_err(err)?.to_rgba8();
        let rel = ProjectDir::frame_name(step_id, "_import");
        img.save(p.dir.path(&rel)).map_err(err)?;
        Ok(ImportedImage { rel, width: img.width(), height: img.height() })
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .try_init();
    openscrib_capture::platform::init_dpi_awareness();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            list_recordings,
            open_project,
            save_project,
            export_project,
            start_recording,
            stop_recording,
            list_devices,
            default_export_dir,
            list_models,
            download_model,
            remove_model,
            process_project,
            draft_text,
            import_image,
            download_openh264,
        ])
        .build(tauri::generate_context!())
        .expect("error while building OpenSCRIB")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event
                && let Some(state) = app.try_state::<AppState>()
                && let Ok(mut p) = state.project.lock()
                && let Some(p) = p.take()
            {
                p.discard_work_dir();
            }
        });
}
