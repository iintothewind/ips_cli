pub mod comfyui;
pub mod exif;
pub mod jpeg;
pub mod png;
pub mod webp;

use std::path::Path;
use crate::types::{PromptRecord, LoraInfo};

/// Extract all prompt records from an image file.
/// Returns an empty Vec on unsupported format or error (errors are logged when verbose=true).
pub fn extract_prompt(path: &Path, verbose: bool) -> Vec<PromptRecord> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase());

    match ext.as_deref() {
        Some("png") => png::extract(path, verbose),
        Some("jpg") | Some("jpeg") => jpeg::extract(path, verbose),
        Some("webp") => webp::extract(path, verbose),
        _ => {
            if verbose {
                eprintln!("ips: unsupported format: {:?}", ext);
            }
            Vec::new()
        }
    }
}

/// Parse prompt components from extracted metadata.
/// Handles ComfyUI workflow JSON, A1111 parameters, NovelAI prompts.
/// Reference: ips_gui src/ips/extract/a1111.rs
pub fn parse_prompt_components(prompt: &str) -> (Option<String>, Vec<LoraInfo>, Option<String>, Option<String>) {
    // Check if this is ComfyUI JSON format
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(prompt) {
        return parse_comfyui_json(&json);
    }
    
    // 2. Parse LoRA from positive prompt (must be Some)
    // Reference: ips_gui src/ips/extract/a1111.rs parse_loras()
    let (positive, negative, model) = parse_prompt_and_model(prompt);
    let loras = if let Some(pos) = &positive {
        parse_loras(pos)
    } else {
        vec![]
    };

    (model, loras, positive, negative)
}

/// Parse ComfyUI workflow JSON and extract model, loras, positive/negative prompts.
fn parse_comfyui_json(json: &serde_json::Value) -> (Option<String>, Vec<LoraInfo>, Option<String>, Option<String>) {
    let details = comfyui::extract_details_from_workflow(json);
    let loras = details.loras;
    let positive_prompt = details.positive_prompt;
    let negative_prompt = details.negative_prompt;
    
    (details.model, loras, positive_prompt, negative_prompt)
}

/// Parse prompt text and extract positive/negative prompts and model.
/// Reference: ips_gui src/ips/extract/a1111.rs split_prompts_and_settings()
fn parse_prompt_and_model(text: &str) -> (Option<String>, Option<String>, Option<String>) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return (None, None, None);
    }

    // First, split by "Steps:" to separate prompt from settings
    let (body, settings) = match find_line_marker(text, "Steps:") {
        Some(idx) => (&text[..idx], Some(text[idx..].trim())),
        None => (text, None),
    };

    // Then, split body by "Negative prompt:"
    match body.find("Negative prompt:") {
        Some(idx) => {
            let positive = body[..idx].trim();
            let negative = body[idx + "Negative prompt:".len()..].trim();
            
            // Extract model from settings
            let model = if let Some(s) = settings {
                extract_model_from_settings(s)
            } else {
                None
            };
            
            (Some(positive.to_string()), Some(negative.to_string()), model)
        }
        None => {
            let positive = if body.trim_start().starts_with("Steps:") {
                ""
            } else {
                body.trim()
            };
            (Some(positive.to_string()), None, None)
        }
    }
}

/// Find line marker (either at start or after newline)
fn find_line_marker(text: &str, marker: &str) -> Option<usize> {
    if text.starts_with(marker) {
        return Some(0);
    }
    text.find(&format!("\n{marker}")).map(|idx| idx + 1)
}

/// Extract model name from settings text (e.g., "Steps: 30, Model: base.safetensors, Seed: 1")
fn extract_model_from_settings(settings: &str) -> Option<String> {
    // Look for "Model: " followed by filename
    if let Some(model_start) = settings.find("Model: ") {
        let after_model = &settings[model_start + 7..]; // skip "Model: "
        if let Some(quote) = after_model.find('"') {
            let model = &after_model[..quote];
            // Trim any trailing commas or spaces
            let model = model.trim_end_matches(',');
            return Some(model.to_string());
        }
    }
    None
}

/// Parse LoRA weights from prompt string.
/// Supports formats: "<lora:name:weight>", "(name:weight)", "name:(weight)"
/// Reference: ips_gui src/ips/extract/a1111.rs parse_loras()
fn parse_loras(prompt: &str) -> Vec<LoraInfo> {
    let mut loras: Vec<LoraInfo> = Vec::new();
    let mut rest = prompt;

    while let Some(start) = rest.find("<lora:") {
        let after_start = &rest[start + "<lora:".len()..];
        let Some(end) = after_start.find('>') else {
            break;
        };
        let tag = &after_start[..end];
        let mut parts = tag.rsplitn(2, ':');
        let weight = parts.next().unwrap_or_default().trim();
        let name = parts.next().unwrap_or_default().trim();

        if !name.is_empty() && !weight.is_empty() && !loras.iter().any(|l| l.name == name) {
            if let Ok(w) = weight.parse::<f32>() {
                loras.push(LoraInfo {
                    name: name.to_string(),
                    weight: w,
                });
            }
        }

        rest = &after_start[end + 1..];
    }

    loras
}
