# Technical Design Document: `ips` (Image Prompt Search)

## 1. Project Identity

- **Name:** `ips` (Image Prompt Search)
- **Language:** Rust (2021 edition, MSRV 1.75+)
- **Goal:** A high-performance, cross-platform CLI tool to search AI-generated image prompts embedded in local image metadata (PNG, JPEG, WebP).
- **Scope:** Local directory search only for v1. Remote URL list support is deferred to a future version to keep the binary small and dependency tree lean.

---

## 2. Architecture Overview

Single-binary pipeline with four stages:

```
[Discovery] → [Extraction] → [Matching] → [Output]
```

1. **Discovery:** Recursive directory traversal filtered by image extension.
2. **Extraction:** Header-only binary inspection to extract AI prompt text from image metadata chunks. No pixel decoding.
3. **Matching:** Exact substring or fuzzy matching against extracted prompt text.
4. **Output:** Console (ANSI-highlighted), JSON, or CSV.

All stages are parallelized via work-stealing thread pool.

---

## 3. Technical Stack

| Concern | Crate | Notes |
|---|---|---|
| CLI framework | `clap` v4 | Derive API |
| Directory walking | `walkdir` | Handles symlink loop detection |
| Parallelism | `rayon` | Work-stealing parallel iterators |
| Fuzzy matching | `fuzzy-matcher` | Skim algorithm (`SkimMatcherV2`) |
| PNG chunk parsing | `img-parts` | Access to raw `tEXt`/`iTXt` chunks with keyword. If `img-parts` does not expose chunk keywords, fall back to manual PNG chunk parsing (see §4B fallback). |
| JPEG segment parsing | Manual (raw `std::io`) | Read SOI + APP/COM markers directly. No full EXIF decode needed. |
| WebP chunk parsing | `img-parts` | RIFF-based access to `EXIF` and `XMP ` chunks |
| JSON parsing (ComfyUI) | `serde_json` | Walk workflow JSON to extract text prompts from nodes |
| Serialization | `serde` + `serde_json` + `csv` | For `--format json` and `--format csv` output |
| Console coloring | `termcolor` or `anstream` | ANSI highlight for matched text; respects `NO_COLOR` env |

### Crates explicitly NOT used

- `image` / `image-rs` — Decodes pixels; far too heavy for metadata-only reads.
- `reqwest` / `tokio` — Async HTTP adds ~3MB to binary. Deferred to v2.
- `exif` — Standard EXIF tags (e.g. `UserComment`) do not contain AI prompts in most generators. We parse raw segments instead.

---

## 4. Metadata Extraction Rules

This is the most critical section. AI image generators embed prompts in non-standard locations. The extractor must support all major generators.

### A. PNG

PNG stores metadata in ancillary text chunks. Read chunks sequentially from the file header; stop after `IDAT` (image data starts, no more metadata).

| Chunk Type | Keyword | Generator | Format |
|---|---|---|---|
| `tEXt` | `parameters` | Stable Diffusion (A1111/Forge) | Plain text. Prompt is the value directly. |
| `tEXt` | `prompt` | ComfyUI | **JSON string** containing the full workflow graph. Must be parsed (see §4E). |
| `tEXt` | `workflow` | ComfyUI | Workflow JSON (secondary; `prompt` is the one with actual text prompts). |
| `tEXt` | `Comment` | NovelAI | **JSON string** with a `"prompt"` field inside. Parse JSON, extract `.prompt`. |
| `tEXt` | `Description` | NovelAI (alternate) | Plain text prompt. |
| `iTXt` | (same keys) | Same generators (UTF-8 variant) | Same extraction logic; `iTXt` adds language/translation fields to skip. |

**Implementation:**

```
for each chunk in png_chunks:
    if chunk.type in [tEXt, iTXt]:
        (keyword, value) = parse_text_chunk(chunk)
        if keyword in ["parameters", "prompt", "Comment", "Description"]:
            prompt_text = normalize(keyword, value)  // see §4E
            yield (file_path, keyword, prompt_text)
```

**Fallback:** If `img-parts` does not provide keyword-level access to `tEXt`/`iTXt` data, implement manual PNG chunk parsing:
- Validate 8-byte PNG signature.
- Read chunks: 4-byte length (big-endian) + 4-byte type + data + 4-byte CRC.
- For `tEXt`: keyword is null-terminated string at start of data; value is the rest.
- For `iTXt`: keyword is null-terminated, followed by compression flag, compression method, language tag (null-terminated), translated keyword (null-terminated), then the text value.

