//! Where OpenSCRIB keeps its files.
//!
//! - **Portable** (the zip, and development builds): everything lives next
//!   to the executable, so the folder can move between machines.
//! - **Installed** (the per-user setup.exe, marked by an `openscrib-installed`
//!   file beside the exe): the install folder is replaced on update and
//!   removed on uninstall, so recordings go to `Documents\OpenSCRIB` and
//!   models, OpenH264 and settings to `%LOCALAPPDATA%\OpenSCRIB`.

use std::path::{Path, PathBuf};

/// Shipped with the installer, never with the portable zip.
pub const INSTALLED_MARKER: &str = "openscrib-installed";

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn is_installed() -> bool {
    exe_dir().join(INSTALLED_MARKER).is_file()
}

/// App data: models, OpenH264. Next to the exe when portable.
pub fn data_dir() -> PathBuf {
    if is_installed()
        && let Some(local) = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("HOME"))
    {
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
