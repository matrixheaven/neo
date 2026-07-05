//! Cross-platform clipboard image reading for image paste.

#[cfg(target_os = "linux")]
use std::process::Command;

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
            if let Ok(out) = std::process::Command::new(cmd).args(args).output()
                && out.status.success()
                && let Ok(text) = String::from_utf8(out.stdout)
                && !text.is_empty()
            {
                return Some(text);
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
    use super::{ClipboardError, ClipboardImage, detect_image_mime, is_vision_mime};
    use std::io::Write as _;
    use std::process::Command;

    pub fn read_clipboard_image() -> Result<ClipboardImage, ClipboardError> {
        // Read both PNG and TIFF from the pasteboard, then pick whichever has
        // more data. macOS screenshots often put the full-res image in TIFF
        // and only a tiny placeholder in PNG (or vice-versa).
        let png_bytes = read_pasteboard_type("$.NSPasteboardTypePNG")?;
        let tiff_bytes = read_pasteboard_type("$.NSPasteboardTypeTIFF")?;

        tracing::debug!(
            "clipboard: png={:?}, tiff={:?}",
            png_bytes.as_ref().map(Vec::len),
            tiff_bytes.as_ref().map(Vec::len),
        );

        let bytes = match (png_bytes, tiff_bytes) {
            (Some(png), Some(tiff)) => {
                if png.len() >= tiff.len() {
                    png
                } else {
                    tiff
                }
            }
            (Some(png), None) => png,
            (None, Some(tiff)) => tiff,
            (None, None) => return Err(ClipboardError::NoImage),
        };

        if bytes.is_empty() {
            return Err(ClipboardError::NoImage);
        }

        // Detect the actual format from magic bytes.
        let mime = detect_image_mime(&bytes);

        match mime {
            Some(m) if is_vision_mime(m) => Ok(ClipboardImage {
                bytes,
                mime_type: m.to_owned(),
            }),
            Some("image/tiff") => {
                // TIFF is not supported by providers — convert to PNG.
                let png = tiff_to_png(&bytes)?;
                Ok(ClipboardImage {
                    bytes: png,
                    mime_type: "image/png".into(),
                })
            }
            _ => Err(ClipboardError::NoImage),
        }
    }

    /// Read raw bytes for a given `NSPasteboard` type via JXA.
    ///
    /// Returns `Ok(Some(bytes))` when image data of that type is present,
    /// `Ok(None)` when the pasteboard simply does not contain that type, and
    /// `Err` for unexpected failures (e.g. the temporary path is not valid
    /// UTF-8 or cannot be created).
    fn read_pasteboard_type(pasteboard_type: &str) -> Result<Option<Vec<u8>>, ClipboardError> {
        let suffix = if pasteboard_type.contains("PNG") {
            "png"
        } else {
            "tiff"
        };
        let tmp = tempfile::Builder::new()
            .suffix(&format!(".{suffix}"))
            .tempfile_in(std::env::temp_dir())
            .map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
        let tmp_path = tmp.into_temp_path();
        let tmp_path_str = tmp_path.to_str().ok_or_else(|| {
            ClipboardError::ReadFailed("clipboard temporary path is not valid UTF-8".into())
        })?;

        let script = format!(
            "ObjC.import('AppKit'); var pb = $.NSPasteboard.generalPasteboard; var data = pb.dataForType({pasteboard_type}); var ok = false; if (data && !data.isNil()) {{ ok = data.writeToFileAtomically({tmp_path_str:?}, true); }} ok;"
        );
        let out = Command::new("osascript")
            .args(["-l", "JavaScript", "-e", &script])
            .output()
            .map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        tracing::debug!(
            "{pasteboard_type}: exit={} stdout={:?} stderr={:?} file_exists={}",
            out.status,
            stdout.trim(),
            String::from_utf8_lossy(&out.stderr).trim(),
            tmp_path.exists(),
        );
        if !tmp_path.exists() {
            return Ok(None);
        }
        let bytes =
            std::fs::read(&tmp_path).map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
        Ok((!bytes.is_empty()).then_some(bytes))
        // `tmp_path` drops here and deletes the temporary file.
    }

    /// Convert TIFF bytes to PNG using the built-in macOS `sips` tool.
    fn tiff_to_png(tiff_bytes: &[u8]) -> Result<Vec<u8>, ClipboardError> {
        let mut in_tmp = tempfile::Builder::new()
            .suffix(".tiff")
            .tempfile_in(std::env::temp_dir())
            .map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
        in_tmp
            .write_all(tiff_bytes)
            .map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
        let in_path = in_tmp.into_temp_path();
        let in_path_str = in_path.to_str().ok_or_else(|| {
            ClipboardError::ReadFailed("clipboard temporary path is not valid UTF-8".into())
        })?;

        let out_tmp = tempfile::Builder::new()
            .suffix(".png")
            .tempfile_in(std::env::temp_dir())
            .map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
        let out_path = out_tmp.into_temp_path();
        let out_path_str = out_path.to_str().ok_or_else(|| {
            ClipboardError::ReadFailed("clipboard temporary path is not valid UTF-8".into())
        })?;

        let out = Command::new("sips")
            .args(["-s", "format", "png"])
            .arg(in_path_str)
            .args(["--out", out_path_str])
            .output();

        match out {
            Ok(o) if o.status.success() => {
                let png = std::fs::read(&out_path)
                    .map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
                Ok(png)
            }
            Ok(o) => Err(ClipboardError::ReadFailed(format!(
                "sips conversion failed: {}",
                String::from_utf8_lossy(&o.stderr)
            ))),
            Err(e) => Err(ClipboardError::ReadFailed(e.to_string())),
        }
        // `in_path` and `out_path` drop here and delete the temporary files.
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{ClipboardError, ClipboardImage, Command, detect_image_mime, is_vision_mime};

    pub fn read_clipboard_image() -> Result<ClipboardImage, ClipboardError> {
        let candidates: [(&str, &[&str]); 2] = [
            ("wl-paste", &["--type", "image/png"]),
            (
                "xclip",
                &["-selection", "clipboard", "-t", "image/png", "-o"],
            ),
        ];

        // Track the last real error — if a command exists but fails to execute
        // (e.g. permission denied), we surface it instead of silently returning
        // `NoImage`. A `NotFound` is expected: only one of wl-paste/xclip will
        // be installed depending on the display server (Wayland vs X11).
        let mut spawn_error: Option<String> = None;

        for (cmd, args) in candidates {
            let out = match Command::new(cmd).args(args).output() {
                Ok(out) => out,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    spawn_error = Some(format!("{cmd}: {e}"));
                    continue;
                }
            };

            if out.status.success()
                && !out.stdout.is_empty()
                && let Some(m) = detect_image_mime(&out.stdout)
                && is_vision_mime(m)
            {
                return Ok(ClipboardImage {
                    bytes: out.stdout,
                    mime_type: m.to_owned(),
                });
            }
        }

        match spawn_error {
            Some(msg) => Err(ClipboardError::ReadFailed(msg)),
            None => Err(ClipboardError::NoImage),
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use std::process::Command;

    pub fn read_clipboard_image() -> Result<ClipboardImage, ClipboardError> {
        let tmp = tempfile::Builder::new()
            .suffix(".png")
            .tempfile_in(std::env::temp_dir())
            .map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
        let tmp_path = tmp.into_temp_path();
        let tmp_path_str = tmp_path.to_str().ok_or_else(|| {
            ClipboardError::ReadFailed("clipboard temporary path is not valid UTF-8".into())
        })?;

        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $img = [Windows.Forms.Clipboard]::GetImage(); if ($img -eq $null) {{ exit 1 }}; $img.Save({tmp_path_str:?}, [System.Drawing.Imaging.ImageFormat]::Png);"
        );
        let out = Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", &script])
            .output()
            .map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
        if !out.status.success() {
            return Err(ClipboardError::NoImage);
        }
        let bytes =
            std::fs::read(&tmp_path).map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
        let mime = detect_image_mime(&bytes).unwrap_or("image/png");
        Ok(ClipboardImage {
            bytes,
            mime_type: mime.to_owned(),
        })
        // `tmp_path` drops here and deletes the temporary file.
    }
}
