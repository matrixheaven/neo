//! Cross-platform clipboard image reading for image paste.

use thiserror::Error;

/// Image bytes read from the system clipboard.
pub struct ClipboardImage {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

/// Errors that can occur when reading an image from the clipboard.
#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("no image in clipboard")]
    NoImage,
    #[error("clipboard read failed: {0}")]
    ReadFailed(String),
}

/// Read plain text from the system clipboard. Used as a fallback when Ctrl+V
/// is pressed but no image is available.
pub fn read_text_clipboard() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("pbpaste")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .filter(|s| !s.is_empty())
    }
    #[cfg(target_os = "linux")]
    {
        for (cmd, args) in [
            ("wl-paste", &["--no-newline"][..]),
            ("xclip", &["-selection", "clipboard", "-o"][..]),
        ] {
            if let Ok(out) = std::process::Command::new(cmd).args(args).output() {
                if out.status.success() {
                    if let Ok(text) = String::from_utf8(out.stdout)
                        && !text.is_empty()
                    {
                        return Some(text);
                    }
                }
            }
        }
        None
    }
    #[cfg(target_os = "windows")]
    {
        let script =
            "Add-Type -AssemblyName System.Windows.Forms; [Windows.Forms.Clipboard]::GetText()";
        std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", script])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .filter(|s| !s.is_empty())
    }
}

/// Read an image from the system clipboard, if one is available.
pub fn read_clipboard_image() -> Result<ClipboardImage, ClipboardError> {
    #[cfg(target_os = "macos")]
    return macos::read_clipboard_image();
    #[cfg(target_os = "linux")]
    return linux::read_clipboard_image();
    #[cfg(target_os = "windows")]
    return windows::read_clipboard_image();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return Err(ClipboardError::ReadFailed("unsupported platform".into()));
}

/// Detect the MIME type of image bytes using magic-byte sniffing.
/// Returns `None` if the bytes are not a recognized image format.
fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        return Some("image/png");
    }
    if bytes.len() >= 3 && &bytes[..3] == b"\xff\xd8\xff" {
        return Some("image/jpeg");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.len() >= 6 && (&bytes[..6] == b"GIF87a" || &bytes[..6] == b"GIF89a") {
        return Some("image/gif");
    }
    // TIFF: big-endian (MM\x00\x2a) or little-endian (II\x2a\x00)
    if bytes.len() >= 4 && (&bytes[..4] == b"MM\x00\x2a" || &bytes[..4] == b"II\x2a\x00") {
        return Some("image/tiff");
    }
    None
}

