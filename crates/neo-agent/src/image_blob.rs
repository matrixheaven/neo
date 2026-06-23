//! Image blob storage and minimal dimension detection.

use sha2::{Digest, Sha256};
use std::path::Path;

/// Save image bytes to the session's `blobs/` directory, keyed by SHA-256.
/// Duplicate content is not written twice. Returns the SHA-256 key.
pub fn save_image_blob(
    session_dir: &Path,
    bytes: &[u8],
    mime_type: &str,
) -> anyhow::Result<String> {
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let ext = mime_to_extension(mime_type).unwrap_or("bin");
    let blob_dir = session_dir.join("blobs");
    std::fs::create_dir_all(&blob_dir)?;
    let path = blob_dir.join(format!("{}.{}", sha256, ext));
    if !path.exists() {
        std::fs::write(&path, bytes)?;
    }
    Ok(sha256)
}

fn mime_to_extension(mime: &str) -> Option<&str> {
    match mime {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        _ => None,
    }
}

/// Detect image dimensions from raw bytes for common formats.
pub fn detect_image_dimensions(bytes: &[u8], mime_type: &str) -> Option<(u32, u32)> {
    match mime_type {
        "image/png" => png_dimensions(bytes),
        "image/jpeg" => jpeg_dimensions(bytes),
        "image/gif" => gif_dimensions(bytes),
        "image/webp" => webp_dimensions(bytes),
        _ => None,
    }
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 {
        return None;
    }
    if &bytes[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut i = 0;
    while i + 8 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        if marker == 0xD8 || marker == 0xD9 {
            i += 2;
            continue;
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        if i + 4 >= bytes.len() {
            break;
        }
        let segment_len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if marker == 0xC0 || marker == 0xC1 || marker == 0xC2 {
            if i + 9 >= bytes.len() {
                break;
            }
            let height = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
            let width = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
            return Some((width, height));
        }
        i += 2 + segment_len;
    }
    None
}

fn gif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 10 {
        return None;
    }
    if &bytes[0..3] != b"GIF" {
        return None;
    }
    let width = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
    let height = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
    Some((width, height))
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 30 {
        return None;
    }
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    if &bytes[12..16] == b"VP8 " {
        // Simple lossy VP8: bytes 26-29 hold width/height.
        let width = u16::from_le_bytes([bytes[26], bytes[27]]) as u32 & 0x3FFF;
        let height = u16::from_le_bytes([bytes[28], bytes[29]]) as u32 & 0x3FFF;
        return Some((width, height));
    }
    if &bytes[12..16] == b"VP8L" {
        // Lossless VP8L: dimensions are in a 28-bit field starting at byte 21.
        if bytes.len() < 25 {
            return None;
        }
        let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
        let width = (bits & 0x3FFF) + 1;
        let height = ((bits >> 14) & 0x3FFF) + 1;
        return Some((width, height));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_dimensions_detects_size() {
        // Minimal 1x1 PNG.
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x02\x00\x00\x00\x90wS\xde";
        assert_eq!(png_dimensions(png), Some((1, 1)));
    }

    #[test]
    fn gif_dimensions_detects_size() {
        let gif = b"GIF89a\x01\x00\x01\x00\x00\x00\x00!";
        assert_eq!(gif_dimensions(gif), Some((1, 1)));
    }

    #[test]
    fn save_image_blob_deduplicates() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bytes = b"fake image";
        let blob1 = save_image_blob(temp.path(), bytes, "image/png").expect("save blob");
        let blob2 = save_image_blob(temp.path(), bytes, "image/png").expect("save blob");
        assert_eq!(blob1, blob2);
    }
}
