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
| PNG chunk parsing | Manual (`std::io`) | Sequential chunk read; stop at `IDAT`. No CRC validation. |
| JPEG segment parsing | Manual (`std::io`) | Read SOI + APP/COM markers directly. No full EXIF decode library. |
| WebP chunk parsing | Manual (`std::io` + `seek`) | RIFF header; skip VP8/VP8L image chunks without loading. |
| JSON parsing (ComfyUI) | `serde_json` | Walk workflow JSON to extract text prompts from nodes |
| Serialization | `serde` + `serde_json` + `csv` | For `--format json` and `--format csv` output |
| Console coloring | `termcolor` or `anstream` | ANSI highlight for matched text; respects `NO_COLOR` env |

### Crates explicitly NOT used

- `image` / `image-rs` — Decodes pixels; far too heavy for metadata-only reads.
- `reqwest` / `tokio` — Async HTTP adds ~3MB to binary. Deferred to v2.
- `exif` — Standard EXIF tags (e.g. `UserComment`) do not contain AI prompts in most generators. We parse raw segments instead.

---

## 4. Metadata Extraction Rules

This is the most critical section. AI image generators embed prompts in non-standard locations. The extractor must support all major generators **and degrade gracefully** when metadata is missing, truncated, or malformed.

### 4.0 Design Principles (Fault Tolerance)

All extraction code follows a layered, never-panic strategy:

1. **File-level:** Open/signature failure → return empty `Vec`; optional `eprintln!` when `--verbose`.
2. **Container-level:** Truncated chunk/segment → stop reading, return whatever was collected so far.
3. **Text-level:** Non-UTF-8 bytes → `String::from_utf8_lossy` (replacement characters, no abort).
4. **JSON-level:** `serde_json::from_str` is all-or-nothing. There is **no partial JSON repair**. On failure, fall back to plain-text A1111 parsing where applicable (see §4.7).
5. **Field-level:** Structured fields (`model`, `loras`, `positive_prompt`, …) are always `Option` / empty collections. A missing field never prevents extraction of other fields or the raw searchable `prompt` string.
6. **Graph-level (ComfyUI):** Each workflow node is skipped independently if `class_type`, `inputs`, or referenced node IDs are missing. Cycle detection via `HashSet` prevents infinite loops.

**General rule:** Missing, incomplete, or malformed metadata leaves individual fields blank instead of failing the whole file. The searchable `prompt` string is preserved whenever any text can be recovered.

---

### 4.1 Extraction Pipeline Overview

```
extract_prompt(path)
  ├─ .png  → png::extract   → process_keyword() per tEXt/iTXt chunk
  ├─ .jpg/.jpeg → jpeg::extract → COM / APP1 XMP / APP1 EXIF
  └─ .webp → webp::extract  → XMP / EXIF RIFF chunks (seek past VP8*)

Each raw text blob is routed to:
  ├─ a1111::extract_details()     → A1111/Forge parameters, COM, XMP, NovelAI Description, JSON fallbacks
  └─ comfyui::extract_workflow()  → ComfyUI workflow JSON

Output: Vec<PromptRecord>  (0..N records per file; see §4.8)
```

Structured details (`PromptDetails`: model, loras, positive/negative prompts) are populated **at extraction time** and stored on `PromptRecord.details`. Output layers (JSON, `--structured` console) read this directly — no second-pass parsing.

---

### 4.2 PNG

PNG stores metadata in ancillary text chunks **before** image data. The reader validates the 8-byte signature, then loops:

```
for each chunk:
    read length (BE u32) + type (4 bytes) + data + CRC (4 bytes, not validated)
    if type == IDAT → stop (no metadata after image data in normal AI PNGs)
    if type in [tEXt, iTXt]:
        parse (keyword, value)
        process_keyword(keyword, value)
```

#### Chunk parsing

| Chunk | Layout | Notes |
|---|---|---|
| `tEXt` | `keyword\0value` | Latin-1 / UTF-8 lossy |
| `iTXt` | `keyword\0compression_flag(1) compression_method(1) language\0 translated_keyword\0 text` | **Only uncompressed** (`compression_flag == 0`) is supported. Compressed `iTXt` and `zTXt` chunks are not inflated — value will be garbage and JSON parse will fail → A1111 fallback if text looks like parameters. |

#### Keyword routing

