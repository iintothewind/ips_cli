use serde_json::Value;
use std::collections::HashSet;

use crate::types::{LoraInfo, PromptDetails};

/// Extract text prompts from a ComfyUI workflow JSON object.
///
/// ComfyUI stores the workflow as a JSON object where each value is a node
/// with `class_type` and `inputs` fields. Text prompts live inside
/// CLIPTextEncode (and similar) nodes.
pub fn extract_from_workflow(json: &Value) -> Vec<String> {
    let mut prompts = Vec::new();

    let nodes = match json.as_object() {
        Some(o) => o,
        None => return prompts,
    };

    for (_node_id, node) in nodes {
        let class_type = match node.get("class_type").and_then(|v| v.as_str()) {
            Some(ct) => ct,
            None => continue,
        };

        if !is_text_encode_node(class_type) {
            continue;
        }

        let inputs = match node.get("inputs").and_then(|v| v.as_object()) {
            Some(i) => i,
            None => continue,
        };

        // CLIPTextEncodeSDXL uses text_g and text_l
        for field in &["text_g", "text_l", "text", "prompt"] {
            if let Some(text) = inputs.get(*field).and_then(|v| v.as_str()) {
                let text = text.trim().to_string();
                if !text.is_empty() && !prompts.contains(&text) {
                    prompts.push(text);
                }
            }
        }
    }

    prompts
}

/// Extract searchable prompt text and structured details from a ComfyUI workflow.
pub fn extract_workflow(json: &Value) -> (String, PromptDetails) {
    let prompts = extract_from_workflow(json);
    let details = extract_details_from_workflow(json);
    let prompt = if !prompts.is_empty() {
        prompts.join(" | ")
    } else {
        [details.positive_prompt.as_deref(), details.negative_prompt.as_deref()]
            .into_iter()
            .flatten()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(" | ")
    };
    (prompt, details)
}

pub fn has_extractable_content(prompt: &str, details: &PromptDetails) -> bool {
    !prompt.trim().is_empty()
        || details.model.is_some()
        || !details.loras.is_empty()
        || details.positive_prompt.is_some()
        || details.negative_prompt.is_some()
}

pub fn extract_details_from_workflow(json: &Value) -> PromptDetails {
    let Some(nodes) = json.as_object() else {
        return PromptDetails::default();
    };

    let mut details = PromptDetails {
        model: extract_model(json),
        loras: extract_loras(json),
        positive_prompt: None,
        negative_prompt: None,
    };

    for (_node_id, node) in nodes {
        let class_type = node.get("class_type").and_then(|v| v.as_str()).unwrap_or_default();
        if !is_sampler_node(class_type) && class_type != "CFGGuider" {
            continue;
        }

        let Some(inputs) = node.get("inputs").and_then(|v| v.as_object()) else {
            continue;
        };

        if details.positive_prompt.is_none() {
            details.positive_prompt = inputs
                .get("positive")
                .and_then(ref_node_id)
                .and_then(|id| resolve_conditioning_text(json, &id, PromptRole::Positive));
        }
        if details.negative_prompt.is_none() {
            details.negative_prompt = inputs
                .get("negative")
                .and_then(ref_node_id)
                .and_then(|id| resolve_conditioning_text(json, &id, PromptRole::Negative));
        }

        if details.positive_prompt.is_some() || details.negative_prompt.is_some() {
            break;
        }
    }

    details
}

fn extract_model(json: &Value) -> Option<String> {
    let nodes = json.as_object()?;

    for field in ["ckpt_name", "unet_name"] {
        for (_node_id, node) in nodes {
            let class_type = node.get("class_type").and_then(|v| v.as_str()).unwrap_or_default();
            if !is_generation_model_node(class_type) {
                continue;
            }

            if let Some(model) = node
                .get("inputs")
                .and_then(|v| v.get(field))
                .and_then(value_to_string)
                .and_then(non_empty)
            {
                return Some(model);
            }
        }
    }

    None
}

fn is_generation_model_node(class_type: &str) -> bool {
    matches!(
        class_type,
        "CheckpointLoaderSimple"
            | "CheckpointLoader"
            | "CheckpointLoaderNF4"
            | "UNETLoader"
            | "UnetLoaderGGUF"
    )
}

fn extract_loras(json: &Value) -> Vec<LoraInfo> {
    let Some(nodes) = json.as_object() else {
        return Vec::new();
    };

    let mut loras = Vec::new();

    for (_node_id, node) in nodes {
        let class_type = node.get("class_type").and_then(|v| v.as_str()).unwrap_or_default();
        if !class_type.to_lowercase().contains("lora") {
            continue;
        }

        let Some(inputs) = node.get("inputs").and_then(|v| v.as_object()) else {
            continue;
        };

        if let Some(name) = inputs
            .get("lora_name")
            .and_then(value_to_string)
            .and_then(non_empty)
        {
            let model_weight = inputs.get("strength_model").and_then(value_to_string);
            let clip_weight = inputs.get("strength_clip").and_then(value_to_string);
            let weight = format_lora_weight(model_weight.as_deref(), clip_weight.as_deref())
                .unwrap_or_else(|| "1".to_string());
            push_lora(&mut loras, name, weight);
        }

        for (_field, value) in inputs {
            let Some(value) = value.as_object() else {
                continue;
            };
            if matches!(value.get("on").and_then(|v| v.as_bool()), Some(false)) {
                continue;
            }
            let Some(name) = value
                .get("lora")
                .and_then(value_to_string)
                .and_then(non_empty)
            else {
                continue;
            };
            let weight = value
                .get("strength")
                .and_then(value_to_string)
                .or_else(|| value.get("strength_model").and_then(value_to_string))
                .unwrap_or_else(|| "1".to_string());
            push_lora(&mut loras, name, weight);
        }
    }

    loras
}

