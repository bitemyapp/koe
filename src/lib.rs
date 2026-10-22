//! Koe's versioned desktop protocol and dictation engine.
pub mod audio;
pub mod config;
pub mod daemon;
pub mod desktop;
pub mod engine;
pub mod ipc;
pub mod models;
pub mod protocol;
pub mod text;
pub mod tray;
#[cfg(feature = "ui")]
pub mod ui;
pub mod vocabulary;
