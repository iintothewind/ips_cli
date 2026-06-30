use serde_json::{Map, Value};
use std::borrow::Cow;
use std::collections::HashSet;

use crate::types::{LoraInfo, PromptDetails};

/// Normalize ComfyUI JSON from API prompt, wrapped prompt, or UI workflow format.
pub fn normalize_workflow(json: &Value) -> Cow<'_, Value> {
    if let Some(api) = extract_api_nodes(json) {
        return Cow::Owned(Value::Object(api));
    }

    if let Some(inner) = json.get("prompt") {
        if let Some(api) = extract_api_nodes(inner) {
            return Cow::Owned(Value::Object(api));
        }
        if let Some(converted) = ui_workflow_to_api(inner) {
            return Cow::Owned(converted);
        }
    }

    if let Some(converted) = ui_workflow_to_api(json) {
        return Cow::Owned(converted);
    }

    Cow::Borrowed(json)
}

fn extract_api_nodes(json: &Value) -> Option<Map<String, Value>> {
    let obj = json.as_object()?;
    let nodes: Map<String, Value> = obj
        .iter()
        .filter(|(_, v)| v.get("class_type").is_some())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if nodes.is_empty() {
        None
    } else {
        Some(nodes)
    }
}

fn ui_workflow_to_api(json: &Value) -> Option<Value> {
    let nodes_arr = json.get("nodes")?.as_array()?;
    let links_arr = json.get("links").and_then(|v| v.as_array());

    let mut api = Map::new();

    for node in nodes_arr {
        let id = node_id_string(node.get("id")?)?;
        let class_type = node
            .get("type")
            .or_else(|| node.get("class_type"))
            .and_then(|v| v.as_str())?;

        let mut inputs = Map::new();

        if let Some(node_inputs) = node.get("inputs").and_then(|v| v.as_array()) {
            for input in node_inputs {
                let Some(name) = input.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                if let Some(link_id) = input.get("link").and_then(|v| v.as_u64()) {
                    if let Some((src_id, src_slot)) = resolve_link(links_arr, link_id) {
                        inputs.insert(
                            name.to_string(),
                            Value::Array(vec![
                                Value::String(src_id),
                                Value::Number(src_slot.into()),
                            ]),
                        );
                    }
                }
            }
        }

        apply_widgets_values(class_type, node.get("widgets_values"), &mut inputs);

        let mut node_obj = Map::new();
        node_obj.insert("class_type".to_string(), Value::String(class_type.to_string()));
        node_obj.insert("inputs".to_string(), Value::Object(inputs));
        api.insert(id, Value::Object(node_obj));
    }

    if api.is_empty() {
        None
    } else {
        Some(Value::Object(api))
    }
}

