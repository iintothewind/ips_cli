use clap::Parser;
use ips::types::{Config, MatchMode, MatchResult, OutputFormat, PromptRecord};
use ips::{discovery, extract, matcher, output, remote};
use rayon::prelude::*;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Bad flag, invalid regex, unreadable PATH or URL list. Same code clap uses for usage errors.
const EXIT_INPUT_ERROR: u8 = 2;
/// The run completed but at least one URL could not be read.
const EXIT_URL_FAILED: u8 = 3;

const AFTER_HELP: &str = "\
Examples:
  ips -q cyberpunk ./outputs               search a directory
  ips -f json image.png                    no query: dump the metadata of one file
  ips -f json --url https://host/a.png     read one remote image
  ips -q 1girl --url-list urls.txt         one URL per line; `--url-list -` reads stdin

Exit codes:
  0  completed (zero matches is not an error)
  2  usage or input error (bad flag, invalid regex, unreadable PATH or URL list)
  3  completed, but at least one URL could not be read; stderr has one
     `ips: <url>: <reason>` line per failure

JSON output (-f json) is an array of objects with the fields
  path, generator, score?, model?, loras?[{name, weight}], positive_prompt?, negative_prompt?
`path` is the file path, or the URL exactly as given. Fields marked ? are omitted when empty.";

#[derive(Parser, Debug)]
#[command(
    name = "ips",
    about = "Search AI-generated image prompts embedded in image metadata (local files or http(s) URLs)",
    version,
    after_help = AFTER_HELP
)]
struct Args {
    /// Directory or image file to search [default: .]
    #[arg(conflicts_with_all = ["url", "url_list"])]
    path: Option<PathBuf>,

    /// Search query; omit to output every image that has prompt metadata
    #[arg(short = 'q', long)]
    query: Option<String>,

    /// Read one remote image from an http(s) URL instead of PATH
    #[arg(long, value_name = "URL", value_parser = parse_url, conflicts_with = "url_list")]
    url: Option<String>,

    /// Read remote images from the URLs in FILE, one per line (`-` = stdin), instead of PATH
    #[arg(long, value_name = "FILE")]
    url_list: Option<PathBuf>,

    /// Output format
    #[arg(short = 'f', long, value_enum, default_value_t = OutputFormat::Console)]
    format: OutputFormat,

    /// Enable fuzzy matching (default is exact substring)
    #[arg(long, requires = "query")]
    fuzzy: bool,

    /// Enable regex matching
    #[arg(long, requires = "query")]
    regex: bool,

    /// Minimum fuzzy match score
    #[arg(long, default_value_t = 50)]
    min_score: i64,

    /// Do not truncate long prompt text in console output; also enables structured mode
    #[arg(long)]
    full: bool,

    /// Maximum directory recursion depth
    #[arg(long)]
    depth: Option<usize>,

    /// Disable recursive directory traversal
    #[arg(long)]
    no_recursive: bool,

    /// Number of worker threads (also the number of concurrent downloads)
    #[arg(short = 'j', long)]
    threads: Option<usize>,

    /// Print skipped files and errors to stderr
    #[arg(short = 'v', long)]
    verbose: bool,

    /// Disable ANSI color output (automatic when NO_COLOR is set or stdout is not a terminal)
    #[arg(long)]
    no_color: bool,

    /// Path only: print file paths without prompt text (console mode)
    #[arg(short = 'p', long)]
    path_only: bool,
}

fn parse_url(value: &str) -> Result<String, String> {
    if remote::is_url(value) {
        Ok(value.to_string())
    } else {
        Err("expected an http:// or https:// URL".to_string())
    }
}

