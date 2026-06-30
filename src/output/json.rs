use serde::Serialize;
use crate::types::{Config, MatchResult, LoraInfo};

#[derive(Serialize)]
struct JsonRecord {
    path: String,
    #[serde(rename = "generator")]
    generator: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    score: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    loras: Vec<LoraInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    positive_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    negative_prompt: Option<String>,
}

pub fn output(results: &[MatchResult], _config: &Config) {
    let records: Vec<JsonRecord> = results
        .iter()
        .map(|r| {
            let model = r.record.details.as_ref().and_then(|d| d.model.clone());
            let loras = r.record.details.as_ref().map(|d| d.loras.clone()).unwrap_or_default();
            let positive_prompt = r.record.details.as_ref().and_then(|d| d.positive_prompt.clone());
            let negative_prompt = r.record.details.as_ref().and_then(|d| d.negative_prompt.clone());
            
            JsonRecord {
                path: r.record.path.to_string_lossy().into_owned(),
                generator: r.record.generator.to_string().to_lowercase(),
                score: r.score,
                model,
                loras,
                positive_prompt,
                negative_prompt,
            }
        })
        .collect();

    match serde_json::to_string_pretty(&records) {
        Ok(json) => println!("{}", json),
        Err(e) => eprintln!("ips: JSON serialization error: {}", e),
    }
}
