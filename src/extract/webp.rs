use std::path::Path;

use crate::types::PromptRecord;
use super::{a1111, jpeg, ByteSource};

pub(super) const RIFF_MAGIC: &[u8; 4] = b"RIFF";
pub(super) const WEBP_MAGIC: &[u8; 4] = b"WEBP";

pub fn extract<R: ByteSource>(reader: &mut R, origin: &Path, verbose: bool) -> Vec<PromptRecord> {
    let mut header = [0u8; 12];
    if reader.read_exact(&mut header).is_err() {
        if verbose {
            eprintln!("ips: {}: file too small to be a WebP", origin.display());
        }
        return vec![];
    }

    if &header[0..4] != RIFF_MAGIC || &header[8..12] != WEBP_MAGIC {
        if verbose {
            eprintln!("ips: {}: not a valid WebP", origin.display());
        }
        return vec![];
    }

    let mut results = Vec::new();

    loop {
        let mut chunk_header = [0u8; 8];
        if reader.read_exact(&mut chunk_header).is_err() {
            break;
        }

        let chunk_id = &chunk_header[..4];
        let chunk_size = u32::from_le_bytes([
            chunk_header[4], chunk_header[5], chunk_header[6], chunk_header[7],
        ]) as usize;
        let padded_size = chunk_size + (chunk_size & 1);

        match chunk_id {
            b"XMP " => {
                let chunk_data = match super::read_exact_vec(reader, chunk_size) {
                    Ok(data) => data,
                    Err(_) => {
                        if verbose {
                            eprintln!("ips: {}: truncated XMP chunk", origin.display());
                        }
                        break;
                    }
                };
                if chunk_size & 1 != 0 {
                    let _ = reader.skip(1);
                }
                if let Some(prompt) = jpeg::extract_xmp_description(&chunk_data) {
                    let generator = jpeg::detect_xmp_generator(&chunk_data);
                    results.push(PromptRecord::with_details(
                        origin.to_path_buf(),
                        prompt.clone(),
                        generator,
                        "XMP",
                        a1111::extract_details(&prompt),
                    ));
                }
            }
            b"EXIF" => {
                let chunk_data = match super::read_exact_vec(reader, chunk_size) {
                    Ok(data) => data,
                    Err(_) => {
                        if verbose {
                            eprintln!("ips: {}: truncated EXIF chunk", origin.display());
                        }
                        break;
                    }
                };
                if chunk_size & 1 != 0 {
                    let _ = reader.skip(1);
                }
                results.extend(super::exif_records(origin, &chunk_data, verbose));
            }
            _ => {
                if reader.skip(padded_size as u64).is_err() {
                    break;
                }
            }
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::Stream;

    fn make_webp_with_xmp(xmp: &str) -> Vec<u8> {
        let xmp_bytes = xmp.as_bytes();
        let chunk_size = xmp_bytes.len() as u32;
        let padded_size = (xmp_bytes.len() + (xmp_bytes.len() & 1)) as u32;
        let riff_size = (4 + 8 + padded_size).to_le_bytes();

        let mut webp = Vec::new();
        webp.extend_from_slice(b"RIFF");
        webp.extend_from_slice(&riff_size);
        webp.extend_from_slice(b"WEBP");
        webp.extend_from_slice(b"XMP ");
        webp.extend_from_slice(&chunk_size.to_le_bytes());
        webp.extend_from_slice(xmp_bytes);
        if xmp_bytes.len() % 2 != 0 {
            webp.push(0);
        }
        webp
    }

    #[test]
    fn extracts_xmp_from_webp() {
        let xmp = r#"<rdf:RDF>
  <rdf:Description>
    <dc:description>
      <rdf:Alt>
        <rdf:li xml:lang="x-default">sunset landscape, watercolor</rdf:li>
      </rdf:Alt>
    </dc:description>
  </rdf:Description>
</rdf:RDF>"#;
        let webp_bytes = make_webp_with_xmp(xmp);

        let records = extract(&mut Stream(webp_bytes.as_slice()), Path::new("test.webp"), false);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].prompt, "sunset landscape, watercolor");
    }
}
