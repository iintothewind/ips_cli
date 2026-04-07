pub mod console;
pub mod csv;
pub mod json;

use crate::types::{Config, MatchResult, OutputFormat};

pub fn output_results(results: &[MatchResult], config: &Config) {
    match config.format {
        OutputFormat::Console => console::output(results, config),
        OutputFormat::Json => json::output(results, config),
        OutputFormat::Csv => csv::output(results, config),
    }
}