/// Whether the MIME type is one that providers accept for vision (base64).
fn is_vision_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    )
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::process::Command;

    pub fn read_clipboard_image() -> Result<ClipboardImage, ClipboardError> {
        // Read both PNG and TIFF from the pasteboard, then pick whichever has
        // more data. macOS screenshots often put the full-res image in TIFF
        // and only a tiny placeholder in PNG (or vice-versa).
        let png_bytes = read_pasteboard_type("$.NSPasteboardTypePNG");
        let tiff_bytes = read_pasteboard_type("$.NSPasteboardTypeTIFF");

        tracing::debug!(
            "clipboard: png={} bytes, tiff={} bytes",
            png_bytes.as_deref().map_or(0, <[u8]>::len),
            tiff_bytes.as_deref().map_or(0, <[u8]>::len),
        );

        let bytes = tiff_bytes
            .as_ref()
            .map(|t| (t.len(), t, "tiff"))
            .into_iter()
            .chain(png_bytes.as_ref().map(|p| (p.len(), p, "png")))
            .max_by_key(|(len, _, _)| *len)
            .map(|(_, bytes, _)| bytes);

        let Some(bytes) = bytes else {
            return Err(ClipboardError::NoImage);
        };
        if bytes.is_empty() {
            return Err(ClipboardError::NoImage);
        }

        // Detect the actual format from magic bytes.
        let mime = detect_image_mime(bytes);

        match mime {
            Some(m) if is_vision_mime(m) => Ok(ClipboardImage {
                bytes: bytes.clone(),
                mime_type: m.to_owned(),
            }),
            Some("image/tiff") => {
                // TIFF is not supported by providers — convert to PNG.
                let png = tiff_to_png(bytes)?;
                Ok(ClipboardImage {
                    bytes: png,
                    mime_type: "image/png".into(),
                })
            }
            _ => Err(ClipboardError::NoImage),
        }
    }

    /// Read raw bytes for a given NSPasteboard type via JXA.
    fn read_pasteboard_type(pasteboard_type: &str) -> Option<Vec<u8>> {
        let suffix = if pasteboard_type.contains("PNG") {
            "png"
        } else {
            "tiff"
        };
        let tmp = std::env::temp_dir().join(format!("neo-clip-{suffix}.dat"));
        let _ = std::fs::remove_file(&tmp);

        let script = format!(
            "ObjC.import('AppKit'); var pb = $.NSPasteboard.generalPasteboard; var data = pb.dataForType({pasteboard_type}); var ok = false; if (data && !data.isNil()) {{ ok = data.writeToFileAtomically({:?}, true); }} ok;",
            tmp.to_str().unwrap_or("")
        );
        let out = Command::new("osascript")
            .args(["-l", "JavaScript", "-e", &script])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        tracing::debug!(
            "{pasteboard_type}: exit={} stdout={:?} stderr={:?} file_exists={}",
            out.status,
            stdout.trim(),
            String::from_utf8_lossy(&out.stderr).trim(),
            tmp.exists(),
        );
        if !tmp.exists() {
            return None;
        }
        let bytes = std::fs::read(&tmp).ok()?;
        let _ = std::fs::remove_file(&tmp);
        (!bytes.is_empty()).then_some(bytes)
    }

    /// Convert TIFF bytes to PNG using the built-in macOS `sips` tool.
    fn tiff_to_png(tiff_bytes: &[u8]) -> Result<Vec<u8>, ClipboardError> {
        let in_path = std::env::temp_dir().join(format!("neo-clip-{}.tiff", std::process::id()));
        let out_path = std::env::temp_dir().join(format!("neo-clip-{}.png", std::process::id()));

        std::fs::write(&in_path, tiff_bytes)
            .map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;

        let out = Command::new("sips")
            .args(["-s", "format", "png"])
            .arg(&in_path)
            .args(["--out", out_path.to_str().unwrap_or("")])
            .output();

        let _ = std::fs::remove_file(&in_path);

        match out {
            Ok(o) if o.status.success() => {
                let png = std::fs::read(&out_path)
                    .map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
                let _ = std::fs::remove_file(&out_path);
                Ok(png)
            }
            Ok(o) => Err(ClipboardError::ReadFailed(format!(
                "sips conversion failed: {}",
                String::from_utf8_lossy(&o.stderr)
            ))),
            Err(e) => Err(ClipboardError::ReadFailed(e.to_string())),
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    pub fn read_clipboard_image() -> Result<ClipboardImage, ClipboardError> {
        let candidates: [(&str, &[&str]); 2] = [
            ("wl-paste", &["--type", "image/png"]),
            (
                "xclip",
                &["-selection", "clipboard", "-t", "image/png", "-o"],
            ),
        ];
        for (cmd, args) in candidates {
            if let Ok(out) = Command::new(cmd).args(args).output() {
                if out.status.success() && !out.stdout.is_empty() {
                    let mime = detect_image_mime(&out.stdout);
                    match mime {
                        Some(m) if is_vision_mime(m) => {
                            return Ok(ClipboardImage {
                                bytes: out.stdout,
                                mime_type: m.to_owned(),
                            });
                        }
                        _ => continue,
                    }
                }
            }
        }
        Err(ClipboardError::NoImage)
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use std::process::Command;

    pub fn read_clipboard_image() -> Result<ClipboardImage, ClipboardError> {
        let tmp = std::env::temp_dir().join(format!("neo-clipboard-{}.png", std::process::id()));
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $img = [Windows.Forms.Clipboard]::GetImage(); if ($img -eq $null) {{ exit 1 }}; $img.Save({:?}, [System.Drawing.Imaging.ImageFormat]::Png);",
            tmp.to_str().unwrap_or("")
        );
        let out = Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", &script])
            .output()
            .map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
        if !out.status.success() {
            return Err(ClipboardError::NoImage);
        }
        let bytes = std::fs::read(&tmp).map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
        let _ = std::fs::remove_file(&tmp);
        let mime = detect_image_mime(&bytes).unwrap_or("image/png");
        Ok(ClipboardImage {
            bytes,
            mime_type: mime.to_owned(),
        })
    }
}
