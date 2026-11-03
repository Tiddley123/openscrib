//! Live capture: global input hooks, click-time screenshots and the step
//! state machine that turns raw input into a project manifest.

pub mod platform;
pub mod recorder;
pub mod screen;
pub mod session;

pub use recorder::record;
pub use session::SessionConfig;
