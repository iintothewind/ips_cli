use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::types::{Generator, PromptRecord};
use super::{a1111, exif, jpeg};

const RIFF_MAGIC: &[u8; 4] = b"RIFF";
const WEBP_MAGIC: &[u8; 4] = b"WEBP";

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

    let mut header = [0u8; 12];
    if reader.read_exact(&mut header).is_err() {
        if verbose {
            eprintln!("ips: {}: file too small to be a WebP", path.display());
        }
        return vec![];
    }

    if &header[0..4] != RIFF_MAGIC || &header[8..12] != WEBP_MAGIC {
        if verbose {
            eprintln!("ips: {}: not a valid WebP", path.display());
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
                let mut chunk_data = vec![0u8; chunk_size];
                if reader.read_exact(&mut chunk_data).is_err() {
                    if verbose {
                        eprintln!("ips: {}: truncated XMP chunk", path.display());
                    }
                    break;
                }
                if chunk_size & 1 != 0 {
                    let _ = reader.seek(SeekFrom::Current(1));
                }
                if let Some(prompt) = jpeg::extract_xmp_description(&chunk_data) {
                    let generator = jpeg::detect_xmp_generator(&chunk_data);
                    results.push(PromptRecord::with_details(
                        path.to_path_buf(),
                        prompt.clone(),
                        generator,
                        "XMP",
                        a1111::extract_details(&prompt),
                    ));
                }
            }
            b"EXIF" => {
                let mut chunk_data = vec![0u8; chunk_size];
                if reader.read_exact(&mut chunk_data).is_err() {
                    if verbose {
                        eprintln!("ips: {}: truncated EXIF chunk", path.display());
                    }
                    break;
                }
                if chunk_size & 1 != 0 {
                    let _ = reader.seek(SeekFrom::Current(1));
                }
                if let Some(prompt) = exif::extract_user_comment(&chunk_data) {
                    results.push(PromptRecord::with_details(
                        path.to_path_buf(),
                        prompt.clone(),
                        Generator::Unknown,
                        "UserComment",
                        a1111::extract_details(&prompt),
                    ));
                }
            }
            _ => {
                if reader.seek(SeekFrom::Current(padded_size as i64)).is_err() {
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

        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.webp");
        std::fs::write(&path, &webp_bytes).unwrap();

        let records = extract(&path, false);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].prompt, "sunset landscape, watercolor");
    }
}