fn main() -> ExitCode {
    let args = Args::parse();

    let match_mode = match (&args.query, args.regex, args.fuzzy) {
        (None, _, _) => MatchMode::All,
        (Some(_), true, _) => MatchMode::Regex,
        (Some(_), false, true) => MatchMode::Fuzzy,
        (Some(_), false, false) => MatchMode::Exact,
    };
    let query = args.query.unwrap_or_default();

    if match_mode == MatchMode::Regex {
        if let Err(e) = regex::Regex::new(&query) {
            eprintln!("ips: invalid regex pattern: {}", e);
            return ExitCode::from(EXIT_INPUT_ERROR);
        }
    }

    let no_color = args.no_color
        || std::env::var("NO_COLOR").is_ok()
        || !std::io::stdout().is_terminal();

    // --full enables structured output and full prompt display (no truncation)
    let config = Config {
        query,
        path: args.path.unwrap_or_else(|| PathBuf::from(".")),
        format: args.format,
        match_mode,
        min_score: args.min_score,
        full: args.full,
        depth: args.depth,
        no_recursive: args.no_recursive,
        threads: args.threads,
        verbose: args.verbose,
        no_color,
        path_only: args.path_only,
    };

    if let Some(n) = config.threads {
        if let Err(e) = rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()
        {
            eprintln!("ips: could not set thread count to {}: {}", n, e);
        }
    }

    // (results, failed URL inputs, total URL inputs)
    let (mut results, failed, total) = if let Some(url) = &args.url {
        let (results, failed) = search_urls(std::slice::from_ref(url), &config);
        (results, failed, 1)
    } else if let Some(list_path) = &args.url_list {
        let text = match read_url_list(list_path) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("ips: cannot read URL list {}: {}", list_path.display(), e);
                return ExitCode::from(EXIT_INPUT_ERROR);
            }
        };
        let list = remote::parse_url_list(&text);
        for (line, text) in &list.invalid {
            eprintln!("ips: {}:{}: not an http(s) URL: {}", list_path.display(), line, text);
        }
        let (results, failed) = search_urls(&list.urls, &config);
        (results, failed + list.invalid.len(), list.urls.len() + list.invalid.len())
    } else {
        match search_local(&config) {
            Ok(results) => (results, 0, 0),
            Err(reason) => {
                eprintln!("ips: {}", reason);
                return ExitCode::from(EXIT_INPUT_ERROR);
            }
        }
    };

    results.sort_by(|a, b| {
        a.record
            .path
            .cmp(&b.record.path)
            .then(a.record.metadata_key.cmp(&b.record.metadata_key))
    });

    if results.is_empty() && config.format == OutputFormat::Console {
        eprintln!("No matches found.");
    }

    output::output_results(&results, &config);

    if failed > 0 {
        eprintln!("ips: {} of {} URL(s) failed", failed, total);
        return ExitCode::from(EXIT_URL_FAILED);
    }
    ExitCode::SUCCESS
}

fn read_url_list(path: &Path) -> std::io::Result<String> {
    if path == Path::new("-") {
        std::io::read_to_string(std::io::stdin())
    } else {
        std::fs::read_to_string(path)
    }
}

fn matched(records: Vec<PromptRecord>, config: &Config) -> Vec<MatchResult> {
    records
        .iter()
        .filter_map(|record| matcher::match_record(record, config))
        .collect()
}

fn search_local(config: &Config) -> Result<Vec<MatchResult>, String> {
    if config.path.to_str().is_some_and(remote::is_url) {
        return Err(format!("{} is a URL; pass it with --url", config.path.display()));
    }
    if let Err(e) = std::fs::metadata(&config.path) {
        return Err(format!("cannot read {}: {}", config.path.display(), e));
    }

    let files = discovery::discover_files(config);
    if config.verbose {
        eprintln!("ips: found {} image file(s) to scan", files.len());
    }

    Ok(files
        .par_iter()
        .flat_map(|path| matched(extract::extract_prompt(path, config.verbose), config))
        .collect())
}

fn search_urls(urls: &[String], config: &Config) -> (Vec<MatchResult>, usize) {
    let fetcher = remote::Fetcher::new();
    let failed = AtomicUsize::new(0);
    let results = urls
        .par_iter()
        .flat_map(|url| match fetcher.extract(url, config.verbose) {
            Ok(records) => matched(records, config),
            Err(reason) => {
                eprintln!("ips: {}: {}", url, reason);
                failed.fetch_add(1, Ordering::Relaxed);
                Vec::new()
            }
        })
        .collect();
    (results, failed.into_inner())
}