| Keyword | Expected generator | Value format | Handler |
|---|---|---|---|
| `parameters` | A1111 / Forge | Plain A1111 parameters text | `a1111::extract_details(value)` → generator `A1111` |
| `prompt` | ComfyUI | JSON workflow object `{ "node_id": { "class_type", "inputs" }, ... }` | See §4.5 |
| `workflow` | ComfyUI | Secondary workflow JSON | **Ignored** — `prompt` keyword is the authoritative ComfyUI chunk |
| `Comment` | NovelAI | JSON with `"prompt"` field | See §4.6 |
| `Description` | NovelAI (alt) | Plain text | `a1111::extract_details(value)` → generator `NovelAI` |
| *(other)* | — | — | Silently skipped |

#### PNG-specific fault scenarios

| Scenario | Behavior |
|---|---|
| Truncated chunk (EOF mid-read) | Log if `--verbose`; return records collected so far |
| Chunk missing null separator | `parse_text_chunk` returns `None`; chunk skipped |
| `iTXt` with compression_flag = 1 | Uncompressed parser reads garbage; JSON fails → A1111 fallback on `prompt` keyword |
| Both `parameters` AND `prompt` chunks present | **Two separate `PromptRecord`s** emitted (common for ComfyUI exports that also embed A1111-style parameters). Search may match both. |
| Valid JSON `prompt` but no text-encode nodes | Record still created if model, loras, or sampler-traced prompts exist (`has_extractable_content`) |
| `Comment` JSON valid but no `"prompt"` key | **No record** — silently skipped |
| `Comment` JSON invalid | Fallback: treat raw value as plain text, generator `Unknown`, A1111 parse |

---

### 4.3 JPEG

JPEG metadata lives in markers between SOI (`0xFFD8`) and SOS (`0xFFDA`, start of scan data).

```
loop markers:
    if SOS → stop
    read segment length (BE u16, includes length field itself)
    match marker type:
        0xFFFE (COM)  → body as UTF-8 lossy text
        0xFFE1 (APP1) → if XMP header → extract dc:description
                      → if Exif\0\0 header → EXIF UserComment
        else          → skip
```

| Segment | Generator (heuristic) | Handler |
|---|---|---|
| COM (`0xFFFE`) | A1111 | Full body → `a1111::extract_details` |
| APP1 XMP | InvokeAI if `invokeai:` in XML; else Unknown | `extract_xmp_description` → `a1111::extract_details` |
| APP1 EXIF | Unknown | `exif::extract_user_comment` → `a1111::extract_details` |

**Note:** Standard EXIF tags other than `UserComment` are not scanned. AI prompts in JPEG are almost always in COM or XMP, not in camera EXIF fields.

#### JPEG-specific fault scenarios

| Scenario | Behavior |
|---|---|
| Truncated segment body | Stop loop; return partial results |
| `seg_len < 2` | Treat as malformed; stop |
| Marker sync lost (`!= 0xFF`) | Stop |
| COM body empty/whitespace | Skip (no record) |
| XMP missing `<dc:description>` or `<rdf:li>` | `extract_xmp_description` returns `None`; skip |
| EXIF UserComment count exceeds segment size | IFD reader bounds-checks; reads available bytes only (see §4.4) |
| Same file has COM + XMP + EXIF UserComment | Up to **three records** if each contains non-empty text |

---

### 4.4 EXIF UserComment (JPEG APP1 / WebP EXIF chunk)

Implemented in `exif.rs` as a minimal TIFF IFD walker — no external EXIF library.

```
Exif\0\0 prefix (optional) → TIFF header (II/MM + magic 42)
→ walk IFD0 → find ExifIFD (tag 0x8769) → find UserComment (tag 0x9286)
→ decode charset prefix + payload
```

| Charset tag | Decoding |
|---|---|
| `ASCII\0\0\0` | Raw bytes as lossy UTF-8 |
| `UNICODE\0` | UTF-16 BE or LE (BOM-aware: `FE FF` / `FF FE`) |
| Other / missing | Raw bytes as lossy UTF-8 |

**Safety limits:**
- IFD recursion depth capped at **4**
- Every read checks `offset + length <= buffer.len()`
- Oversized `count` field (declared length > available bytes) → decode what fits, no panic

Extracted text is passed through `a1111::extract_details` because UserComment often contains full A1111-style parameter blocks.

---

### 4.5 ComfyUI Workflow JSON

