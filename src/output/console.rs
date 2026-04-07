use std::io::Write;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};
use crate::types::{Config, MatchResult};

pub fn output(results: &[MatchResult], config: &Config) {
    let color_choice = if config.no_color {
        ColorChoice::Never
    } else {
        ColorChoice::Auto
    };

    let mut stdout = StandardStream::stdout(color_choice);

    for result in results {
        print_result(&mut stdout, result, config);
    }
}

fn print_result(stdout: &mut StandardStream, result: &MatchResult, config: &Config) {
    // File path — bold cyan, same highlight colour used for matched text
    stdout
        .set_color(ColorSpec::new().set_bold(true).set_fg(Some(Color::Cyan)))
        .ok();
    write!(stdout, "{}", result.record.path.display()).ok();

    // Generator tag (dimmed)
    stdout
        .set_color(ColorSpec::new().set_dimmed(true))
        .ok();
    writeln!(stdout, " [{}]", result.record.generator).ok();
    stdout.reset().ok();

    // Compute what to display
    let (display_text, adj_ranges, prefix_dots, suffix_dots) =
        compute_display_window(&result.record.prompt, &result.match_ranges, config);

    write!(stdout, "   ").ok();

    if prefix_dots {
        stdout.set_color(ColorSpec::new().set_dimmed(true)).ok();
        write!(stdout, "...").ok();
        stdout.reset().ok();
    }

    print_highlighted(stdout, display_text, &adj_ranges);

    if suffix_dots {
        stdout.set_color(ColorSpec::new().set_dimmed(true)).ok();
        write!(stdout, "...").ok();
        stdout.reset().ok();
    }

    writeln!(stdout).ok();
    writeln!(stdout).ok(); // blank line between results
}

/// Returns (window_text, adjusted_ranges, prefix_ellipsis, suffix_ellipsis)
fn compute_display_window<'a>(
    prompt: &'a str,
    ranges: &[(usize, usize)],
    config: &Config,
) -> (&'a str, Vec<(usize, usize)>, bool, bool) {
    const CONTEXT: usize = 80;
    const MAX_PLAIN: usize = 500;

    if config.full || prompt.len() <= MAX_PLAIN {
        return (prompt, ranges.to_vec(), false, false);
    }

    let first_match = ranges.first().map(|&(s, _)| s).unwrap_or(0);
    let raw_start = first_match.saturating_sub(CONTEXT);
    let raw_end = (first_match + CONTEXT * 2).min(prompt.len());

    let window_start = floor_char_boundary(prompt, raw_start);
    let window_end = ceil_char_boundary(prompt, raw_end);

    let window = &prompt[window_start..window_end];
    let adj: Vec<(usize, usize)> = ranges
        .iter()
        .filter_map(|&(s, e)| {
            if s >= window_start && e <= window_end {
                Some((s - window_start, e - window_start))
            } else {
                None
            }
        })
        .collect();

    (window, adj, window_start > 0, window_end < prompt.len())
}

fn print_highlighted(stdout: &mut StandardStream, text: &str, ranges: &[(usize, usize)]) {
    if ranges.is_empty() {
        stdout.reset().ok();
        write!(stdout, "{}", text).ok();
        return;
    }

    let mut sorted = ranges.to_vec();
    sorted.sort_by_key(|&(s, _)| s);

    let mut cursor = 0usize;

    for (start, end) in sorted {
        // Clamp to valid byte boundaries within text
        let start = start.min(text.len());
        let end = end.min(text.len());

        if start > cursor {
            stdout.reset().ok();
            write!(stdout, "{}", &text[cursor..start]).ok();
        }

        if start < end {
            stdout
                .set_color(
                    ColorSpec::new()
                        .set_bold(true)
                        .set_fg(Some(Color::Yellow)),
                )
                .ok();
            write!(stdout, "{}", &text[start..end]).ok();
            stdout.reset().ok();
        }

        cursor = end;
    }

    if cursor < text.len() {
        stdout.reset().ok();
        write!(stdout, "{}", &text[cursor..]).ok();
    }
}

fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(s: &str, mut idx: usize) -> usize {
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx.min(s.len())
}
