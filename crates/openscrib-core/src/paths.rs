//! Where OpenSCRIB keeps its files.
//!
//! - **Portable** (the zip, and development builds): everything lives next
//!   to the executable, so the folder can move between machines.
//! - **Installed** (the per-user setup.exe, marked by an `openscrib-installed`
//!   file beside the exe, or a macOS `.app` bundle): the install is replaced
//!   on update and removed on uninstall, and a signed bundle must not be
//!   written into at all, so recordings go to `Documents/OpenSCRIB` and
//!   models, OpenH264 and settings to `%LOCALAPPDATA%\OpenSCRIB` or
//!   `~/Library/Application Support/OpenSCRIB`.

use std::path::{Path, PathBuf};

/// Shipped with the installer, never with the portable zip.
pub const INSTALLED_MARKER: &str = "openscrib-installed";

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `OpenSCRIB.app/Contents/MacOS/<exe>`. Code signing covers every byte of a
/// bundle, so writing inside one breaks its signature.
pub fn in_app_bundle() -> bool {
    cfg!(target_os = "macos")
        && exe_dir().ancestors().any(|d| d.extension().is_some_and(|e| e == "app"))
}

/// What the OS calls this program in a window list: inside a macOS `.app`
/// that is the bundle's name, which is what CoreGraphics reports as a
/// window's owner, not the executable's. Used to leave the editor's own
/// windows out of a recording.
pub fn process_name() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    if cfg!(target_os = "macos")
        && let Some(app) = exe.ancestors().find(|d| d.extension().is_some_and(|e| e == "app"))
    {
        return app.file_stem().map(|s| s.to_string_lossy().into_owned());
    }
    exe.file_name().map(|s| s.to_string_lossy().into_owned())
}

pub fn is_installed() -> bool {
    exe_dir().join(INSTALLED_MARKER).is_file() || in_app_bundle()
}

/// App data: models, OpenH264. Next to the exe when portable.
pub fn data_dir() -> PathBuf {
    if !is_installed() {
        return exe_dir();
    }
    if cfg!(target_os = "macos")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join("Library").join("Application Support").join("OpenSCRIB");
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("HOME")) {
        return PathBuf::from(local).join("OpenSCRIB");
    }
    exe_dir()
}

/// Default folder for recordings and exports.
pub fn projects_dir() -> PathBuf {
    if is_installed()
        && let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
    {
        return PathBuf::from(home).join("Documents").join("OpenSCRIB");
    }
    exe_dir().join("projects")
}
