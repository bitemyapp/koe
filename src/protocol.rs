use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const MAX_MESSAGE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Start,
    Stop,
    Toggle,
    Pause,
    Resume,
    Cancel,
    Status,
    Subscribe,
    Last,
    CopyLast,
    PasteLast,
    ClearLast,
    Unload,
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub action: Action,
    #[serde(default = "version")]
    pub version: u32,
}
fn version() -> u32 {
    VERSION
}
impl Request {
    pub fn new(action: Action) -> Self {
        Self {
            action,
            version: VERSION,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    #[default]
    Idle,
    Recording,
    Paused,
    Finishing,
    Transcribing,
    Delivering,
    Error,
}

/// Status never contains audio or transcript text. `last` explicitly retrieves text.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Status {
    pub phase: Phase,
    pub sequence: u64,
    pub elapsed_ms: u64,
    /// Root mean square level, normalized to 0..1.
    pub level: f32,
    pub has_last: bool,
    pub model_loaded: bool,
    /// Recognition wall time for the last completed attempt, excluding capture.
    #[serde(default)]
    pub transcription_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub version: u32,
    pub ok: bool,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
impl Response {
    pub fn status(status: Status) -> Self {
        Self {
            version: VERSION,
            ok: true,
            status,
            text: None,
            error: None,
        }
    }
}
