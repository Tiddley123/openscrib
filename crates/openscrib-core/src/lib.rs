//! Project model shared by every OpenSCRIB stage: the single recording clock,
//! the `project.json` manifest, the `.scrib` container and click markers.

pub mod clock;
pub mod manifest;
pub mod markers;
pub mod project;

pub use clock::Clock;
pub use manifest::*;
pub use project::{PROJECT_EXT, ProjectDir, ProjectError};
