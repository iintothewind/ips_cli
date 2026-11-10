//! Remote (http/https) image input.

use std::collections::HashSet;
use std::io::{self, Cursor, Read};
use std::path::Path;
use std::time::Duration;

use crate::extract::{self, ImageFormat, Stream};
use crate::types::PromptRecord;

/// End-to-end budget per URL: DNS, connect, redirects and body.
const TIMEOUT: Duration = Duration::from_secs(30);
/// Upper bound on bytes read from one response body.
const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

/// True for `http://` / `https://` (scheme compared case-insensitively).
pub fn is_url(s: &str) -> bool {
    ["http://", "https://"]
        .iter()
        .any(|scheme| s.get(..scheme.len()).is_some_and(|p| p.eq_ignore_ascii_case(scheme)))
}

/// Result of parsing a URL list.
#[derive(Debug, PartialEq)]
pub struct UrlList {
    /// Distinct URLs in first-seen order.
    pub urls: Vec<String>,
    /// Non-blank lines that are not http(s) URLs: (1-based line number, trimmed text).
    pub invalid: Vec<(usize, String)>,
}

/// Parse one URL per line. Accepts LF and CRLF, a leading UTF-8 BOM, surrounding
/// whitespace and blank lines.
pub fn parse_url_list(text: &str) -> UrlList {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut seen = HashSet::new();
    let mut list = UrlList { urls: Vec::new(), invalid: Vec::new() };
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !is_url(line) {
            list.invalid.push((index + 1, line.to_string()));
        } else if seen.insert(line) {
            list.urls.push(line.to_string());
        }
    }
    list
}

/// Remembers the first transport error. The container parsers treat a failed read as a
/// truncated file, which would otherwise turn a timeout into "no metadata".
struct Tracked<R> {
    inner: R,
    error: Option<String>,
}

impl<R: Read> Read for Tracked<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let result = self.inner.read(buf);
        if let Err(e) = &result {
            if e.kind() != io::ErrorKind::Interrupted && self.error.is_none() {
                self.error = Some(e.to_string());
            }
        }
        result
    }
}

/// Blocking HTTP client shared by all worker threads (one connection pool).
pub struct Fetcher {
    agent: ureq::Agent,
}

impl Default for Fetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetcher {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .user_agent(concat!("ips/", env!("CARGO_PKG_VERSION")))
            .build();
        Self { agent: ureq::Agent::new_with_config(config) }
    }

    /// GET `url` and extract prompt records from the response body as it streams.
    /// PNG and JPEG parsing stops at the image data, so the rest is never downloaded.
    /// `Err` is a one-line reason: HTTP status, transport error, or not an image.
    pub fn extract(&self, url: &str, verbose: bool) -> Result<Vec<PromptRecord>, String> {
        let response = self.agent.get(url).call().map_err(|e| e.to_string())?;
        let mut body = Tracked {
            inner: response.into_body().into_with_config().limit(MAX_BODY_BYTES).reader(),
            error: None,
        };

        let mut head = Vec::with_capacity(12);
        body.by_ref().take(12).read_to_end(&mut head).map_err(|e| e.to_string())?;
        let format = ImageFormat::sniff(&head)
            .ok_or_else(|| "not a PNG, JPEG, or WebP image".to_string())?;

        let mut source = Stream(Cursor::new(head).chain(&mut body));
        let records = extract::extract_from(format, &mut source, Path::new(url), verbose);
        match body.error {
            Some(reason) => Err(reason),
            None => Ok(records),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url_list_normalizes_lines() {
        let list = parse_url_list(
            "\u{feff}https://a/1.png\r\n\r\n  HTTP://b/2.png  \nnot-a-url\nhttps://a/1.png\n",
        );
        assert_eq!(list.urls, ["https://a/1.png", "HTTP://b/2.png"]);
        assert_eq!(list.invalid, [(4, "not-a-url".to_string())]);
        assert!(!is_url("图片/a.png"));
        assert!(!is_url("ftp://x/a.png"));
    }
}