The PNG `prompt` keyword holds a JSON object: flat map of node IDs to node objects:

```json
{
  "3": {
    "class_type": "CLIPTextEncode",
    "inputs": { "text": "a beautiful sunset" }
  },
  "5": {
    "class_type": "KSampler",
    "inputs": { "positive": ["3", 0], "negative": ["4", 0] }
  }
}
```

**Expected top-level shape:** object whose values are nodes with `class_type` + `inputs`. Wrapped forms like `{"prompt": {...nodes...}}` are **not** unwrapped — they fail JSON routing and fall back to A1111 text parse.

#### Extraction strategy (`comfyui::extract_workflow`)

Two parallel extractions merge into one `(prompt, PromptDetails)`:

**A. Searchable prompt text** (for matching):
1. Scan all nodes; collect non-empty strings from text-encode nodes (`CLIPTextEncode`, `CLIPTextEncodeSDXL`, `CLIPTextEncodeFlux`, `TextEncodeQwenImageEdit*`, any `class_type` containing `Prompt` or `TextEncode`).
2. Fields checked per node: `text_g`, `text_l`, `text`, `prompt` (SDXL uses `_g` / `_l`).
3. Deduplicate identical strings; join with `" | "`.
4. **Fallback:** if step 1–3 yield nothing, join sampler-traced `positive_prompt` and `negative_prompt` with `" | "`.

**B. Structured details** (`PromptDetails`):
| Field | Source |
|---|---|
| `model` | `CheckpointLoader*` / `UNETLoader` / `UnetLoaderGGUF` nodes → `inputs.ckpt_name` or `inputs.unet_name` |
| `loras` | Any node whose `class_type` contains `"lora"` (case-insensitive). Standard `LoraLoader`: `lora_name` + weights. rgthree `Power Lora Loader`: object-valued inputs with `"lora"` key; skip if `"on": false`. |
| `positive_prompt` / `negative_prompt` | First `KSampler*` or `CFGGuider` node → follow `inputs.positive` / `inputs.negative` node refs through conditioning graph |

**Conditioning graph walk** (`resolve_conditioning_text`):
- Node refs are `["node_id", slot_index]` arrays — only first element used.
- If target is text-encode node → extract text fields.
- If target is `ZeroOut` → return `None` (empty negative conditioning).
- Else follow `inputs.positive` / `inputs.negative` / `inputs.conditioning`.
- **Cycle guard:** `HashSet<String>` of visited node IDs.

#### ComfyUI fault scenarios

| Scenario | Behavior |
|---|---|
| JSON parse fails entirely | **A1111 fallback** on raw string; generator `Unknown` |
| JSON valid but root is not an object | Empty `PromptDetails`; no record unless fallback text non-empty |
| Node missing `class_type` or `inputs` | Skip node |
| Text-encode nodes empty but model/loras present | Record created (`has_extractable_content`) |
| Text-encode nodes empty, sampler trace finds prompts | Searchable prompt from sampler fallback |
| Malformed `<lora:` tag in embedded A1111 text | `parse_loras` stops at first unclosed `>`; prior tags kept |
| Disabled rgthree lora (`"on": false`) | Skipped |
| Split model/clip lora weights | Stored as `"0.7 / 0.5"` string |
| Non-numeric lora weight | Kept as string (not dropped) |

---

### 4.6 NovelAI JSON (`Comment` keyword)

Expected shape:

```json
{"prompt": "1girl, masterpiece, ...", "steps": 28, "sampler": "..."}
```

| Parse result | Behavior |
|---|---|
| Valid JSON + `"prompt"` string field | Use that string as searchable prompt; `a1111::extract_details` on prompt text; generator `NovelAI` |
| Valid JSON but `"prompt"` missing or not a string | **No record** |
| Invalid / truncated JSON | Fallback: entire raw value as prompt text; generator `Unknown`; A1111 parse for structured fields |
| `Description` keyword (plain text) | Always A1111 parse; generator `NovelAI` |

---

### 4.7 A1111 Parameter Text Parsing (`a1111.rs`)

Used for: PNG `parameters`, JPEG COM, XMP `dc:description`, EXIF UserComment, NovelAI `Description`, and **all JSON-parse failure fallbacks**.

#### Text structure

A1111/Forge embeds a single text block:

```
{positive prompt body}
Negative prompt: {negative prompt body}        ← optional
Steps: 30, Sampler: Euler a, CFG scale: 7, Model: foo.safetensors, Seed: 123, ...
```

