use std::path::PathBuf;
use serde::Serialize;

/// A prompt extracted from a single image file.
#[derive(Debug, Clone, Serialize)]
pub struct PromptRecord {
    pub path: PathBuf,
    pub prompt: String,
    pub generator: Generator,
    pub metadata_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Generator {
    A1111,
    ComfyUI,
    NovelAI,
    InvokeAI,
    Unknown,
}

impl std::fmt::Display for Generator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Generator::A1111 => write!(f, "a1111"),
            Generator::ComfyUI => write!(f, "comfyui"),
            Generator::NovelAI => write!(f, "novelai"),
            Generator::InvokeAI => write!(f, "invokeai"),
            Generator::Unknown => write!(f, "unknown"),
        }
    }
}

/// A matched result ready for output.
#[derive(Debug, Clone)]
pub struct MatchResult {
    pub record: PromptRecord,
    pub score: Option<i64>,
    pub match_ranges: Vec<(usize, usize)>, // byte ranges in prompt string
}

#[derive(Debug, Clone)]
pub struct Config {
    pub query: String,
    pub path: PathBuf,
    pub format: OutputFormat,
    pub match_mode: MatchMode,
    pub min_score: i64,
    pub full: bool,
    pub depth: Option<usize>,
    pub no_recursive: bool,
    pub threads: Option<usize>,
    pub verbose: bool,
    pub no_color: bool,
    pub path_only: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutputFormat {
    Console,
    Json,
    Csv,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchMode {
    Exact,
    Fuzzy,
    Regex,
}
