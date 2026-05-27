//! Shared, model-agnostic sidecar scaffolding: audio capture, VAD, the
//! Talon JSON protocol, model download, and the run loop. Each engine binary
//! (parakeet, qwen) supplies a [`Transcriber`] and an [`EngineConfig`].

pub mod audio;
pub mod model;
pub mod protocol;
pub mod recognizer;
pub mod runtime;

pub use model::{download_to, ensure_files};
pub use protocol::{Command, Event};
pub use recognizer::Transcriber;
pub use runtime::{model_dir, run, sidecar_root};
