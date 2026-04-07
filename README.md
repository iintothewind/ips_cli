# ips — Image Prompt Search

A fast CLI tool to search AI-generated image prompts embedded in local image metadata. Supports PNG, JPEG, and WebP files from Stable Diffusion (A1111/Forge), ComfyUI, NovelAI, and InvokeAI.

## Installation

```bash
cargo install --path .
```

Or build a release binary:

```bash
cargo build --release
# binary at: target/release/ips
```

## Usage

```
ips [OPTIONS] --query <QUERY> [PATH]
```

`PATH` defaults to the current directory.

### Options

| Flag | Description |
|---|---|
| `-q, --query <QUERY>` | Search query (required) |
| `-f, --format <FORMAT>` | Output format: `console`, `json`, `csv` (default: `console`) |
| `--fuzzy` | Fuzzy matching instead of exact substring |
| `--regex` | Regex matching |
| `--min-score <N>` | Minimum fuzzy match score (default: `50`) |
| `--full` | Show full prompt without truncation |
| `--depth <N>` | Max directory recursion depth |
| `--no-recursive` | Only search the top-level directory |
| `-j, --threads <N>` | Number of worker threads |
| `-v, --verbose` | Log skipped/corrupt files to stderr |
| `--no-color` | Disable ANSI color (also respects `NO_COLOR` env var) |

### Examples

```bash
# Search current directory
ips -q "cyberpunk"

# Fuzzy search in a specific directory
ips -q "sunset landscape" --fuzzy ./ai_art

# Export matches to JSON
ips -q "masterpiece" -f json ./images > results.json

# Export to CSV with full prompts
ips -q "1girl" -f csv --full /volumes/ai_art > results.csv

# Regex search
ips -q "negative_prompt:.*blur" --regex ./outputs

# Top-level only, verbose
ips -q "portrait" --no-recursive -v ./downloads
```

## Supported Generators

| Generator | Format | How prompts are stored |
|---|---|---|
| Stable Diffusion A1111 / Forge | PNG | `tEXt` chunk, keyword `parameters` |
| ComfyUI | PNG | `tEXt` chunk, keyword `prompt` (workflow JSON) |
| NovelAI | PNG | `tEXt` chunk, keyword `Comment` (JSON with `"prompt"` field) |
| NovelAI (alternate) | PNG | `tEXt` chunk, keyword `Description` |
| A1111 / others | JPEG | `COM` marker |
| Various | JPEG / WebP | XMP `dc:description` field |
| InvokeAI | JPEG / WebP | XMP with `invokeai:` namespace |

## Output Formats

### Console (default)

Highlights matched text in context with the file path and detected generator:

```
./assets/cyberpunk_city.png [a1111]
   ...a detailed photo of a cyberpunk city at night, neon lights...
```

### JSON (`-f json`)

```json
[
  {
    "path": "./assets/cyberpunk_city.png",
    "generator": "a1111",
    "prompt": "a detailed photo of a cyberpunk city at night, neon lights...",
    "score": 120
  }
]
```

`score` is only present in fuzzy mode.

### CSV (`-f csv`)

```
path,generator,prompt,score
./assets/cyberpunk_city.png,a1111,"a detailed photo of a cyberpunk city...",
```

## Performance

- Reads only image metadata headers — never decodes pixel data.
- All extraction and matching runs in parallel via `rayon`.
- Target: 10,000 local images searched in under 3 seconds on an SSD.

## Development

```bash
cargo test       # run all tests
cargo build      # debug build
cargo build --release  # optimized release build
```

Test fixtures for each generator can be placed in `tests/fixtures/`.
