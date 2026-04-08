mod discovery;
mod extract;
mod matcher;
mod output;
mod types;

use clap::Parser;
use rayon::prelude::*;
use std::path::PathBuf;
use types::{Config, MatchMode, OutputFormat};

#[derive(Parser, Debug)]
#[command(
    name = "ips",
    about = "Search AI-generated image prompts embedded in local image metadata",
    version
)]
struct Args {
    /// Directory to search
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Search query
    #[arg(short = 'q', long)]
    query: String,

    /// Output format: console, json, csv
    #[arg(short = 'f', long, default_value = "console")]
    format: String,

    /// Enable fuzzy matching (default is exact substring)
    #[arg(long)]
    fuzzy: bool,

    /// Enable regex matching
    #[arg(long)]
    regex: bool,

    /// Minimum fuzzy match score [default: 50]
    #[arg(long, default_value = "50")]
    min_score: i64,

    /// Show full prompt text in console mode (no truncation)
    #[arg(long)]
    full: bool,

    /// Maximum directory recursion depth
    #[arg(long)]
    depth: Option<usize>,

    /// Disable recursive directory traversal
    #[arg(long)]
    no_recursive: bool,

    /// Number of worker threads
    #[arg(short = 'j', long)]
    threads: Option<usize>,

    /// Print skipped files and errors to stderr
    #[arg(short = 'v', long)]
    verbose: bool,

    /// Disable ANSI color output (also respects NO_COLOR env var)
    #[arg(long)]
    no_color: bool,

    /// Path only: print file paths without prompt text (console mode)
    #[arg(short = 'p', long)]
    path_only: bool,
}

fn main() {
    let args = Args::parse();

    let format = match args.format.as_str() {
        "json" => OutputFormat::Json,
        "csv" => OutputFormat::Csv,
        _ => OutputFormat::Console,
    };

    let match_mode = if args.regex {
        MatchMode::Regex
    } else if args.fuzzy {
        MatchMode::Fuzzy
    } else {
        MatchMode::Exact
    };

    // Validate the regex pattern early so we can give a clear error message.
    if match_mode == MatchMode::Regex {
        if let Err(e) = regex::Regex::new(&args.query) {
            eprintln!("ips: invalid regex pattern: {}", e);
            std::process::exit(1);
        }
    }

    let no_color = args.no_color || std::env::var("NO_COLOR").is_ok();

    let config = Config {
        query: args.query,
        path: args.path,
        format,
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

    // Configure rayon thread pool if a specific count was requested.
    if let Some(n) = config.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()
            .ok();
    }

    // Phase 1: discover image files
    let files = discovery::discover_files(&config);

    if config.verbose {
        eprintln!("ips: found {} image file(s) to scan", files.len());
    }

    // Phase 2 & 3: extract prompts and match in parallel
    let mut results: Vec<_> = files
        .par_iter()
        .flat_map(|path| {
            let records = extract::extract_prompt(path, config.verbose);
            records
                .into_iter()
                .filter_map(|record| matcher::match_record(&record, &config))
                .collect::<Vec<_>>()
        })
        .collect();

    // Sort results by file path for deterministic output
    results.sort_by(|a, b| a.record.path.cmp(&b.record.path));

    if results.is_empty() && config.format == OutputFormat::Console {
        eprintln!("No matches found.");
    }

    // Phase 4: output
    output::output_results(&results, &config);
}
