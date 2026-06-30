pub mod console;
pub mod csv;
pub mod json;

use crate::types::{Config, MatchResult, StructuredPromptRecord, OutputFormat};

pub fn output_results(results: &[MatchResult], config: &Config) {
    match config.format {
        OutputFormat::Console => {
            // If --full is set, use structured output
            if config.full {
                let structured_results: Vec<StructuredPromptRecord> = results.iter().map(|r| {
                    // Convert MatchResult to StructuredPromptRecord
                    let record = &r.record;
                    let (model, loras, positive, negative) = crate::extract::parse_prompt_components(&record.prompt);
                    StructuredPromptRecord {
                        path: record.path.clone(),
                        generator: record.generator.clone(),
                        model,
                        loras: loras,
                        positive,
                        negative,
                    }
                }).collect();
                console::output_structured(&structured_results, config)
            } else {
                console::output(results, config)
            }
        }
        OutputFormat::Json => json::output(results, config),
        OutputFormat::Csv => csv::output(results, config),
    }
}
