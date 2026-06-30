use serde::Serialize;
use crate::types::{Config, MatchResult, LoraInfo};

#[derive(Serialize)]
struct JsonRecord {
    path: String,
    #[serde(rename = "generator")]
    generator: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    score: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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
            let details = r.record.details_or_default();
            JsonRecord {
                path: r.record.path.to_string_lossy().into_owned(),
                generator: r.record.generator.to_string(),
                prompt: r.record.prompt.clone(),
                score: r.score,
                model: details.model,
                loras: details.loras,
                positive_prompt: details.positive_prompt,
                negative_prompt: details.negative_prompt,
            }
        })
        .collect();

    match serde_json::to_string_pretty(&records) {
        Ok(json) => println!("{}", json),
        Err(e) => eprintln!("ips: JSON serialization error: {}", e),
    }
}
