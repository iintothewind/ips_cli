use std::io::{BufReader, Read};
use std::path::Path;

use crate::types::{Generator, PromptRecord};
use super::{a1111, comfyui};

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

pub fn extract(path: &Path, verbose: bool) -> Vec<PromptRecord> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            if verbose {
                eprintln!("ips: cannot read {}: {}", path.display(), e);
            }
            return vec![];
        }
    };

    let mut reader = BufReader::new(file);

    let mut sig = [0u8; 8];
    if reader.read_exact(&mut sig).is_err() || &sig != PNG_SIGNATURE {
        if verbose {
            eprintln!("ips: {}: not a valid PNG", path.display());
        }
        return vec![];
    }

    let mut results = Vec::new();

    loop {
        let mut length_buf = [0u8; 4];
        match reader.read_exact(&mut length_buf) {
            Ok(_) => {}
            Err(_) => break,
        }
        let length = u32::from_be_bytes(length_buf) as usize;

        let mut chunk_type = [0u8; 4];
        if reader.read_exact(&mut chunk_type).is_err() {
            break;
        }

        if &chunk_type == b"IDAT" {
            break;
        }

        let mut chunk_data = vec![0u8; length];
        if reader.read_exact(&mut chunk_data).is_err() {
            if verbose {
                eprintln!("ips: {}: truncated chunk", path.display());
            }
            break;
        }

        let mut crc_buf = [0u8; 4];
        if reader.read_exact(&mut crc_buf).is_err() {
            break;
        }

        match &chunk_type {
            b"tEXt" => {
                if let Some((keyword, value)) = parse_text_chunk(&chunk_data) {
                    process_keyword(path, &keyword, &value, &mut results);
                }
            }
            b"iTXt" => {
                if let Some((keyword, value)) = parse_itxt_chunk(&chunk_data) {
                    process_keyword(path, &keyword, &value, &mut results);
                }
            }
            _ => {}
        }
    }

    results
}

fn parse_text_chunk(data: &[u8]) -> Option<(String, String)> {
    let null_pos = data.iter().position(|&b| b == 0)?;
    let keyword = String::from_utf8_lossy(&data[..null_pos]).into_owned();
    let value = String::from_utf8_lossy(&data[null_pos + 1..]).into_owned();
    Some((keyword, value))
}

fn parse_itxt_chunk(data: &[u8]) -> Option<(String, String)> {
    let kw_end = data.iter().position(|&b| b == 0)?;
    let keyword = String::from_utf8_lossy(&data[..kw_end]).into_owned();

    let mut pos = kw_end + 3;
    if pos > data.len() {
        return None;
    }

    let lang_end = data[pos..].iter().position(|&b| b == 0)?;
    pos += lang_end + 1;

    let trans_end = data[pos..].iter().position(|&b| b == 0)?;
    pos += trans_end + 1;

    let value = String::from_utf8_lossy(&data[pos..]).into_owned();
    Some((keyword, value))
}

fn process_keyword(path: &Path, keyword: &str, value: &str, results: &mut Vec<PromptRecord>) {
    match keyword {
        "parameters" => {
            results.push(PromptRecord::with_details(
                path.to_path_buf(),
                value.to_string(),
                Generator::A1111,
                keyword,
                a1111::extract_details(value),
            ));
        }
        "prompt" => {
            match serde_json::from_str::<serde_json::Value>(value) {
                Ok(json) => {
                    let (prompt, details) = comfyui::extract_workflow(&json);
                    if comfyui::has_extractable_content(&prompt, &details) {
                        results.push(PromptRecord::with_details(
                            path.to_path_buf(),
                            prompt,
                            Generator::ComfyUI,
                            keyword,
                            details,
                        ));
                    }
                }
                Err(_) => {
                    if !value.trim().is_empty() {
                        results.push(PromptRecord::with_details(
                            path.to_path_buf(),
                            value.to_string(),
                            Generator::Unknown,
                            keyword,
                            a1111::extract_details(value),
                        ));
                    }
                }
            }
        }
        "Comment" => {
            match serde_json::from_str::<serde_json::Value>(value) {
                Ok(json) => {
                    if let Some(prompt) = json.get("prompt").and_then(|v| v.as_str()) {
                        results.push(PromptRecord::with_details(
                            path.to_path_buf(),
                            prompt.to_string(),
                            Generator::NovelAI,
                            keyword,
                            a1111::extract_details(prompt),
                        ));
                    }
                }
                Err(_) => {
                    if !value.trim().is_empty() {
                        results.push(PromptRecord::with_details(
                            path.to_path_buf(),
                            value.to_string(),
                            Generator::Unknown,
                            keyword,
                            a1111::extract_details(value),
                        ));
                    }
                }
            }
        }
        "Description" => {
            if !value.trim().is_empty() {
                results.push(PromptRecord::with_details(
                    path.to_path_buf(),
                    value.to_string(),
                    Generator::NovelAI,
                    keyword,
                    a1111::extract_details(value),
                ));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_png_chunk(chunk_type: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let length = (data.len() as u32).to_be_bytes();
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&length);
        chunk.extend_from_slice(chunk_type);
        chunk.extend_from_slice(data);
        chunk.extend_from_slice(&[0u8; 4]);
        chunk
    }

    fn make_png_with_chunks(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut png = Vec::new();
        png.extend_from_slice(PNG_SIGNATURE);
        for chunk in chunks {
            png.extend_from_slice(chunk);
        }
        png.extend_from_slice(&make_png_chunk(b"IEND", &[]));
        png
    }

    #[test]
    fn extracts_a1111_model_from_parameters() {
        let text = "masterpiece\nNegative prompt: blurry\nSteps: 30, Model: base.safetensors, Seed: 1";
        let mut chunk_data = b"parameters\x00".to_vec();
        chunk_data.extend_from_slice(text.as_bytes());
        let png_bytes = make_png_with_chunks(&[make_png_chunk(b"tEXt", &chunk_data)]);

        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.png");
        std::fs::write(&path, &png_bytes).unwrap();

        let records = extract(&path, false);
        assert_eq!(
            records[0].details_or_default().model.as_deref(),
            Some("base.safetensors")
        );
    }

    #[test]
    fn falls_back_to_a1111_when_prompt_json_invalid() {
        let text = "masterpiece\nNegative prompt: blurry\nSteps: 30, Model: base.safetensors, Seed: 1";
        let mut data = b"prompt\x00".to_vec();
        data.extend_from_slice(text.as_bytes());
        let png_bytes = make_png_with_chunks(&[make_png_chunk(b"tEXt", &data)]);

        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.png");
        std::fs::write(&path, &png_bytes).unwrap();

        let records = extract(&path, false);
        assert_eq!(records[0].generator, Generator::Unknown);
        assert_eq!(
            records[0].details_or_default().model.as_deref(),
            Some("base.safetensors")
        );
    }
}
