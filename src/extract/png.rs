use std::io::Read;
use std::path::Path;

use flate2::read::ZlibDecoder;

use crate::types::{Generator, PromptRecord};
use super::a1111;

pub(super) const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

pub fn extract<R: Read>(reader: &mut R, origin: &Path, verbose: bool) -> Vec<PromptRecord> {
    let mut sig = [0u8; 8];
    if reader.read_exact(&mut sig).is_err() || &sig != PNG_SIGNATURE {
        if verbose {
            eprintln!("ips: {}: not a valid PNG", origin.display());
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

        let chunk_data = match super::read_exact_vec(reader, length) {
            Ok(data) => data,
            Err(_) => {
                if verbose {
                    eprintln!("ips: {}: truncated chunk", origin.display());
                }
                break;
            }
        };

        let mut crc_buf = [0u8; 4];
        if reader.read_exact(&mut crc_buf).is_err() {
            break;
        }

        match &chunk_type {
            b"tEXt" => {
                if let Some((keyword, value)) = parse_text_chunk(&chunk_data) {
                    process_keyword(origin, &keyword, &value, verbose, &mut results);
                }
            }
            b"iTXt" => {
                if let Some((keyword, value)) = parse_itxt_chunk(&chunk_data) {
                    process_keyword(origin, &keyword, &value, verbose, &mut results);
                }
            }
            _ => {}
        }
    }

    results
}

fn parse_text_chunk(data: &[u8]) -> Option<(String, String)> {
    let null_pos = data.iter().position(|&b| b == 0)?;
    Some((latin1_or_utf8(&data[..null_pos]), latin1_or_utf8(&data[null_pos + 1..])))
}

/// tEXt is Latin-1 by specification (Pillow writes it that way); some tools store UTF-8
/// instead, so valid UTF-8 is taken as such.
fn latin1_or_utf8(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

fn parse_itxt_chunk(data: &[u8]) -> Option<(String, String)> {
    let kw_end = data.iter().position(|&b| b == 0)?;
    let keyword = String::from_utf8_lossy(&data[..kw_end]).into_owned();

    let mut pos = kw_end + 1;
    if pos + 1 >= data.len() {
        return None;
    }
    let compression_flag = data[pos];
    pos += 2; // skip compression flag + method

    let lang_end = data[pos..].iter().position(|&b| b == 0)?;
    pos += lang_end + 1;

    let trans_end = data[pos..].iter().position(|&b| b == 0)?;
    pos += trans_end + 1;

    let raw = &data[pos..];
    let value = if compression_flag == 1 {
        inflate_zlib(raw)?
    } else {
        String::from_utf8_lossy(raw).into_owned()
    };
    Some((keyword, value))
}

fn inflate_zlib(data: &[u8]) -> Option<String> {
    let mut decoder = ZlibDecoder::new(data);
    let mut buf = Vec::new();
    decoder.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn process_keyword(
    path: &Path,
    keyword: &str,
    value: &str,
    verbose: bool,
    results: &mut Vec<PromptRecord>,
) {
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
        "prompt" | "workflow" => {
            results.extend(super::comfyui_keyword_record(path, keyword, value, verbose));
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

        let records = extract(&mut png_bytes.as_slice(), Path::new("test.png"), false);
        assert_eq!(
            records[0].details_or_default().model.as_deref(),
            Some("base.safetensors")
        );
    }

    #[test]
    fn extracts_comfyui_from_workflow_keyword() {
        let workflow = r#"{"1":{"class_type":"TextEncodeQwenImageEditPlus","inputs":{"prompt":"workflow keyword prompt"}}}"#;
        let mut data = b"workflow\x00".to_vec();
        data.extend_from_slice(workflow.as_bytes());
        let png_bytes = make_png_with_chunks(&[make_png_chunk(b"tEXt", &data)]);

        let records = extract(&mut png_bytes.as_slice(), Path::new("test.png"), false);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].generator, Generator::ComfyUI);
        assert_eq!(records[0].prompt, "workflow keyword prompt");
    }

    #[test]
    fn falls_back_to_a1111_when_prompt_json_invalid() {
        let text = "masterpiece\nNegative prompt: blurry\nSteps: 30, Model: base.safetensors, Seed: 1";
        let mut data = b"prompt\x00".to_vec();
        data.extend_from_slice(text.as_bytes());
        let png_bytes = make_png_with_chunks(&[make_png_chunk(b"tEXt", &data)]);

        let records = extract(&mut png_bytes.as_slice(), Path::new("test.png"), false);
        assert_eq!(records[0].generator, Generator::Unknown);
        assert_eq!(
            records[0].details_or_default().model.as_deref(),
            Some("base.safetensors")
        );
    }
}
