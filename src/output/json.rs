use serde::Serialize;
use crate::types::{Config, MatchResult};

#[derive(Serialize)]
struct JsonRecord {
    path: String,
    generator: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    score: Option<i64>,
}

pub fn output(results: &[MatchResult], _config: &Config) {
    let records: Vec<JsonRecord> = results
        .iter()
        .map(|r| JsonRecord {
            path: r.record.path.to_string_lossy().into_owned(),
            generator: r.record.generator.to_string(),
            prompt: r.record.prompt.clone(),
            score: r.score,
        })
        .collect();

    match serde_json::to_string_pretty(&records) {
        Ok(json) => println!("{}", json),
        Err(e) => eprintln!("ips: JSON serialization error: {}", e),
    }
}
