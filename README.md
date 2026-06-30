# ips — Image Prompt Search

A fast CLI tool to search AI-generated image prompts embedded in local image metadata. Supports PNG, JPEG, and WebP files from Stable Diffusion (A1111/Forge), ComfyUI, NovelAI, and InvokeAI.

Extracts searchable prompt text plus structured fields (model, LoRAs, positive/negative prompts) when available.

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
| `--structured` | Show model, LoRAs, and positive/negative prompts in console mode |
| `--full` | Do not truncate long prompts; also enables structured console output |
| `-p, --path-only` | Print only matching file paths (no prompt text) |
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

# Structured console output (model, loras, prompts)
ips -q "masterpiece" --structured ./images

# List matching files only
ips -q "1girl" -p ./outputs

# Export matches to JSON (includes structured fields when present)
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
| Stable Diffusion A1111 / Forge | PNG | `tEXt`/`iTXt` chunk, keyword `parameters` |
| ComfyUI | PNG | `tEXt`/`iTXt` chunk, keyword `prompt` (workflow JSON) |
| NovelAI | PNG | `tEXt`/`iTXt` chunk, keyword `Comment` (JSON with `"prompt"` field) |
| NovelAI (alternate) | PNG | `tEXt`/`iTXt` chunk, keyword `Description` |
| A1111 / others | JPEG | `COM` marker |
| Various | JPEG / WebP | XMP `dc:description` field |
| Legacy / rare | JPEG / WebP | EXIF `UserComment` |
| InvokeAI | JPEG / WebP | XMP with `invokeai:` namespace |

A single PNG may contain both A1111 `parameters` and ComfyUI `prompt` chunks — ips emits one record per metadata source, so the same file can appear twice in results.

For detailed extraction rules, JSON fallback behavior, and fault-tolerance, see [`ips_design_doc.md`](ips_design_doc.md).

## Output Formats

### Console (default)

Highlights matched text in context with the file path and detected generator:

```
./assets/cyberpunk_city.png [a1111]
   ...a detailed photo of a [cyberpunk] city at night, neon lights...
```

Long prompts are truncated to a ±80 character window around the match unless `--full` is set.

### Structured console (`--structured` or `--full`)

Prints parsed metadata fields instead of the context window:

```
./assets/image.png

Generator: comfyui
Model: PlantMilkModelSuite_almond
LoRA: lora:style:0.7, lora:detail:0.5
Positive Prompt:
a beautiful sunset over mountains
Negative Prompt:
blurry, low quality
```

Use `-p` / `--path-only` to print only paths (works in structured mode too).

### JSON (`-f json`)

```json
[
  {
    "path": "./assets/cyberpunk_city.png",
    "generator": "a1111",
    "model": "realisticVision.safetensors",
    "loras": [{"name": "style", "weight": "0.7"}],
    "positive_prompt": "a detailed photo of a cyberpunk city...",
    "negative_prompt": "blurry, low quality",
    "score": 120
  }
]
```

- `positive_prompt` and `negative_prompt` are omitted when empty.
- `model`, `loras` are omitted when empty.
- `score` is only present in fuzzy mode.

### CSV (`-f csv`)

```
path,generator,positive_prompt,negative_prompt,score
./assets/cyberpunk_city.png,a1111,"a detailed photo of a cyberpunk city...","blurry, low quality",
```

`model` and `loras` are not included in CSV output (use JSON for those).

## Performance

- Reads only image metadata headers — never decodes pixel data.
- All extraction and matching runs in parallel via `rayon`.
- Target: 10,000 local images searched in under 3 seconds on an SSD.

## Development

```bash
cargo test              # run unit tests
cargo build             # debug build
cargo build --release   # optimized release build
```

Test image fixtures can be placed in `tests/examples/` for integration tests.