Parsing steps:
1. Split at line-start `Steps:` (also matches `\nSteps:`) → body vs settings block.
2. Split body at `Negative prompt:` → positive / negative.
3. Extract `<lora:name:weight>` tags from **full original text** (not just positive).
4. Extract model from settings via `parse_model()`:

```
parse_model() search order:
  ", Model:"  →  value until comma/newline
  "\nModel:"  →  same
  "Model:"    →  same, BUT skip if match is "Model hash:" prefix

Quoted values supported: Model: "name.safetensors"
```

**Known pitfall avoided:** `Model hash: abc123, Model: RealModel` — naive `find("Model:")` would match `Model hash:` first. Parser explicitly skips that prefix.

#### A1111 fault scenarios

| Scenario | Behavior |
|---|---|
| No `Steps:` marker | Entire text treated as positive prompt; model searched in full text |
| No `Negative prompt:` | Whole body is positive; negative = empty |
| Body starts with `Steps:` | Positive = empty string |
| Malformed `<lora:name` (no closing `>`) | Stop scanning; keep tags found so far |
| Duplicate lora names | First occurrence kept |
| Settings-only text (`Steps: 4, Sampler: Euler`) | All structured fields empty; raw text still searchable |
| Model value contains `Lora hashes: "abc"` later | Comma-terminated parse stops at first comma — model not polluted by later quoted fields |

---

### 4.8 Multi-Record Files & Generator Tagging

A single image file may yield **0 to N** `PromptRecord` entries:

| Source | Typical count |
|---|---|
| PNG with only `parameters` | 1 |
| PNG with `parameters` + `prompt` (ComfyUI) | 2 |
| JPEG with COM + XMP | 1–2 |
| WebP with XMP only | 1 |

Each record carries:
- `prompt` — searchable text (used by matcher)
- `generator` — best-effort tag (informational; does not affect matching)
- `metadata_key` — source chunk/segment (`parameters`, `prompt`, `COM`, `XMP`, `UserComment`, …)
- `details` — optional structured fields parsed at extraction time

Records are sorted by `(path, metadata_key)` for deterministic output.

#### Generator detection (best-effort)

| Signal | `Generator` |
|---|---|
| PNG keyword `parameters` | `A1111` |
| PNG keyword `prompt` + valid JSON | `ComfyUI` |
| PNG keyword `Comment` + valid JSON with `"prompt"` | `NovelAI` |
| PNG keyword `Description` | `NovelAI` |
| PNG keyword `prompt` + JSON fail | `Unknown` |
| JPEG COM | `A1111` |
| XMP containing `invokeai:` | `InvokeAI` |
| Other XMP / EXIF UserComment | `Unknown` |

---

### 4.9 Known Limitations (Not Yet Supported)

| Limitation | Impact | Workaround |
|---|---|---|
| `iTXt` / `zTXt` compressed PNG text | Garbled value; JSON fails | A1111 fallback if text resembles parameters |
| PNG metadata after `IDAT` | Never read | Rare; re-save PNG with metadata before image data |
| ComfyUI JSON wrapped in outer object | Treated as invalid JSON → A1111 fallback | — |
| Partial JSON repair | Not implemented | Full string preserved for search via fallback |
| XMP formats without `<dc:description><rdf:Alt><rdf:li>` | `None` returned | — |
| Multiple XMP `<rdf:li>` entries | Only first extracted | — |
| AVIF / TIFF / GIF / BMP | Skipped (unsupported extension) | — |

---

### 4.10 Reference: Keyword → Parser Matrix

```
PNG keyword "parameters"  ──→ a1111::extract_details          [A1111]
PNG keyword "prompt"      ──→ serde_json OK  → comfyui::extract_workflow  [ComfyUI]
                         └→ serde_json ERR → a1111::extract_details      [Unknown]
PNG keyword "Comment"   ──→ serde_json OK  → json["prompt"] → a1111       [NovelAI]
                         └→ serde_json ERR → a1111::extract_details      [Unknown]
PNG keyword "Description" ─→ a1111::extract_details          [NovelAI]

JPEG COM                ──→ a1111::extract_details          [A1111]
JPEG APP1 XMP           ──→ extract_xmp_description → a1111 [InvokeAI/Unknown]
JPEG APP1 EXIF          ──→ exif::extract_user_comment → a1111 [Unknown]

WebP XMP chunk          ──→ (same as JPEG XMP)
WebP EXIF chunk         ──→ (same as JPEG EXIF)
```

