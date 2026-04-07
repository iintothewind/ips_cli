pub mod comfyui;
pub mod exif;
pub mod jpeg;
pub mod png;
pub mod webp;

use std::path::Path;
use crate::types::PromptRecord;

/// Extract all prompt records from an image file.
/// Returns an empty Vec on unsupported format or error (errors are logged when verbose=true).
pub fn extract_prompt(path: &Path, verbose: bool) -> Vec<PromptRecord> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase());

    match ext.as_deref() {
        Some("png") => png::extract(path, verbose),
        Some("jpg") | Some("jpeg") => jpeg::extract(path, verbose),
        Some("webp") => webp::extract(path, verbose),
        _ => {
            // Unsupported extension — silently skip
            vec![]
        }
    }
}
