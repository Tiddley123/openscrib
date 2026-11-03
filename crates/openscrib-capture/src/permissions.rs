//! The privacy permissions a recording needs. Only macOS gates them, and it
//! fails quietly: without Screen Recording every capture is blank, and
//! without Accessibility the input hook never sees a click. Windows and
//! Linux report everything granted and the requests do nothing.

/// What the OS has granted. `required` is false where nothing is gated, so
/// the UI can leave the whole subject out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub required: bool,
    /// Screen Recording: screenshots, screen video and window titles.
    pub screen: bool,
    /// Accessibility: the global click and keystroke hook.
    pub input: bool,
}

impl Permissions {
    pub fn all_granted(&self) -> bool {
        !self.required || (self.screen && self.input)
    }
}

#[cfg(target_os = "macos")]
pub fn status() -> Permissions {
    Permissions {
        required: true,
        screen: crate::mac::screen_recording_granted(),
        input: crate::mac::accessibility_granted(),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn status() -> Permissions {
    Permissions { required: false, screen: true, input: true }
}

/// Ask for Screen Recording, returning the answer. macOS shows its prompt
/// only the first time a build asks, so follow a `false` with
/// [`open_screen_settings`].
pub fn request_screen() -> bool {
    #[cfg(target_os = "macos")]
    return crate::mac::request_screen_recording();
    #[cfg(not(target_os = "macos"))]
    true
}

/// Ask for Accessibility. macOS shows a prompt with a button that opens
/// System Settings.
pub fn request_input() -> bool {
    #[cfg(target_os = "macos")]
    return crate::mac::request_accessibility();
    #[cfg(not(target_os = "macos"))]
    true
}

pub fn open_screen_settings() {
    #[cfg(target_os = "macos")]
    let _ = crate::mac::open_privacy_pane("Privacy_ScreenCapture");
}

pub fn open_input_settings() {
    #[cfg(target_os = "macos")]
    let _ = crate::mac::open_privacy_pane("Privacy_Accessibility");
}

/// Changing either permission makes macOS restart the app, so a recording
/// started before the change would still be blind. Worth saying once.
pub const RESTART_NOTE: &str =
    "macOS quits and reopens OpenSCRIB when you change these, so grant them before recording.";
