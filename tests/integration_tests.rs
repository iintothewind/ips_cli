use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener};
use std::process::{Command, Stdio};
use std::thread;

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
        path_only: false,
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
        path_only: false,
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
        details: None,
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
        path_only: false,
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
        details: None,
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
        path_only: false,
    };

    let result = matcher::match_record(&record, &config);
    assert!(result.is_some());
}


fn ips() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ips"));
    for var in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        cmd.env_remove(var);
    }
    cmd
}

fn http_response(status: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

/// Serve canned raw HTTP responses by request target; anything else gets 404.
/// Each connection is answered once and then half-closed, so a client that reads past
/// what was sent sees a premature EOF instead of hanging.
fn serve(routes: Vec<(&'static str, Vec<u8>)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let routes = routes.clone();
            thread::spawn(move || {
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(n) if n > 0 => request.extend_from_slice(&buf[..n]),
                        _ => return,
                    }
                }
                let head = String::from_utf8_lossy(&request);
                let target = head.split_whitespace().nth(1).unwrap_or("");
                let response = routes
                    .iter()
                    .find(|(route, _)| *route == target)
                    .map(|(_, response)| response.clone())
                    .unwrap_or_else(|| http_response("404 Not Found", b"missing"));
                stream.write_all(&response).ok();
                stream.shutdown(Shutdown::Write).ok();
                while matches!(stream.read(&mut buf), Ok(n) if n > 0) {}
            });
        }
    });
    base
}

/// WebP whose XMP chunk follows an odd-sized image chunk (forces skip + padding).
fn build_webp_with_xmp_after_image(prompt: &str) -> Vec<u8> {
    fn riff_chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut chunk = id.to_vec();
        chunk.extend_from_slice(&(data.len() as u32).to_le_bytes());
        chunk.extend_from_slice(data);
        if data.len() % 2 == 1 {
            chunk.push(0);
        }
        chunk
    }
    let xmp = format!(
        r#"<rdf:RDF><rdf:Description><dc:description><rdf:Alt><rdf:li xml:lang="x-default">{prompt}</rdf:li></rdf:Alt></dc:description></rdf:Description></rdf:RDF>"#
    );
    let mut payload = b"WEBP".to_vec();
    payload.extend(riff_chunk(b"VP8 ", &vec![0u8; 4097]));
    payload.extend(riff_chunk(b"XMP ", xmp.as_bytes()));
    let mut webp = b"RIFF".to_vec();
    webp.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    webp.extend(payload);
    webp
}

