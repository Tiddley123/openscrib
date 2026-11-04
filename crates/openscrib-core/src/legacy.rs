//! Replacing a portable copy of OpenSCRIB with the installed one.
//!
//! Releases before the installer were folders people unzipped wherever they
//! liked, with recordings, models and OpenH264 kept inside. Nothing registered
//! them, so they are found from Windows' own record of programs that have run
//! plus a look through the usual places.
//!
//! Replacing one removes the program, never a recording: everything under
//! `projects\` is copied and checked byte for byte, then left where it was.
//! Models and OpenH264 can be downloaded again and are large, so they move.
//! Only files this project shipped are deleted, by name, and folders are only
//! removed once empty.

use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use crate::PROJECT_EXT;
use crate::paths;

const APP_EXE: &str = "OpenSCRIB.exe";
const HELPER_EXES: [&str; 2] = ["openscrib-cli.exe", "openscrib-vision.exe"];
/// Everything else a portable release put in its top folder.
const SHIPPED_FILES: [&str; 6] = ["msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll", "vcomp140.dll", "LICENSE-MIT.txt", "README.txt"];
/// Stands in for an unzipped release in `models\` until a model lands there.
const MODELS_PLACEHOLDER: &str = "README.txt";
const NOTE: &str = "Copied to the installed OpenSCRIB.txt";
/// `<folder>\OpenSCRIB-x.y.z-windows-x64\OpenSCRIB-x.y.z-windows-x64\` is what
/// Explorer's "Extract all" makes of the zip.
const SCAN_DEPTH: usize = 3;

/// Windows notes the path of programs started from Explorer in these keys.
#[cfg(windows)]
const RUN_HISTORY_KEYS: [&str; 2] = [
    r"HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache",
    r"HKCU\Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Compatibility Assistant\Store",
];

/// Where the installed app keeps what comes out of an old copy.
#[derive(Clone, Debug)]
pub struct Destinations {
    pub projects: PathBuf,
    pub models: PathBuf,
    /// OpenH264 sits directly in here.
    pub data: PathBuf,
}

impl Destinations {
    pub fn installed() -> Self {
        let data = paths::data_dir();
        Self { projects: paths::projects_dir(), models: data.join("models"), data }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Replaced {
    /// Recordings now in the installed app's folder, new or already there.
    pub recordings: usize,
    /// Other files from `projects\`, such as exports.
    pub other_files: usize,
    pub models_moved: usize,
    /// Set when the old folder is still there, holding the originals.
    pub folder_kept: bool,
}

/// True for a portable release, and for nothing else that happens to hold an
/// `OpenSCRIB.exe`: never an installed copy.
pub fn is_portable_copy(dir: &Path) -> bool {
    dir.join(APP_EXE).is_file()
        && HELPER_EXES.iter().any(|e| dir.join(e).is_file())
        && !dir.join(paths::INSTALLED_MARKER).exists()
}

/// Portable copies on this machine, apart from the running program's own.
pub fn find() -> Vec<PathBuf> {
    let mut seen = run_history();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = PathBuf::from(home);
        for sub in ["Desktop", "Downloads", "Documents"] {
            scan(&home.join(sub), SCAN_DEPTH, &mut seen);
        }
        scan(&home, 1, &mut seen);
    }
    let own = paths::exe_dir();
    let mut found: Vec<PathBuf> = seen.into_iter().filter(|d| is_portable_copy(d) && !same_dir(d, &own)).collect();
    found.sort_by_key(|d| d.to_string_lossy().to_lowercase());
    found.dedup_by(|a, b| same_dir(a, b));
    found
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn scan(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if is_portable_copy(dir) {
        out.push(dir.to_path_buf());
    }
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        // file_type doesn't follow links, so junctions can't send this in circles.
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            scan(&e.path(), depth - 1, out);
        }
    }
}

#[cfg(windows)]
fn run_history() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for key in RUN_HISTORY_KEYS {
        let Ok(out) = std::process::Command::new("reg").args(["query", key]).output() else { continue };
        dirs.extend(exe_folders(&String::from_utf8_lossy(&out.stdout)));
    }
    dirs
}

#[cfg(not(windows))]
fn run_history() -> Vec<PathBuf> {
    Vec::new()
}

