use crate::types::{Config, MatchResult};

pub fn output(results: &[MatchResult], _config: &Config) {
    let mut wtr = ::csv::Writer::from_writer(std::io::stdout());

    // Header row
    wtr.write_record(["path", "generator", "prompt", "score"])
        .ok();

    for result in results {
        let score = result
            .score
            .map(|s| s.to_string())
            .unwrap_or_default();

        wtr.write_record([
            result.record.path.to_string_lossy().as_ref(),
            &result.record.generator.to_string(),
            &result.record.prompt,
            &score,
        ])
        .ok();
    }

    wtr.flush().ok();
}
