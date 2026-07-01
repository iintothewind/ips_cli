use std::cell::RefCell;

use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use crate::types::{Config, MatchMode, MatchResult, PromptRecord};

thread_local! {
    static SKIM: SkimMatcherV2 = SkimMatcherV2::default();
    static REGEX_CACHE: RefCell<Option<(String, regex::Regex)>> = RefCell::new(None);
}

pub fn match_record(record: &PromptRecord, config: &Config) -> Option<MatchResult> {
    match config.match_mode {
        MatchMode::Exact => match_exact(record, config),
        MatchMode::Fuzzy => match_fuzzy(record, config),
        MatchMode::Regex => match_regex(record, config),
    }
}

fn match_exact(record: &PromptRecord, config: &Config) -> Option<MatchResult> {
    let query_chars: Vec<char> = config.query.chars().collect();
    if query_chars.is_empty() {
        return None;
    }

    let prompt_chars: Vec<(usize, char)> = record.prompt.char_indices().collect();

    for window in prompt_chars.windows(query_chars.len()) {
        let matches = window
            .iter()
            .zip(query_chars.iter())
            .all(|(&(_, prompt_ch), &query_ch)| chars_eq_ignore_case(prompt_ch, query_ch));

        if matches {
            let start = window[0].0;
            let (end_byte, last_ch) = window[window.len() - 1];
            return Some(MatchResult {
                record: record.clone(),
                score: None,
                match_ranges: vec![(start, end_byte + last_ch.len_utf8())],
            });
        }
    }

    None
}

fn chars_eq_ignore_case(a: char, b: char) -> bool {
    if a == b {
        return true;
    }
    a.to_lowercase().eq(b.to_lowercase())
}

fn match_fuzzy(record: &PromptRecord, config: &Config) -> Option<MatchResult> {
    SKIM.with(|matcher| {
        let (score, char_indices) = matcher.fuzzy_indices(&record.prompt, &config.query)?;

        if score < config.min_score {
            return None;
        }

        let prompt_chars: Vec<(usize, char)> = record.prompt.char_indices().collect();
        let match_ranges: Vec<(usize, usize)> = char_indices
            .iter()
            .filter_map(|&ci| {
                prompt_chars.get(ci).map(|&(byte_start, ch)| {
                    (byte_start, byte_start + ch.len_utf8())
                })
            })
            .collect();

        Some(MatchResult {
            record: record.clone(),
            score: Some(score),
            match_ranges,
        })
    })
}

fn match_regex(record: &PromptRecord, config: &Config) -> Option<MatchResult> {
    REGEX_CACHE.with(|cache| {
        let mut c = cache.borrow_mut();
        if c.as_ref().map_or(true, |(q, _)| q != &config.query) {
            let re = regex::Regex::new(&config.query).ok()?;
            *c = Some((config.query.clone(), re));
        }
        let re = &c.as_ref()?.1;
        re.find(&record.prompt).map(|m| MatchResult {
            record: record.clone(),
            score: None,
            match_ranges: vec![(m.start(), m.end())],
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::types::{Generator, OutputFormat};

    fn make_record(prompt: &str) -> PromptRecord {
        PromptRecord::with_details(
            PathBuf::from("test.png"),
            prompt.to_string(),
            Generator::Unknown,
            "parameters",
            Default::default(),
        )
    }

    fn config(query: &str, mode: MatchMode) -> Config {
        Config {
            query: query.to_string(),
            path: PathBuf::from("."),
            format: OutputFormat::Console,
            match_mode: mode,
            min_score: 50,
            full: false,
            depth: None,
            no_recursive: false,
            threads: None,
            verbose: false,
            no_color: true,
            path_only: false,
        }
    }

    #[test]
    fn exact_match_found() {
        let record = make_record("masterpiece, 1girl, cyberpunk");
        let result = match_exact(&record, &config("cyberpunk", MatchMode::Exact));
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.match_ranges, vec![(20, 29)]);
    }

    #[test]
    fn exact_match_case_insensitive() {
        let record = make_record("Masterpiece, 1girl");
        let result = match_exact(&record, &config("masterpiece", MatchMode::Exact));
        assert!(result.is_some());
    }

    #[test]
    fn exact_match_unicode() {
        let record = make_record("杰作 masterpiece 测试");
        let result = match_exact(&record, &config("杰作", MatchMode::Exact));
        assert!(result.is_some());
        let r = result.unwrap();
        let matched = &record.prompt[r.match_ranges[0].0..r.match_ranges[0].1];
        assert_eq!(matched, "杰作");
    }

    #[test]
    fn exact_match_not_found() {
        let record = make_record("landscape, mountains");
        let result = match_exact(&record, &config("cyberpunk", MatchMode::Exact));
        assert!(result.is_none());
    }

    #[test]
    fn fuzzy_match_found() {
        let record = make_record("a beautiful cyberpunk cityscape");
        let result = match_fuzzy(&record, &config("cyber", MatchMode::Fuzzy));
        assert!(result.is_some());
    }

    #[test]
    fn regex_match_found() {
        let record = make_record("1girl, masterpiece, detailed background");
        let result = match_regex(&record, &config(r"\d+girl", MatchMode::Regex));
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.match_ranges[0], (0, 5));
    }

    #[test]
    fn regex_no_match() {
        let record = make_record("landscape, trees");
        let result = match_regex(&record, &config(r"\d+girl", MatchMode::Regex));
        assert!(result.is_none());
    }
}