fn json(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn no_query_dumps_every_record() {
    let dir = tempfile::TempDir::new().unwrap();
    fs::write(dir.path().join("a.png"), build_png_with_text("parameters", "masterpiece, 1girl")).unwrap();

    let output = ips().args(["-f", "json"]).arg(dir.path()).output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let records = json(&output);
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(records[0]["generator"], "a1111");
    assert_eq!(records[0]["positive_prompt"], "masterpiece, 1girl");
}

#[test]
fn input_errors_exit_2() {
    let dir = tempfile::TempDir::new().unwrap();
    let cases: [(Vec<std::ffi::OsString>, &str); 4] = [
        (vec!["-q".into(), "x".into(), dir.path().join("nope").into()], "cannot read"),
        (vec!["-q".into(), "x".into(), "http://127.0.0.1:9/a.png".into()], "--url"),
        (vec!["-q".into(), "(".into(), "--regex".into(), dir.path().into()], "invalid regex pattern"),
        (vec!["--url-list".into(), dir.path().join("nope.txt").into()], "cannot read URL list"),
    ];
    for (args, message) in cases {
        let output = ips().args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(stderr(&output).contains(message), "{args:?}: {}", stderr(&output));
    }
}

#[test]
fn url_without_extension_is_sniffed() {
    let png = build_png_with_text("parameters", "cyberpunk city");
    let base = serve(vec![("/img?id=1", http_response("200 OK", &png))]);
    let url = format!("{base}/img?id=1");

    let output = ips().args(["-f", "json", "--url", &url]).output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let records = json(&output);
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(records[0]["path"], url.as_str());
    assert_eq!(records[0]["positive_prompt"], "cyberpunk city");
}

#[test]
fn url_list_mixed_outcomes_exit_3() {
    let png = build_png_with_text("parameters", "cyberpunk city");
    let base = serve(vec![
        ("/ok.png", http_response("200 OK", &png)),
        ("/page", http_response("200 OK", b"<html></html>")),
    ]);
    let dir = tempfile::TempDir::new().unwrap();
    let list = dir.path().join("urls.txt");
    fs::write(
        &list,
        format!("\u{feff}{base}/ok.png\r\n\r\n{base}/missing.png\r\n{base}/page\r\nnot-a-url\r\n{base}/ok.png\r\n"),
    )
    .unwrap();

    let output = ips().args(["-f", "json", "--url-list"]).arg(&list).output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    let records = json(&output);
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(records[0]["path"], format!("{base}/ok.png").as_str());
    let err = stderr(&output);
    let missing = err
        .lines()
        .find(|line| line.starts_with(&format!("ips: {base}/missing.png: ")))
        .unwrap_or_else(|| panic!("{err}"));
    assert!(missing.contains("404"), "{err}");
    assert!(err.contains(&format!("ips: {base}/page: not a PNG, JPEG, or WebP image")), "{err}");
    assert!(err.contains(":5: not an http(s) URL: not-a-url"), "{err}");
    assert!(err.contains("ips: 3 of 4 URL(s) failed"), "{err}");
}

#[test]
fn url_list_from_stdin() {
    let png = build_png_with_text("parameters", "cyberpunk city");
    let base = serve(vec![("/ok.png", http_response("200 OK", &png))]);

    let mut child = ips()
        .args(["-f", "json", "--url-list", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(format!("{base}/ok.png\n").as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(json(&output).as_array().unwrap().len(), 1);
}

#[test]
fn webp_url_reads_metadata_after_image_data() {
    let webp = build_webp_with_xmp_after_image("sunset landscape, watercolor");
    let base = serve(vec![("/a", http_response("200 OK", &webp))]);

    let output = ips().args(["-f", "json", "--url", &format!("{base}/a")]).output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(json(&output)[0]["positive_prompt"], "sunset landscape, watercolor");
}

#[test]
fn png_url_stops_at_image_data() {
    let png = build_png_with_text("parameters", "early stop");
    let cut = png.windows(4).position(|w| w == b"IDAT").unwrap() + 4;
    let mut response =
        b"HTTP/1.1 200 OK\r\nContent-Length: 50000000\r\nConnection: close\r\n\r\n".to_vec();
    response.extend_from_slice(&png[..cut]);
    let base = serve(vec![("/big", response)]);

    let output = ips().args(["-f", "json", "--url", &format!("{base}/big")]).output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(json(&output)[0]["positive_prompt"], "early stop");
}

#[test]
fn truncated_url_body_is_a_failure() {
    let png = build_png_with_text("parameters", "lost");
    // 8 signature + 25 IHDR + 8 tEXt header + 5 bytes of tEXt data
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        png.len()
    )
    .into_bytes();
    response.extend_from_slice(&png[..46]);
    let base = serve(vec![("/cut", response)]);

    let output = ips().args(["-f", "json", "--url", &format!("{base}/cut")]).output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(json(&output), serde_json::json!([]));
    assert!(stderr(&output).contains(&format!("ips: {base}/cut: ")), "{}", stderr(&output));
}

#[test]
fn piped_console_output_has_no_ansi() {
    let dir = tempfile::TempDir::new().unwrap();
    fs::write(dir.path().join("a.png"), build_png_with_text("parameters", "masterpiece, 1girl")).unwrap();

    let output = ips()
        .args(["-q", "masterpiece"])
        .arg(dir.path())
        .env_remove("NO_COLOR")
        .env("TERM", "xterm-256color")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("masterpiece"), "{stdout}");
    assert!(!stdout.contains("\x1b["), "{stdout:?}");
}