### B. JPEG

JPEG does **not** use the EXIF `UserComment` tag for AI prompts. Prompts are stored in:

| Segment | Marker | Generator | Format |
|---|---|---|---|
| COM (Comment) | `0xFFFE` | A1111, some others | Plain text. The raw comment string is the prompt. |
| APP1 (XMP) | `0xFFE1` | Various | XML; extract content of `<dc:description>` or generator-specific tags. |
| APP1 (EXIF with UserComment) | `0xFFE1` | Rare/legacy | Only as a last-resort fallback. |

**Implementation:**

```
read 2 bytes → assert SOI (0xFFD8)
loop:
    read marker (0xFF + type_byte)
    if marker == SOS (0xFFDA) → stop (image data starts)
    read 2-byte segment length (big-endian, includes length field)
    if marker == 0xFFFE (COM):
        prompt_text = read segment body as UTF-8
        yield prompt
    if marker == 0xFFE1 (APP1):
        if body starts with "http://ns.adobe.com/xap/1.0/\0":
            parse XMP XML → extract dc:description
            yield prompt
    else:
        skip segment
```

### C. WebP

WebP uses RIFF container format. Metadata is in dedicated chunks:

| RIFF Chunk | Content | Extraction |
|---|---|---|
| `EXIF` | Standard EXIF blob | Parse as JPEG APP1 EXIF; check `UserComment` as fallback. |
| `XMP ` (with trailing space) | XMP XML | Same XMP parsing as JPEG: extract `<dc:description>`. |

Use `img-parts` for RIFF chunk access. It provides `WebP::chunks()` iteration.

### D. Generator Detection (Best-Effort)

When extracting, tag the source generator if identifiable:

- PNG `parameters` key → `"a1111"`
- PNG `prompt` key with JSON object → `"comfyui"`
- PNG `Comment` key with JSON containing `"prompt"` field → `"novelai"`
- JPEG COM marker → `"a1111"` (heuristic)
- XMP with `<invokeai:...>` → `"invokeai"`
- Otherwise → `"unknown"`

This is informational only (for `--verbose` output). Does not affect matching.

### E. ComfyUI Workflow JSON Parsing

ComfyUI's `prompt` chunk contains a JSON object representing the workflow graph. Text prompts are buried inside node inputs. Extraction strategy:

```
parse JSON → for each node in values:
    if node.class_type in ["CLIPTextEncode", "CLIPTextEncodeSDXL", ...]:
        if node.inputs.text exists and is a string:
            collect node.inputs.text
    // Also check common custom node types:
    if node.class_type contains "PromptEncode" or "TextInput":
        scan node.inputs for string values
```

Concatenate all extracted text strings with `" | "` separator to form the searchable prompt.

**Known `class_type` values to scan:**
- `CLIPTextEncode`
- `CLIPTextEncodeSDXL` (has `text_g` and `text_l` fields)
- `CLIPTextEncodeFlux`
- Any `class_type` containing the substring `Prompt` or `TextEncode`

### F. NovelAI JSON Parsing

The `Comment` chunk contains a JSON string. Parse and extract the `"prompt"` field:

```json
{"prompt": "1girl, masterpiece, ...", "steps": 28, "sampler": "..."}
```

Extract the value of `"prompt"` as the searchable text.

---

## 5. Matching Engine

### Exact Mode (default)

Case-insensitive substring search using `str::to_lowercase()` + `str::contains()`.

Optimization: Pre-lowercase the query once. For each prompt, use a streaming lowercase comparison to avoid allocating a new lowercase string per file. If this is too complex, simple `to_lowercase().contains()` is acceptable for v1 — profile before optimizing.

### Fuzzy Mode (`--fuzzy`)

Use `fuzzy-matcher`'s `SkimMatcherV2`:
- Returns a score and matched index ranges.
- Filter results by a minimum score threshold (configurable via `--min-score`, default 50).
- Use matched index ranges to highlight the matching portions in console output.

### Regex Mode (`--regex`, stretch goal for v1)