/// The folders of every `OpenSCRIB.exe` named in `reg query` output, where a
/// line is a value name (`C:\a b\OpenSCRIB.exe.FriendlyAppName`), its type and
/// its data.
fn exe_folders(reg_query: &str) -> Vec<PathBuf> {
    let needle = format!("\\{}", APP_EXE.to_lowercase());
    reg_query
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let at = line.to_ascii_lowercase().find(&needle)?;
            Some(PathBuf::from(&line[..at]))
        })
        .collect()
}

/// How many recordings are in an old copy's `projects\`.
pub fn count_recordings(dir: &Path) -> usize {
    files_under(&dir.join("projects")).unwrap_or_default().iter().filter(|p| is_recording(p)).count()
}

fn is_recording(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == PROJECT_EXT)
}

/// Replaces the portable copy in `dir`. An error means the old program is
/// still there and nothing was lost, so it can simply be run again.
pub fn replace(dir: &Path, to: &Destinations) -> io::Result<Replaced> {
    if !is_portable_copy(dir) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} isn't a portable copy of OpenSCRIB", dir.display())));
    }
    // A running exe can't be opened for writing, so this catches it before
    // anything has been touched.
    fs::OpenOptions::new()
        .append(true)
        .open(dir.join(APP_EXE))
        .map_err(|e| io::Error::new(e.kind(), format!("{} is in use. Close OpenSCRIB and try again. ({e})", dir.display())))?;

    let mut done = Replaced::default();
    let projects = dir.join("projects");
    if projects.is_dir() {
        for file in files_under(&projects)? {
            let rel = file.strip_prefix(&projects).map_err(io::Error::other)?;
            if rel == Path::new(NOTE) {
                continue;
            }
            copy_keeping_both(&file, &to.projects.join(rel))?;
            if is_recording(&file) {
                done.recordings += 1;
            } else {
                done.other_files += 1;
            }
        }
    }

    let models = dir.join("models");
    if models.is_dir() {
        for file in files_under(&models)? {
            let rel = file.strip_prefix(&models).map_err(io::Error::other)?;
            if rel != Path::new(MODELS_PLACEHOLDER) && move_unless_present(&file, &to.models.join(rel))? {
                done.models_moved += 1;
            }
        }
    }
    for e in fs::read_dir(dir)?.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if name.starts_with("openh264") && e.path().is_file() {
            move_unless_present(&e.path(), &to.data.join(e.file_name()))?;
        }
    }

    // The app exe goes first: once it's gone this is no longer a portable copy.
    fs::remove_file(dir.join(APP_EXE))?;
    for name in HELPER_EXES.iter().chain(&SHIPPED_FILES) {
        let _ = fs::remove_file(dir.join(name));
    }
    let _ = fs::remove_file(models.join(MODELS_PLACEHOLDER));
    remove_empty_tree(&models);
    remove_empty_tree(&projects);

    if projects.is_dir() {
        let note = format!(
            "OpenSCRIB is now installed, and everything in this folder was copied to\r\n{}\r\n\r\nThis copy is yours to keep or delete.\r\n",
            to.projects.display()
        );
        let _ = fs::write(projects.join(NOTE), note);
    }
    done.folder_kept = fs::remove_dir(dir).is_err() && dir.exists();
    Ok(done)
}

