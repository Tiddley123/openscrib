//! Live capture: global input hooks, click-time screenshots, continuous
//! screen video and narration, and the step state machine that turns raw
//! input into a project manifest.

pub mod audio;
pub mod hotkey;
pub mod media;
pub mod platform;
pub mod recorder;
pub mod screen;
pub mod session;
pub mod video;

pub use recorder::{MediaConfig, record};
pub use hotkey::Hotkey;
pub use session::SessionConfig;
