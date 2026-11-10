pub mod a1111;
pub mod comfyui;
pub mod exif;
pub mod jpeg;
pub mod json_repair;
pub mod png;
pub mod webp;

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

use crate::types::{Generator, PromptRecord};

/// Outcome of reading a metadata value as ComfyUI JSON.
enum ComfyJson {
    /// The JSON (possibly recovered from a truncated document) yielded a prompt record.
    Record(PromptRecord),
    /// Valid JSON without any prompt, model or LoRA.
    Empty,
    /// Not JSON, and nothing recoverable.
    NotJson,
}

/// Parse `text` as a ComfyUI API prompt or UI workflow. Truncated JSON is recovered up to
/// its last complete value.
fn comfyui_json(origin: &Path, key: &str, text: &str, verbose: bool) -> ComfyJson {
    let Some((json, repaired)) = json_repair::parse_lenient(text) else {
        return ComfyJson::NotJson;
    };
    if repaired && verbose {
        eprintln!("ips: {}: {}: truncated JSON, read up to the cut", origin.display(), key);
    }
    let (prompt, details) = comfyui::extract_workflow(&json);
    if comfyui::has_extractable_content(&prompt, &details) {
        ComfyJson::Record(PromptRecord::with_details(
            origin.to_path_buf(),
            prompt,
            Generator::ComfyUI,
            key,
            details,
        ))
    } else {
        ComfyJson::Empty
    }
}

/// Record for a value stored under ComfyUI's own `prompt` / `workflow` keys (PNG text
/// chunks, EXIF `key:{json}` tags). Text that is not JSON is kept as plain parameters text.
pub(crate) fn comfyui_keyword_record(
    origin: &Path,
    key: &str,
    text: &str,
    verbose: bool,
) -> Option<PromptRecord> {
    match comfyui_json(origin, key, text, verbose) {
        ComfyJson::Record(record) => Some(record),
        ComfyJson::Empty => None,
        ComfyJson::NotJson if text.trim().is_empty() => None,
        ComfyJson::NotJson => Some(PromptRecord::with_details(
            origin.to_path_buf(),
            text.to_string(),
            Generator::Unknown,
            key,
            a1111::extract_details(text),
        )),
    }
}

/// Records from an EXIF block (JPEG APP1, WebP EXIF chunk): the UserComment, then ComfyUI
/// graphs stored as `prompt:{…}` / `workflow:{…}` IFD0 tags.
pub(crate) fn exif_records(origin: &Path, exif_data: &[u8], verbose: bool) -> Vec<PromptRecord> {
    let mut records = Vec::new();
    if let Some(text) = exif::extract_user_comment(exif_data) {
        records.push(text_record(origin, "UserComment", text, Generator::Unknown, verbose));
    }
    for (key, text) in exif::extract_comfyui_tags(exif_data) {
        records.extend(comfyui_keyword_record(origin, &key, &text, verbose));
    }
    records
}

/// Record for free-form text metadata (JPEG COM, EXIF UserComment). A JSON object is read as
/// ComfyUI data, as written by JPEG save nodes that store `{"prompt": …, "workflow": …}`;
/// anything else is A1111-style parameters text attributed to `generator`.
pub(crate) fn text_record(
    origin: &Path,
    key: &str,
    text: String,
    generator: Generator,
    verbose: bool,
) -> PromptRecord {
    if text.trim_start().starts_with('{') {
        if let ComfyJson::Record(record) = comfyui_json(origin, key, &text, verbose) {
            return record;
        }
    }
    let details = a1111::extract_details(&text);
    PromptRecord::with_details(origin.to_path_buf(), text, generator, key, details)
}

/// Container formats ips can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    WebP,
}

impl ImageFormat {
    /// Detect by file extension (local files).
    pub fn from_extension(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "webp" => Some(Self::WebP),
            _ => None,
        }
    }

    /// Detect by magic bytes. `head` is the first 12 bytes of the stream, fewer if it is shorter.
    pub fn sniff(head: &[u8]) -> Option<Self> {
        if head.starts_with(png::PNG_SIGNATURE) {
            Some(Self::Png)
        } else if head.starts_with(&jpeg::SOI_MARKER) {
            Some(Self::Jpeg)
        } else if head.len() >= 12
            && head[..4] == webp::RIFF_MAGIC[..]
            && head[8..12] == webp::WEBP_MAGIC[..]
        {
            Some(Self::WebP)
        } else {
            None
        }
    }
}

/// Input the container parsers read from. `skip` lets seekable inputs jump over
/// image data instead of reading it.
pub trait ByteSource: Read {
    fn skip(&mut self, n: u64) -> io::Result<()>;
}

impl ByteSource for BufReader<File> {
    fn skip(&mut self, n: u64) -> io::Result<()> {
        self.seek_relative(n as i64)
    }
}

/// Non-seekable input (HTTP body, in-memory bytes): skipping reads and discards.
pub struct Stream<R>(pub R);

impl<R: Read> Read for Stream<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl<R: Read> ByteSource for Stream<R> {
    fn skip(&mut self, n: u64) -> io::Result<()> {
        let skipped = io::copy(&mut self.0.by_ref().take(n), &mut io::sink())?;
        if skipped == n {
            Ok(())
        } else {
            Err(io::ErrorKind::UnexpectedEof.into())
        }
    }
}

/// Read exactly `len` bytes. The buffer grows with the bytes actually received, so a
/// bogus length field cannot force a multi-gigabyte allocation.
pub(crate) fn read_exact_vec<R: Read>(reader: &mut R, len: usize) -> io::Result<Vec<u8>> {
    const MAX_PREALLOC: usize = 1 << 20;
    let mut buf = Vec::with_capacity(len.min(MAX_PREALLOC));
    reader.by_ref().take(len as u64).read_to_end(&mut buf)?;
    if buf.len() == len {
        Ok(buf)
    } else {
        Err(io::ErrorKind::UnexpectedEof.into())
    }
}

/// Extract all prompt records from a local image file.
/// Returns an empty Vec on unsupported format or error (errors are logged when verbose=true).
pub fn extract_prompt(path: &Path, verbose: bool) -> Vec<PromptRecord> {
    let Some(format) = ImageFormat::from_extension(path) else {
        if verbose {
            eprintln!("ips: {}: unsupported format", path.display());
        }
        return Vec::new();
    };
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            if verbose {
                eprintln!("ips: cannot read {}: {}", path.display(), e);
            }
            return Vec::new();
        }
    };
    extract_from(format, &mut BufReader::new(file), path, verbose)
}

/// Run the parser for `format` over `reader`. `origin` becomes each record's `path` and
/// names the input in verbose diagnostics: the file path, or the URL for remote images.
pub fn extract_from<R: ByteSource>(
    format: ImageFormat,
    reader: &mut R,
    origin: &Path,
    verbose: bool,
) -> Vec<PromptRecord> {
    match format {
        ImageFormat::Png => png::extract(reader, origin, verbose),
        ImageFormat::Jpeg => jpeg::extract(reader, origin, verbose),
        ImageFormat::WebP => webp::extract(reader, origin, verbose),
    }
}
