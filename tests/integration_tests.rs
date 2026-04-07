use std::fs;
use std::path::Path;

/// Build a minimal valid PNG with a tEXt chunk containing the given keyword and value.
fn build_png_with_text(keyword: &str, value: &str) -> Vec<u8> {
    let png_sig: &[u8] = b"\x89PNG\r\n\x1a\n";

    fn make_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&(data.len() as u32).to_be_bytes());
        chunk.extend_from_slice(kind);
        chunk.extend_from_slice(data);
        chunk.extend_from_slice(&[0u8; 4]); // fake CRC
        chunk
    }

    let mut text_data = keyword.as_bytes().to_vec();
    text_data.push(0); // null separator
    text_data.extend_from_slice(value.as_bytes());

    let ihdr_data = [
        0, 0, 0, 1, // width = 1
        0, 0, 0, 1, // height = 1
        8, 2, 0, 0, 0, // bit depth, color type, etc.
    ];

    let mut png = Vec::new();
    png.extend_from_slice(png_sig);
    png.extend_from_slice(&make_chunk(b"IHDR", &ihdr_data));
    png.extend_from_slice(&make_chunk(b"tEXt", &text_data));
    png.extend_from_slice(&make_chunk(b"IDAT", b"\x00")); // minimal image data
    png.extend_from_slice(&make_chunk(b"IEND", b""));
    png
}

fn build_jpeg_with_com(comment: &str) -> Vec<u8> {
    let mut jpeg = Vec::new();
    jpeg.extend_from_slice(&[0xFF, 0xD8]); // SOI
    jpeg.extend_from_slice(&[0xFF, 0xFE]); // COM
    let body = comment.as_bytes();
    let len = (body.len() as u16 + 2).to_be_bytes();
    jpeg.extend_from_slice(&len);
    jpeg.extend_from_slice(body);
    jpeg.extend_from_slice(&[0xFF, 0xD9]); // EOI
    jpeg
}

#[test]
fn end_to_end_a1111_png_exact() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("a1111.png");
    fs::write(&path, build_png_with_text("parameters", "masterpiece, 1girl, cyberpunk city")).unwrap();

    // Use the library directly rather than spawning a process to keep tests fast.
    use ips::*;

    let config = types::Config {
        query: "cyberpunk".to_string(),
        path: dir.path().to_path_buf(),
        format: types::OutputFormat::Console,
        match_mode: types::MatchMode::Exact,
        min_score: 50,
        full: false,
        depth: None,
        no_recursive: false,
        threads: None,
        verbose: false,
        no_color: true,
    };

    let files = discovery::discover_files(&config);
    assert_eq!(files.len(), 1);

    let records = extract::extract_prompt(&files[0], false);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].generator, types::Generator::A1111);

    let result = matcher::match_record(&records[0], &config);
    assert!(result.is_some());
}

#[test]
fn end_to_end_jpeg_com_no_match() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("img.jpg");
    fs::write(&path, build_jpeg_with_com("peaceful forest landscape")).unwrap();

    use ips::*;

    let config = types::Config {
        query: "cyberpunk".to_string(),
        path: dir.path().to_path_buf(),
        format: types::OutputFormat::Console,
        match_mode: types::MatchMode::Exact,
        min_score: 50,
        full: false,
        depth: None,
        no_recursive: false,
        threads: None,
        verbose: false,
        no_color: true,
    };

    let files = discovery::discover_files(&config);
    let results: Vec<_> = files
        .iter()
        .flat_map(|p| extract::extract_prompt(p, false))
        .filter_map(|r| matcher::match_record(&r, &config))
        .collect();

    assert!(results.is_empty());
}

#[test]
fn comfyui_workflow_extraction() {
    use ips::extract::comfyui;
    use serde_json::json;

    let workflow = json!({
        "10": {
            "class_type": "CLIPTextEncode",
            "inputs": { "text": "anime girl, cherry blossoms, spring" }
        },
        "11": {
            "class_type": "CLIPTextEncode",
            "inputs": { "text": "worst quality, blurry" }
        },
        "5": {
            "class_type": "KSampler",
            "inputs": { "steps": 30 }
        }
    });

    let prompts = comfyui::extract_from_workflow(&workflow);
    assert_eq!(prompts.len(), 2);
    assert!(prompts.iter().any(|p| p.contains("cherry blossoms")));
    assert!(prompts.iter().any(|p| p.contains("worst quality")));
}