Use the `regex` crate. Apply the compiled pattern against each prompt string. If implemented, highlight the full match span.

---

## 6. Output Modes

### Console (default)

```
📄 ./assets/cyberpunk_city.png [a1111]
   ...a detailed photo of a [cyberpunk] city at night, neon lights...

📄 ./assets/scifi/robot.png [comfyui]
   ...futuristic [cyber] punk robot, metallic skin...
```

Rules:
- File path in bold/bright.
- Generator tag in brackets (dimmed).
- Prompt text shows a **context window** of ±80 characters around the first match. Matched text is highlighted (bold + color).
- If `--full` flag is set, print the entire prompt without truncation.
- Long prompts (>500 chars) are truncated with `...` on both ends unless `--full` is specified.

### JSON (`--format json`)

```json
[
  {
    "path": "./assets/cyberpunk_city.png",
    "generator": "a1111",
    "prompt": "a detailed photo of a cyberpunk city at night, neon lights...",
    "score": 100
  }
]
```

- `score` is included only in fuzzy mode; omitted (or `null`) in exact mode.
- Output to stdout. Pipe-friendly.

### CSV (`--format csv`)

```
path,generator,prompt,score
./assets/cyberpunk_city.png,a1111,"a detailed photo of a cyberpunk city...",100
```

- Prompts containing commas or quotes are properly escaped per RFC 4180.
- Output to stdout. Pipe-friendly.

---

## 7. CLI Interface

```
ips [OPTIONS] --query <QUERY> [PATH]

Arguments:
  [PATH]    Directory to search [default: .]

Options:
  -q, --query <QUERY>       Search query (required)
  -f, --format <FORMAT>     Output format: console, json, csv [default: console]
      --fuzzy               Enable fuzzy matching (default is exact substring)
      --regex               Enable regex matching
      --min-score <N>       Minimum fuzzy match score, 0-100 [default: 50]
      --full                Show full prompt text in console mode (no truncation)
      --depth <N>           Maximum directory recursion depth [default: unlimited]
      --no-recursive        Disable recursive directory traversal
  -j, --threads <N>         Number of threads [default: num_cpus]
  -v, --verbose             Print skipped files and errors to stderr
      --no-color            Disable ANSI color output (also respects NO_COLOR env)
  -h, --help                Print help
  -V, --version             Print version
```

### Usage Examples

```bash
# Basic search in current directory
ips -q "cyberpunk"

# Fuzzy search in a specific directory
ips -q "sunset landscape" --fuzzy ./ai_art

# Export to JSON
ips -q "masterpiece" -f json ./images > results.json

# Export to CSV with full prompts
ips -q "1girl" -f csv --full /volumes/ai_art > results.csv

# Regex search for negative prompts
ips -q "negative_prompt:.*blur" --regex ./outputs

# Shallow search, top-level only
ips -q "portrait" --no-recursive ./downloads

# Verbose mode to see skipped/corrupt files
ips -q "anime" -v ./mixed_files
```

---

## 8. Error Handling Strategy

| Situation | Behavior |
|---|---|
| File permission denied | Skip file. Log to stderr if `--verbose`. |
| Corrupt/truncated image | Skip file. Log to stderr if `--verbose`. |
| Unsupported format (e.g. `.gif`, `.bmp`) | Skip silently (not an error). |
| Symlink loop | `walkdir` detects and skips by default. |
| Non-UTF-8 metadata | Attempt lossy UTF-8 conversion (`String::from_utf8_lossy`). |
| Empty directory / no matches | Exit with code 0, empty output. Print "No matches found." to stderr in console mode. |
| Invalid query regex | Exit with code 1 and a clear error message. |

General principle: **Never crash on bad input.** Log and skip individual files; only fatal errors (bad CLI args, invalid regex) cause a non-zero exit.

---

## 9. Project Structure

