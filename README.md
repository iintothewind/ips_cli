# ips — Image Prompt Search

A fast CLI tool to search AI-generated image prompts embedded in image metadata, from local files or http(s) URLs. Supports PNG, JPEG, and WebP files from Stable Diffusion (A1111/Forge), ComfyUI, NovelAI, and InvokeAI.

Extracts searchable prompt text plus structured fields (model, LoRAs, positive/negative prompts) when available.

ComfyUI conditioning references carry their **output slot**: slot 0 is the positive stream, slot ≥ 1 the negative one. A node that encodes both prompts in a single pass (`TextEncodeQwenImage21`) therefore reports its own `negative_prompt` instead of duplicating the positive prompt.

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
ips [OPTIONS] [PATH]
ips [OPTIONS] --url <URL>
ips [OPTIONS] --url-list <FILE>
```

Exactly one input: `PATH` (directory or image file, default `.`), `--url`, or `--url-list`. Without `-q`, every image that has prompt metadata is output.

### Options

| Flag | Description |
|---|---|
| `-q, --query <QUERY>` | Search query; omit to output every record |
| `--url <URL>` | Read one remote image over http(s) |
| `--url-list <FILE>` | Read remote images from FILE, one URL per line (LF/CRLF, `-` = stdin) |
| `-f, --format <FORMAT>` | Output format: `console`, `json`, `csv` (default: `console`) |
| `--fuzzy` | Fuzzy matching instead of exact substring |
| `--regex` | Regex matching |
| `--min-score <N>` | Minimum fuzzy match score (default: `50`) |
| `--full` | Show full prompt text (no truncation) with structured fields (model, LoRAs, prompts) |
| `-p, --path-only` | Print only matching file paths (no prompt text) |
| `--depth <N>` | Max directory recursion depth |
| `--no-recursive` | Only search the top-level directory |
| `-j, --threads <N>` | Number of worker threads (also concurrent downloads) |
| `-v, --verbose` | Log skipped/corrupt files to stderr |
| `--no-color` | Disable ANSI color (automatic when `NO_COLOR` is set or stdout is not a terminal) |

### Remote images

The format is detected from magic bytes, so URLs need no file extension. PNG and JPEG downloads stop at the image data; WebP is read through to its metadata. Each URL has a 30 s total timeout and a 64 MiB body limit; there are no retries. The URL is reported as `path` exactly as given. Duplicate URLs in a list are fetched once.

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Completed (zero matches is not an error) |
| `2` | Usage or input error: bad flag, invalid regex, unreadable `PATH` or URL list |
| `3` | Completed, but at least one URL failed; stderr has one `ips: <url>: <reason>` line per failure, and stdout still holds the results |

### Examples

```bash
# Search current directory
ips -q "cyberpunk"

# Fuzzy search in a specific directory
ips -q "sunset landscape" --fuzzy ./ai_art

# Structured console output (model, loras, prompts)
ips -q "masterpiece" --full ./images

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

# Dump all metadata of one file as JSON (no query)
ips -f json image.png

# Remote image and URL list
ips -f json --url https://host/a.png
ips -q "1girl" --url-list urls.txt
cat urls.txt | ips -q "1girl" --url-list -
```

## Supported Generators

| Generator | Format | How prompts are stored |
|---|---|---|
| Stable Diffusion A1111 / Forge | PNG | `tEXt`/`iTXt` chunk, keyword `parameters` |
| ComfyUI | PNG | `tEXt`/`iTXt` chunks, keywords `prompt` (API JSON) and `workflow` (UI JSON) |
| NovelAI | PNG | `tEXt`/`iTXt` chunk, keyword `Comment` (JSON with `"prompt"` field) |
| NovelAI (alternate) | PNG | `tEXt`/`iTXt` chunk, keyword `Description` |
| A1111 / others | JPEG | `COM` marker |
| Various | JPEG / WebP | XMP `dc:description` field |
| A1111 / Forge / ComfyUI savers | JPEG / WebP | EXIF `UserComment`: A1111 parameters text, or ComfyUI JSON `{"prompt", "workflow"}` (e.g. ComfyUI-JPG-Metadata) |
| ComfyUI (Save Animated WEBP) | WebP | EXIF IFD0 tags: Model `prompt:{…}`, Make `workflow:{…}` |
| InvokeAI | JPEG / WebP | XMP with `invokeai:` namespace |

A single PNG may contain both A1111 `parameters` and ComfyUI `prompt` chunks — ips emits one record per metadata source, so the same file can appear twice in results.

JSON cut off before its end — typical for a large ComfyUI graph in JPEG EXIF, which is limited to one 64 KB segment — is read up to its last complete value. Python's `NaN` / `Infinity` values in ComfyUI JSON are read as null.

For detailed extraction rules, JSON fallback behavior, and fault-tolerance, see [`ips_design_doc.md`](ips_design_doc.md).

## Output Formats

### Console (default)

Highlights matched text in context with the file path and detected generator:

```
./assets/cyberpunk_city.png [a1111]
   ...a detailed photo of a [cyberpunk] city at night, neon lights...
```

Long prompts are truncated to a ±80 character window around the match unless `--full` is set.

### Structured console (`--full`)

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

`tests/fixtures/metadata/` holds one JSON file per image: the metadata entries of a real image (`origin` names the source object; only SFW samples) or a hand-written case for a path no available image exercises (`origin: "synthetic"`), plus the records ips must extract. `tests/metadata_fixtures.rs` rebuilds each into a minimal PNG/JPEG/WebP and checks the records; its header documents the entry format.