fn node_id_string(id: &Value) -> Option<String> {
    match id {
        Value::Number(n) => n.as_u64().map(|n| n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn resolve_link(links: Option<&Vec<Value>>, link_id: u64) -> Option<(String, u64)> {
    let links = links?;
    for link in links {
        let arr = link.as_array()?;
        if arr.first()?.as_u64()? != link_id {
            continue;
        }
        let src_id = node_id_string(arr.get(1)?)?;
        let src_slot = arr.get(2)?.as_u64()?;
        return Some((src_id, src_slot));
    }
    None
}

fn apply_widgets_values(class_type: &str, widgets: Option<&Value>, inputs: &mut Map<String, Value>) {
    let Some(widgets) = widgets.and_then(|v| v.as_array()) else {
        return;
    };

    let str_at = |idx: usize| widgets.get(idx).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty());

    if is_text_encode_node(class_type) {
        if inputs.get("text").is_none() {
            if let Some(text) = str_at(0) {
                inputs.insert("text".to_string(), Value::String(text.to_string()));
            }
        }
        if inputs.get("prompt").is_none() {
            if let Some(prompt) = str_at(0) {
                inputs.insert("prompt".to_string(), Value::String(prompt.to_string()));
            }
        }
        return;
    }

    match class_type {
        "CheckpointLoaderSimple" | "CheckpointLoader" | "CheckpointLoaderNF4" => {
            if inputs.get("ckpt_name").is_none() {
                if let Some(name) = str_at(0) {
                    inputs.insert("ckpt_name".to_string(), Value::String(name.to_string()));
                }
            }
        }
        "UNETLoader" | "UnetLoaderGGUF" => {
            if inputs.get("unet_name").is_none() {
                if let Some(name) = str_at(0) {
                    inputs.insert("unet_name".to_string(), Value::String(name.to_string()));
                }
            }
        }
        "LoraLoader" => {
            if inputs.get("lora_name").is_none() {
                if let Some(name) = str_at(0) {
                    inputs.insert("lora_name".to_string(), Value::String(name.to_string()));
                }
            }
            if inputs.get("strength_model").is_none() {
                if let Some(w) = widgets.get(1) {
                    inputs.insert("strength_model".to_string(), w.clone());
                }
            }
            if inputs.get("strength_clip").is_none() {
                if let Some(w) = widgets.get(2) {
                    inputs.insert("strength_clip".to_string(), w.clone());
                }
            }
        }
        "LoraLoaderModelOnly" => {
            if inputs.get("lora_name").is_none() {
                if let Some(name) = str_at(0) {
                    inputs.insert("lora_name".to_string(), Value::String(name.to_string()));
                }
            }
            if inputs.get("strength_model").is_none() {
                if let Some(w) = widgets.get(1) {
                    inputs.insert("strength_model".to_string(), w.clone());
                }
            }
        }
        _ => {}
    }
}

/// Extract text prompts from a ComfyUI workflow JSON object.
///
/// ComfyUI stores the workflow as a JSON object where each value is a node
/// with `class_type` and `inputs` fields. Text prompts live inside
/// CLIPTextEncode (and similar) nodes.
pub fn extract_from_workflow(json: &Value) -> Vec<String> {
    let normalized = normalize_workflow(json);
    let mut prompts = Vec::new();

    let nodes = match normalized.as_object() {
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
    let normalized = normalize_workflow(json);
    let prompts = extract_from_workflow(&normalized);
    let details = extract_details_from_workflow(&normalized);
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
    let normalized = normalize_workflow(json);
    let Some(nodes) = normalized.as_object() else {
        return PromptDetails::default();
    };

    let mut details = PromptDetails {
        model: extract_model(&normalized),
        loras: extract_loras(&normalized),
        positive_prompt: None,
        negative_prompt: None,
    };

    for (_node_id, node) in nodes {
        let class_type = node.get("class_type").and_then(|v| v.as_str()).unwrap_or_default();
        if !is_sampler_node(class_type) && class_type != "CFGGuider" && class_type != "BasicGuider" {
            continue;
        }

        let Some(inputs) = node.get("inputs").and_then(|v| v.as_object()) else {
            continue;
        };

        if details.positive_prompt.is_none() {
            details.positive_prompt = inputs
                .get("positive")
                .and_then(ref_node_id)
                .and_then(|id| resolve_conditioning_text(&normalized, &id, PromptRole::Positive));
        }
        if details.negative_prompt.is_none() {
            details.negative_prompt = inputs
                .get("negative")
                .and_then(ref_node_id)
                .and_then(|id| resolve_conditioning_text(&normalized, &id, PromptRole::Negative));
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

    // Fallback: any node carrying a model filename (custom loaders).
    for field in ["ckpt_name", "unet_name"] {
        for (_node_id, node) in nodes {
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

    for field in [role_field, "conditioning", "cond"] {
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
        || class_type.contains("SamplerCustom")
        || class_type == "SamplerCustomAdvanced"
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

    #[test]
    fn extracts_qwen_prompt_field() {
        let workflow = json!({
            "1": {
                "class_type": "TextEncodeQwenImageEditPlus",
                "inputs": { "prompt": "edit prompt text" }
            }
        });

        let prompts = extract_from_workflow(&workflow);
        assert_eq!(prompts, vec!["edit prompt text".to_string()]);
    }

    #[test]
    fn extracts_qwen_workflow_with_kontext_and_lora() {
        let workflow = json!({
            "37": {
                "class_type": "UNETLoader",
                "inputs": { "unet_name": "qwen_aio_v23.safetensors" }
            },
            "40": {
                "class_type": "LoraLoaderModelOnly",
                "inputs": {
                    "lora_name": "qwen\\image_edit_f2p_qwen.safetensors",
                    "strength_model": 0.35
                }
            },
            "76": {
                "class_type": "TextEncodeQwenImageEditPlus",
                "inputs": { "prompt": "qwen positive prompt" }
            },
            "78": {
                "class_type": "FluxKontextMultiReferenceLatentMethod",
                "inputs": { "conditioning": ["76", 0] }
            },
            "80": {
                "class_type": "ConditioningZeroOut",
                "inputs": { "conditioning": ["77", 0] }
            },
            "3": {
                "class_type": "KSampler",
                "inputs": {
                    "positive": ["78", 0],
                    "negative": ["80", 0]
                }
            }
        });

        let (prompt, details) = extract_workflow(&workflow);
        assert!(prompt.contains("qwen positive prompt"));
        assert_eq!(details.model.as_deref(), Some("qwen_aio_v23.safetensors"));
        assert_eq!(details.loras.len(), 1);
        assert_eq!(details.loras[0].name, "qwen\\image_edit_f2p_qwen.safetensors");
        assert_eq!(details.positive_prompt.as_deref(), Some("qwen positive prompt"));
        assert_eq!(details.negative_prompt, None);
    }

    #[test]
    fn unwraps_wrapped_prompt_object() {
        let workflow = json!({
            "client_id": "abc",
            "prompt": {
                "1": {
                    "class_type": "TextEncodeQwenImageEditPlus",
                    "inputs": { "prompt": "wrapped qwen prompt" }
                }
            }
        });

        let prompts = extract_from_workflow(&workflow);
        assert_eq!(prompts, vec!["wrapped qwen prompt".to_string()]);
    }

    #[test]
    fn parses_ui_workflow_nodes_array() {
        let workflow = json!({
            "nodes": [
                {
                    "id": 1,
                    "type": "TextEncodeQwenImageEditPlus",
                    "widgets_values": ["ui qwen prompt"]
                },
                {
                    "id": 2,
                    "type": "UNETLoader",
                    "widgets_values": ["qwen_aio_v23.safetensors", "default"]
                }
            ],
            "links": []
        });

        let (prompt, details) = extract_workflow(&workflow);
        assert_eq!(prompt, "ui qwen prompt");
        assert_eq!(details.model.as_deref(), Some("qwen_aio_v23.safetensors"));
    }
}