---

### A. PNG (legacy quick reference)

PNG stores metadata in ancillary text chunks. Read chunks sequentially from the file header; stop after `IDAT` (image data starts, no more metadata).

| Chunk Type | Keyword | Generator | Format |
|---|---|---|---|
| `tEXt` | `parameters` | Stable Diffusion (A1111/Forge) | Plain text. Prompt is the value directly. |
| `tEXt` | `prompt` | ComfyUI | **JSON string** containing the full workflow graph. Must be parsed (see §4.5). |
| `tEXt` | `workflow` | ComfyUI | Workflow JSON (secondary; **ignored** — use `prompt` keyword). |
| `tEXt` | `Comment` | NovelAI | **JSON string** with a `"prompt"` field inside. |
| `tEXt` | `Description` | NovelAI (alternate) | Plain text prompt. |
| `iTXt` | (same keys) | Same generators (UTF-8 variant) | Same routing; **uncompressed only** (see §4.2). |

**Manual PNG chunk parsing** (current implementation):
- Validate 8-byte PNG signature.
- Read chunks: 4-byte length (big-endian) + 4-byte type + data + 4-byte CRC (ignored).
- For `tEXt`: keyword is null-terminated string at start of data; value is the rest.
- For `iTXt`: keyword null-terminated, then compression flag + method + language + translated keyword (each null-terminated), then text value.

### B. JPEG (legacy quick reference)

| Segment | Marker | Generator | Format |
|---|---|---|---|
| COM (Comment) | `0xFFFE` | A1111, some others | Plain text. The raw comment string is the prompt. |
| APP1 (XMP) | `0xFFE1` | Various | XML; extract content of `<dc:description>` |
| APP1 (EXIF) | `0xFFE1` | Rare/legacy | EXIF UserComment via manual IFD walk (see §4.4) |

### C. WebP (legacy quick reference)

| RIFF Chunk | Content | Extraction |
|---|---|---|
| `EXIF` | Standard EXIF blob | Same as JPEG APP1 EXIF (§4.4) |
| `XMP ` (trailing space) | XMP XML | Same as JPEG XMP |
| `VP8` / `VP8L` / `VP8X` / … | Image data | **Seeked past** without loading into memory |

### D. Generator Detection

See §4.8 for the full table. Generator tags are informational only and do not affect matching.

### E. ComfyUI Workflow JSON Parsing

See §4.5 for the full extraction strategy, node types, conditioning graph walk, and fault scenarios.

### F. NovelAI JSON Parsing

See §4.6 for valid JSON, invalid JSON fallback, and `Description` handling.

---

## 5. Matching Engine

### Exact Mode (default)

Case-insensitive substring search. The query is lowercased once. Matching uses Unicode-aware case folding (`to_lowercase()`) on both query and prompt — not byte-level ASCII comparison.

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
- If `--full` flag is set, print the entire prompt without truncation (also enables structured mode for backward compatibility).
- Long prompts (>500 chars) are truncated with `...` on both ends unless `--full` is specified.
- Truncation is UTF-8 safe (character boundaries, not byte slices).

### Structured Console (`--structured`, or `--full`)

When enabled, each match prints parsed fields in addition to the context window:

```
📄 ./output/image.png [comfyui]
   Model: PlantMilkModelSuite_almond
   LoRAs: lora:style:0.7, lora:detail:0.5
   Positive: a beautiful sunset over mountains
   Negative: blurry, low quality
   ...context window around match...
```

With `--path-only` (`-p`), only file paths are printed (works in structured mode too).

### JSON (`--format json`)

```json
[
  {
    "path": "./assets/cyberpunk_city.png",
    "generator": "a1111",
    "prompt": "a detailed photo of a cyberpunk city at night, neon lights...",
    "score": 100,
    "model": "realisticVision.safetensors",
    "loras": [{"name": "style", "weight": "0.7"}],
    "positive_prompt": "a detailed photo of a cyberpunk city...",
    "negative_prompt": "blurry, low quality"
  }
]
```

