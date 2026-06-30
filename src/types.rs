use std::path::PathBuf;
use serde::Serialize;

/// A prompt extracted from a single image file.
#[derive(Debug, Clone, Serialize)]
pub struct PromptRecord {
    pub path: PathBuf,
    pub prompt: String,
    pub generator: Generator,
    pub metadata_key: String,
    /// Extracted prompt details (model, loras, positive/negative prompts)
    pub details: Option<PromptDetails>,
}

impl PromptRecord {
    pub fn with_details(
        path: PathBuf,
        prompt: String,
        generator: Generator,
        metadata_key: impl Into<String>,
        details: PromptDetails,
    ) -> Self {
        Self {
            path,
            prompt,
            generator,
            metadata_key: metadata_key.into(),
            details: Some(details),
        }
    }

    pub fn details_or_default(&self) -> PromptDetails {
        self.details.clone().unwrap_or_default()
    }

    pub fn to_structured(&self) -> StructuredPromptRecord {
        let details = self.details_or_default();
        StructuredPromptRecord {
            path: self.path.clone(),
            generator: self.generator.clone(),
            model: details.model,
            loras: details.loras,
            positive: details.positive_prompt,
            negative: details.negative_prompt,
        }
    }
}

/// LoRA info from ComfyUI workflow or A1111 prompt tags.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LoraInfo {
    pub name: String,
    pub weight: String,
}

/// Prompt details extracted from metadata (ComfyUI workflow, A1111 parameters, etc.)
#[derive(Debug, Clone, Default, Serialize)]
pub struct PromptDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loras: Vec<LoraInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub positive_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,
}

/// A structured prompt record with parsed components.
#[derive(Debug, Clone, Serialize)]
pub struct StructuredPromptRecord {
    pub path: PathBuf,
    pub generator: Generator,
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub loras: Vec<LoraInfo>,
    pub positive: Option<String>,
    pub negative: Option<String>,
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
    pub structured: bool,
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