fn files_under(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let e = e?;
            let kind = e.file_type()?;
            if kind.is_dir() {
                stack.push(e.path());
            } else if kind.is_file() {
                files.push(e.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Copies `from` to `to` and leaves `from` alone. A different file already at
/// `to` is kept, and this one lands beside it.
fn copy_keeping_both(from: &Path, to: &Path) -> io::Result<()> {
    let mut target = to.to_path_buf();
    let mut n = 1;
    while target.exists() {
        if same_contents(from, &target)? {
            return Ok(());
        }
        target = beside(to, n);
        n += 1;
    }
    copy_verified(from, &target)
}

/// `name.scrib` -> `name (old copy).scrib`, then `name (old copy 2).scrib`.
fn beside(path: &Path, n: usize) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let tag = if n == 1 { "old copy".to_string() } else { format!("old copy {n}") };
    let mut name = format!("{stem} ({tag})");
    if let Some(ext) = path.extension() {
        name.push('.');
        name.push_str(&ext.to_string_lossy());
    }
    path.with_file_name(name)
}

/// Written under a temporary name, so `to` is only ever a complete, checked
/// file.
fn copy_verified(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut part = to.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let result = fs::copy(from, &part).and_then(|_| {
        if same_contents(from, &part)? {
            Ok(())
        } else {
            Err(io::Error::other(format!("{} didn't copy cleanly", from.display())))
        }
    });
    if let Err(e) = result {
        let _ = fs::remove_file(&part);
        return Err(e);
    }
    fs::rename(&part, to)
}

/// Returns whether it moved. Anything already at `to` wins, and `from` stays.
fn move_unless_present(from: &Path, to: &Path) -> io::Result<bool> {
    if to.exists() {
        return Ok(false);
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    // Instant on one drive. Across drives it fails, and only then is it copied.
    if fs::rename(from, to).is_err() {
        copy_verified(from, to)?;
        fs::remove_file(from)?;
    }
    Ok(true)
}

fn same_contents(a: &Path, b: &Path) -> io::Result<bool> {
    if fs::metadata(a)?.len() != fs::metadata(b)?.len() {
        return Ok(false);
    }
    let mut a = BufReader::with_capacity(1 << 20, File::open(a)?);
    let mut b = BufReader::with_capacity(1 << 20, File::open(b)?);
    let (mut buf_a, mut buf_b) = (vec![0; 1 << 16], vec![0; 1 << 16]);
    loop {
        let n = a.read(&mut buf_a)?;
        if n == 0 {
            return Ok(true);
        }
        b.read_exact(&mut buf_b[..n])?;
        if buf_a[..n] != buf_b[..n] {
            return Ok(false);
        }
    }
}

/// `remove_dir` refuses anything with a file in it, which is the point.
fn remove_empty_tree(dir: &Path) {
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                remove_empty_tree(&e.path());
            }
        }
    }
    let _ = fs::remove_dir(dir);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn old_copy(root: &Path) -> PathBuf {
        let dir = root.join("OpenSCRIB-0.1.0-windows-x64");
        for name in [APP_EXE, HELPER_EXES[0], HELPER_EXES[1]].into_iter().chain(SHIPPED_FILES) {
            write(&dir.join(name), "shipped");
        }
        write(&dir.join("models").join(MODELS_PLACEHOLDER), "shipped");
        dir
    }

    fn destinations(root: &Path) -> Destinations {
        let data = root.join("data");
        Destinations { projects: root.join("docs"), models: data.join("models"), data }
    }

    #[test]
    fn recordings_are_copied_and_stay_where_they_were() {
        let tmp = tempdir().unwrap();
        let dir = old_copy(tmp.path());
        write(&dir.join("projects/one.scrib"), "one");
        write(&dir.join("projects/Guide/guide.pdf"), "pdf");
        let to = destinations(tmp.path());

        let done = replace(&dir, &to).unwrap();

        assert_eq!(done, Replaced { recordings: 1, other_files: 1, models_moved: 0, folder_kept: true });
        assert_eq!(fs::read_to_string(to.projects.join("one.scrib")).unwrap(), "one");
        assert_eq!(fs::read_to_string(to.projects.join("Guide/guide.pdf")).unwrap(), "pdf");
        assert_eq!(fs::read_to_string(dir.join("projects/one.scrib")).unwrap(), "one");
        assert_eq!(fs::read_to_string(dir.join("projects/Guide/guide.pdf")).unwrap(), "pdf");
        assert!(dir.join("projects").join(NOTE).is_file());
    }

    #[test]
    fn program_files_go_but_anything_unknown_stays() {
        let tmp = tempdir().unwrap();
        let dir = old_copy(tmp.path());
        write(&dir.join("my notes.txt"), "mine");
        write(&dir.join("models/custom/file.bin"), "model");

        let done = replace(&dir, &destinations(tmp.path())).unwrap();

        assert!(done.folder_kept);
        assert!(!is_portable_copy(&dir));
        for name in [APP_EXE, HELPER_EXES[0], HELPER_EXES[1]].into_iter().chain(SHIPPED_FILES) {
            assert!(!dir.join(name).exists(), "{name} should be gone");
        }
        assert_eq!(fs::read_to_string(dir.join("my notes.txt")).unwrap(), "mine");
    }

    #[test]
    fn a_folder_with_nothing_of_the_users_is_removed() {
        let tmp = tempdir().unwrap();
        let dir = old_copy(tmp.path());

        let done = replace(&dir, &destinations(tmp.path())).unwrap();

        assert!(!done.folder_kept);
        assert!(!dir.exists());
    }

    #[test]
    fn models_and_openh264_move() {
        let tmp = tempdir().unwrap();
        let dir = old_copy(tmp.path());
        write(&dir.join("models/whisper.bin"), "model");
        write(&dir.join("models/voices/cori.onnx"), "voice");
        write(&dir.join("openh264-2.6.0-win64.dll"), "dll");
        let to = destinations(tmp.path());

        let done = replace(&dir, &to).unwrap();

        assert_eq!(done.models_moved, 2);
        assert_eq!(fs::read_to_string(to.models.join("whisper.bin")).unwrap(), "model");
        assert_eq!(fs::read_to_string(to.models.join("voices/cori.onnx")).unwrap(), "voice");
        assert_eq!(fs::read_to_string(to.data.join("openh264-2.6.0-win64.dll")).unwrap(), "dll");
        assert!(!to.models.join(MODELS_PLACEHOLDER).exists());
    }

    #[test]
    fn a_recording_with_the_same_name_never_overwrites_another() {
        let tmp = tempdir().unwrap();
        let dir = old_copy(tmp.path());
        write(&dir.join("projects/one.scrib"), "old");
        let to = destinations(tmp.path());
        write(&to.projects.join("one.scrib"), "newer work");

        replace(&dir, &to).unwrap();

        assert_eq!(fs::read_to_string(to.projects.join("one.scrib")).unwrap(), "newer work");
        assert_eq!(fs::read_to_string(to.projects.join("one (old copy).scrib")).unwrap(), "old");
    }

    #[test]
    fn running_it_again_adds_no_duplicates() {
        let tmp = tempdir().unwrap();
        let to = destinations(tmp.path());
        let first = tmp.path().join("first");
        let second = tmp.path().join("second");
        let (a, b) = (old_copy(&first), old_copy(&second));
        write(&a.join("projects/one.scrib"), "same");
        write(&b.join("projects/one.scrib"), "same");

        replace(&a, &to).unwrap();
        replace(&b, &to).unwrap();

        assert_eq!(files_under(&to.projects).unwrap().len(), 1);
    }

    #[test]
    fn a_running_copy_is_refused_with_nothing_touched() {
        let tmp = tempdir().unwrap();
        let dir = old_copy(tmp.path());
        write(&dir.join("projects/one.scrib"), "one");
        let to = destinations(tmp.path());
        let exe = dir.join(APP_EXE);
        let mut perms = fs::metadata(&exe).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&exe, perms.clone()).unwrap();

        assert!(replace(&dir, &to).is_err());
        assert!(!to.projects.exists());
        assert!(exe.exists());

        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&exe, perms).unwrap();
    }

    #[test]
    fn an_installed_copy_is_not_a_portable_one() {
        let tmp = tempdir().unwrap();
        let dir = old_copy(tmp.path());
        assert!(is_portable_copy(&dir));
        write(&dir.join(paths::INSTALLED_MARKER), "");
        assert!(!is_portable_copy(&dir));
        assert!(replace(&dir, &destinations(tmp.path())).is_err());
    }

    #[test]
    fn scan_finds_a_copy_zip_extraction_nested_twice() {
        let tmp = tempdir().unwrap();
        let dir = old_copy(&tmp.path().join("OpenSCRIB-0.1.0-windows-x64"));
        let mut found = Vec::new();
        scan(tmp.path(), SCAN_DEPTH, &mut found);
        assert_eq!(found, [dir]);
    }

    #[test]
    fn reads_exe_folders_from_reg_query_output() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Classes\\Local Settings\r\n    \
                   G:\\My Tools\\OpenSCRIB-0.1.0\\OpenSCRIB.exe.FriendlyAppName    REG_SZ    OpenSCRIB\r\n    \
                   C:\\Windows\\notepad.exe.FriendlyAppName    REG_SZ    Notepad\r\n    \
                   D:\\usb\\openscrib.EXE    REG_BINARY    53414353\r\n";
        assert_eq!(exe_folders(out), [PathBuf::from("G:\\My Tools\\OpenSCRIB-0.1.0"), PathBuf::from("D:\\usb")]);
    }
}