#[test]
fn fuzzy_matching_finds_approximate() {
    use ips::*;
    use std::path::PathBuf;

    let record = types::PromptRecord {
        path: PathBuf::from("test.png"),
        prompt: "masterpiece, 1girl, detailed background, cyberpunk".to_string(),
        generator: types::Generator::A1111,
        metadata_key: "parameters".to_string(),
    };

    let config = types::Config {
        query: "cybrpnk".to_string(),
        path: PathBuf::from("."),
        format: types::OutputFormat::Console,
        match_mode: types::MatchMode::Fuzzy,
        min_score: 10, // low threshold to ensure fuzzy match succeeds
        full: false,
        depth: None,
        no_recursive: false,
        threads: None,
        verbose: false,
        no_color: true,
    };

    let result = matcher::match_record(&record, &config);
    assert!(result.is_some());
    assert!(result.unwrap().score.is_some());
}

#[test]
fn regex_matching() {
    use ips::*;
    use std::path::PathBuf;

    let record = types::PromptRecord {
        path: PathBuf::from("test.png"),
        prompt: "1girl, masterpiece, 4k resolution".to_string(),
        generator: types::Generator::A1111,
        metadata_key: "parameters".to_string(),
    };

    let config = types::Config {
        query: r"\dk\s+resolution".to_string(),
        path: PathBuf::from("."),
        format: types::OutputFormat::Console,
        match_mode: types::MatchMode::Regex,
        min_score: 50,
        full: false,
        depth: None,
        no_recursive: false,
        threads: None,
        verbose: false,
        no_color: true,
    };

    let result = matcher::match_record(&record, &config);
    assert!(result.is_some());
}

// ── Real-file tests using tests/examples/ ─────────────────────────────────────

fn examples_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("examples")
}

fn extract_from(filename: &str) -> Vec<ips::types::PromptRecord> {
    let path = examples_dir().join(filename);
    ips::extract::extract_prompt(&path, false)
}

/// 00029-795142250.jpg — A1111 JPEG with EXIF UserComment (big-endian TIFF,
/// full count fits in segment).
#[test]
fn real_jpeg_exif_usercomment_full() {
    let records = extract_from("00029-795142250.jpg");
    assert!(!records.is_empty(), "should extract at least one prompt");
    let combined: String = records.iter().map(|r| r.prompt.as_str()).collect::<Vec<_>>().join(" ");
    assert!(combined.contains("score_9"), "expected 'score_9' in prompt");
    assert!(
        records.iter().any(|r| r.metadata_key == "UserComment"),
        "should come from UserComment"
    );
}

/// 20251230210852.jpg — EXIF UserComment where the declared count exceeds the
/// APP1 body size (truncated/malformed EXIF).  Must still extract what's there.
#[test]
fn real_jpeg_exif_usercomment_truncated_count() {
    let records = extract_from("20251230210852.jpg");
    assert!(!records.is_empty(), "should extract prompt despite oversized count");
    let combined: String = records.iter().map(|r| r.prompt.as_str()).collect::<Vec<_>>().join(" ");
    assert!(combined.to_lowercase().contains("masterpiece"), "expected 'masterpiece' in prompt");
}

/// 00287-1450597514.png — A1111 PNG with tEXt parameters chunk.
#[test]
fn real_png_a1111_parameters() {
    let records = extract_from("00287-1450597514.png");
    assert!(!records.is_empty());
    assert!(
        records.iter().any(|r| r.generator == ips::types::Generator::A1111),
        "should detect A1111 generator"
    );
}

/// Search the whole examples directory and confirm we get results.
#[test]
fn real_directory_search_finds_matches() {
    use ips::*;

    let config = types::Config {
        query: "masterpiece".to_string(),
        path: examples_dir(),
        format: types::OutputFormat::Console,
        match_mode: types::MatchMode::Exact,
        min_score: 50,
        full: false,
        depth: None,
        no_recursive: false,
        threads: None,
        verbose: false,
        no_color: true,
    };

    let files = discovery::discover_files(&config);
    let results: Vec<_> = files
        .iter()
        .flat_map(|p| extract::extract_prompt(p, false))
        .filter_map(|r| matcher::match_record(&r, &config))
        .collect();

    assert!(!results.is_empty(), "should find 'masterpiece' in at least one example image");
}
