//! Guide and video exports built from a project: step images with markers
//! and redactions, Markdown, HTML, PDF, and the stitched video.

pub mod guide;
pub mod icons;
pub mod pdf;
pub mod render;
pub mod video;
pub mod voiceover;

pub use guide::Guide;
pub use render::{CropMode, RenderOptions};