fn push_lora(loras: &mut Vec<LoraInfo>, name: String, weight: String) {
    if loras.iter().any(|l| l.name == name) {
        return;
    }
    loras.push(LoraInfo { name, weight });
}

fn format_lora_weight(model_weight: Option<&str>, clip_weight: Option<&str>) -> Option<String> {
    match (model_weight, clip_weight) {
        (Some(model), Some(clip)) if model == clip => Some(model.to_string()),
        (Some(model), Some(clip)) => Some(format!("{model} / {clip}")),
        (Some(model), None) => Some(model.to_string()),
        (None, Some(clip)) => Some(clip.to_string()),
        (None, None) => None,
    }
}

#[derive(Clone, Copy)]
enum PromptRole {
    Positive,
    Negative,
}

fn resolve_conditioning_text(json: &Value, node_id: &str, role: PromptRole) -> Option<String> {
    let mut visited = HashSet::new();
    resolve_conditioning_text_inner(json, node_id, role, &mut visited)
}

fn resolve_conditioning_text_inner(
    json: &Value,
    node_id: &str,
    role: PromptRole,
    visited: &mut HashSet<String>,
) -> Option<String> {
    if !visited.insert(node_id.to_string()) {
        return None;
    }

    let node = json.as_object()?.get(node_id)?;
    let class_type = node.get("class_type").and_then(|v| v.as_str()).unwrap_or_default();
    if class_type.contains("ZeroOut") {
        return None;
    }

    if is_text_encode_node(class_type) {
        if let Some(text) = extract_text_from_node(node) {
            return Some(text);
        }
    }

    let inputs = node.get("inputs").and_then(|v| v.as_object())?;
    let role_field = match role {
        PromptRole::Positive => "positive",
        PromptRole::Negative => "negative",
    };

    for field in [role_field, "conditioning"] {
        if let Some(next_id) = inputs.get(field).and_then(ref_node_id) {
            if let Some(text) = resolve_conditioning_text_inner(json, &next_id, role, visited) {
                return Some(text);
            }
        }
    }

    None
}

fn extract_text_from_node(node: &Value) -> Option<String> {
    let inputs = node.get("inputs").and_then(|v| v.as_object())?;
    let mut parts = Vec::new();

    for field in ["text_g", "text_l", "text", "prompt"] {
        if let Some(text) = inputs
            .get(field)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            if !parts.contains(&text) {
                parts.push(text);
            }
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn is_sampler_node(class_type: &str) -> bool {
    class_type.contains("KSampler")
}

fn is_text_encode_node(class_type: &str) -> bool {
    matches!(
        class_type,
        "CLIPTextEncode"
            | "CLIPTextEncodeSDXL"
            | "CLIPTextEncodeFlux"
            | "TextEncodeQwenImageEdit"
            | "TextEncodeQwenImageEditPlus"
    ) || class_type.contains("Prompt")
        || class_type.contains("TextEncode")
}

fn ref_node_id(value: &Value) -> Option<String> {
    let array = value.as_array()?;
    let id = array.first()?;
    value_to_string(id).and_then(non_empty)
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_clip_text_encode() {
        let workflow = json!({
            "1": {
                "class_type": "CLIPTextEncode",
                "inputs": {
                    "text": "a beautiful sunset over the ocean"
                }
            },
            "2": {
                "class_type": "CLIPTextEncode",
                "inputs": {
                    "text": "ugly, blurry, low quality"
                }
            }
        });
        let prompts = extract_from_workflow(&workflow);
        assert_eq!(prompts.len(), 2);
        assert!(prompts.contains(&"a beautiful sunset over the ocean".to_string()));
        assert!(prompts.contains(&"ugly, blurry, low quality".to_string()));
    }

    #[test]
    fn extracts_sdxl_text_g_and_l() {
        let workflow = json!({
            "3": {
                "class_type": "CLIPTextEncodeSDXL",
                "inputs": {
                    "text_g": "a detailed landscape",
                    "text_l": "soft lighting, 4k"
                }
            }
        });
        let prompts = extract_from_workflow(&workflow);
        assert_eq!(prompts.len(), 2);
    }

    #[test]
    fn deduplicates_identical_prompts() {
        let workflow = json!({
            "1": {
                "class_type": "CLIPTextEncode",
                "inputs": { "text": "same prompt" }
            },
            "2": {
                "class_type": "CLIPTextEncode",
                "inputs": { "text": "same prompt" }
            }
        });
        let prompts = extract_from_workflow(&workflow);
        assert_eq!(prompts.len(), 1);
    }

    #[test]
    fn ignores_non_text_nodes() {
        let workflow = json!({
            "1": {
                "class_type": "KSampler",
                "inputs": { "steps": 20, "cfg": 7.0 }
            }
        });
        let prompts = extract_from_workflow(&workflow);
        assert!(prompts.is_empty());
    }
}
