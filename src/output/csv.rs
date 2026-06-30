use crate::types::{Config, MatchResult};

pub fn output(results: &[MatchResult], _config: &Config) {
    let mut wtr = ::csv::Writer::from_writer(std::io::stdout());

    wtr.write_record([
        "path",
        "generator",
        "positive_prompt",
        "negative_prompt",
        "score",
    ])
    .ok();

    for result in results {
        let details = result.record.details_or_default();
        let score = result
            .score
            .map(|s| s.to_string())
            .unwrap_or_default();
        let positive = details.positive_prompt.unwrap_or_default();
        let negative = details.negative_prompt.unwrap_or_default();

        wtr.write_record([
            result.record.path.to_string_lossy().as_ref(),
            &result.record.generator.to_string(),
            &positive,
            &negative,
            &score,
        ])
        .ok();
    }

    wtr.flush().ok();
}