```
ips/
├── Cargo.toml
├── src/
│   ├── main.rs              # CLI entry point, argument parsing
│   ├── discovery.rs          # Directory walking, extension filtering
│   ├── extract/
│   │   ├── mod.rs            # Public extract API: fn extract_prompt(path) -> Vec<PromptRecord>
│   │   ├── png.rs            # PNG tEXt/iTXt chunk parsing
│   │   ├── jpeg.rs           # JPEG COM marker and XMP parsing
│   │   ├── webp.rs           # WebP RIFF chunk parsing
│   │   └── comfyui.rs        # ComfyUI workflow JSON prompt extraction
│   ├── matcher.rs            # Exact, fuzzy, and regex matching
│   ├── output/
│   │   ├── mod.rs            # Output dispatcher
│   │   ├── console.rs        # ANSI-highlighted console output
│   │   ├── json.rs           # JSON serialization
│   │   └── csv.rs            # CSV serialization
│   └── types.rs              # Shared types: PromptRecord, MatchResult, Config
└── tests/
    ├── fixtures/             # Test images from each generator (A1111, ComfyUI, NAI)
    └── integration_tests.rs
```

---

## 10. Data Types

```rust
/// A prompt extracted from a single image file.
pub struct PromptRecord {
    pub path: PathBuf,
    pub prompt: String,
    pub generator: Generator,
    pub metadata_key: String,  // e.g. "parameters", "prompt", "COM"
}

pub enum Generator {
    A1111,
    ComfyUI,
    NovelAI,
    InvokeAI,
    Unknown,
}

/// A matched result ready for output.
pub struct MatchResult {
    pub record: PromptRecord,
    pub score: Option<i64>,           // fuzzy score, None for exact match
    pub match_ranges: Vec<(usize, usize)>,  // byte ranges of matched text for highlighting
}
```

---

## 11. Development Phases

### Phase 1: Scaffolding & Discovery
- Set up Cargo project with all dependencies.
- Implement CLI argument parsing with `clap` derive.
- Implement `walkdir`-based directory traversal with extension filtering (`.png`, `.jpg`, `.jpeg`, `.webp`).
- Write tests for discovery with nested directory structures.

### Phase 2: PNG Metadata Extraction
- Implement PNG `tEXt`/`iTXt` chunk parsing.
- Support `parameters` (A1111), `prompt` (ComfyUI), `Comment` (NovelAI), `Description` keys.
- Implement ComfyUI workflow JSON parsing to extract text prompts from nodes.
- Implement NovelAI JSON parsing for the `Comment` key.
- Write unit tests with real PNG test fixtures from each generator.

### Phase 3: JPEG & WebP Extraction
- Implement JPEG raw segment reader (COM marker + XMP from APP1).
- Implement WebP RIFF chunk reader (EXIF + XMP chunks via `img-parts`).
- Write unit tests with JPEG and WebP fixtures.

### Phase 4: Matching Engine
- Implement exact substring matching (case-insensitive).
- Integrate `fuzzy-matcher` `SkimMatcherV2` for `--fuzzy` mode.
- Integrate `rayon` parallel iteration: discovery → parallel extraction + matching.
- Wire matching results to highlight ranges.

### Phase 5: Output Formatting
- Implement console output with ANSI highlighting and context windowing.
- Implement JSON output via `serde_json`.
- Implement CSV output via `csv` crate.
- Respect `--full`, `--no-color`, `NO_COLOR` env.

### Phase 6: Polish
- Error handling audit: ensure no panics on corrupt files.
- Add `--verbose` logging to stderr.
- Add `--depth` and `--no-recursive` support.
- Write integration tests (end-to-end: directory → search → verify output).
- Cross-platform testing (Linux, macOS, Windows).

---

## 12. Performance Targets

| Metric | Target |
|---|---|
| 10,000 local images (SSD) | Search + match < 3 seconds |
| Peak memory | < 100 MB for 10k results buffered |
| Binary size (release, stripped) | < 5 MB |

Key performance techniques:
- Header-only reading: never read past the metadata section of any file.
- `rayon` parallel iteration for CPU-bound extraction and matching.
- Streaming output in console mode (print results as found, don't buffer all).
- For JSON/CSV: buffer results in a `Vec<MatchResult>` then serialize once at the end.

---

## 13. Future Work (v2)

- `--url-list <file>`: Download images from a URL list and search metadata. Requires `reqwest` + `tokio`. Will be gated behind a cargo feature flag to keep the default binary lean.
- `--watch` mode: Watch a directory for new images and search incrementally.
- Index/cache: Build a local SQLite index of extracted prompts for instant repeat searches.
- Additional formats: AVIF, TIFF, PSD metadata extraction.
- GUI: Optional TUI (terminal UI) with interactive fuzzy search using `ratatui`.