- `prompt` is always the full searchable text extracted from metadata.
- `model`, `loras`, `positive_prompt`, `negative_prompt` are omitted when empty (`skip_serializing_if`).
- `score` is included only in fuzzy mode; omitted in exact/regex mode.
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
      --full                Show full prompt text (no truncation); also enables structured mode
      --structured          Show model, loras, positive/negative prompts in console mode
  -p, --path-only           Print only file paths (no prompt text)
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

# Structured console output with parsed fields
ips -q "sunset" --structured ./ai_art

# Export to JSON (includes structured fields when present)
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
| Corrupt/truncated image container | Return partial metadata collected before truncation; log if `--verbose`. |
| Truncated / invalid JSON in metadata | No panic. ComfyUI/NovelAI: fall back to A1111 text parse on raw string (see §4.0, §4.5, §4.6). |
| Malformed A1111 parameter text | Partial structured fields; raw text still searchable (see §4.7). |
| Unsupported format (e.g. `.gif`, `.bmp`) | Skip silently (not an error). |
| Symlink loop | `walkdir` detects and skips by default. |
| Non-UTF-8 metadata | Lossy UTF-8 conversion (`String::from_utf8_lossy`). |
| Empty directory / no matches | Exit with code 0, empty output. Print "No matches found." to stderr in console mode. |
| Invalid query regex | Exit with code 1 and a clear error message. |

General principle: **Never crash on bad input.** Log and skip individual files; only fatal errors (bad CLI args, invalid regex) cause a non-zero exit. Metadata extraction follows the layered fault-tolerance model in §4.0 — individual fields may be empty without losing the whole record.

---

## 9. Project Structure

```
ips/
├── Cargo.toml
├── ips_design_doc.md
├── src/
│   ├── main.rs              # CLI entry point, argument parsing
│   ├── lib.rs               # Library crate root (shared by main + tests)
│   ├── discovery.rs         # Directory walking, extension filtering
│   ├── extract/
│   │   ├── mod.rs           # Public extract API: fn extract_prompt(path) -> Vec<PromptRecord>
│   │   ├── a1111.rs         # A1111/NovelAI parameter text parsing (model, loras, prompts)
│   │   ├── png.rs           # PNG tEXt/iTXt chunk parsing
│   │   ├── jpeg.rs          # JPEG COM marker, XMP, EXIF routing
│   │   ├── webp.rs          # WebP RIFF chunk parsing
│   │   ├── exif.rs          # Minimal EXIF IFD walker (UserComment)
│   │   └── comfyui.rs       # ComfyUI workflow JSON extraction
│   ├── matcher.rs           # Exact, fuzzy, and regex matching
│   ├── output/
│   │   ├── mod.rs           # Output dispatcher
│   │   ├── console.rs       # ANSI-highlighted + structured console output
│   │   ├── json.rs          # JSON serialization (prompt + structured fields)
│   │   └── csv.rs           # CSV serialization
│   └── types.rs             # PromptRecord, PromptDetails, MatchResult, Config
└── tests/
    ├── examples/            # Test images from each generator (optional fixtures)
    └── integration_tests.rs
```

---

## 10. Data Types

```rust
/// A prompt extracted from a single image file.
pub struct PromptRecord {
    pub path: PathBuf,
    pub prompt: String,              // searchable text for matching
    pub generator: Generator,
    pub metadata_key: String,        // e.g. "parameters", "prompt", "COM", "XMP"
    pub details: Option<PromptDetails>, // parsed at extraction time
}

/// Structured fields extracted from A1111 text or ComfyUI workflow.
pub struct PromptDetails {
    pub model: Option<String>,
    pub loras: Vec<LoraInfo>,        // weight is String (supports "0.7 / 0.5")
    pub positive_prompt: Option<String>,
    pub negative_prompt: Option<String>,
}

pub struct LoraInfo {
    pub name: String,
    pub weight: String,
}

pub enum Generator {
    A1111, ComfyUI, NovelAI, InvokeAI, Unknown,
}

/// A matched result ready for output.
pub struct MatchResult {
    pub record: PromptRecord,
    pub score: Option<i64>,              // fuzzy score; None for exact/regex
    pub match_ranges: Vec<(usize, usize)>, // byte ranges for highlighting
}
```

Structured details are populated during extraction (`a1111::extract_details`, `comfyui::extract_workflow`) and consumed directly by JSON output and `--structured` console — there is no second-pass parsing at output time.

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
- Implement WebP RIFF chunk reader (EXIF + XMP chunks; seek past VP8* image data).
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
