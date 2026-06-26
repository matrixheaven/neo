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

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::process::Command;

    pub fn read_clipboard_image() -> Result<ClipboardImage, ClipboardError> {
        if let Ok(out) = Command::new("pngpaste").arg("-").output() {
            if out.status.success() && !out.stdout.is_empty() {
                return Ok(ClipboardImage {
                    bytes: out.stdout,
                    mime_type: "image/png".into(),
                });
            }
        }

        let tmp = std::env::temp_dir().join(format!("neo-clipboard-{}.png", std::process::id()));
        let tmp_tiff =
            std::env::temp_dir().join(format!("neo-clipboard-{}.tiff", std::process::id()));

        // Helper: run a JXA script that reads from the pasteboard.
        let read_pasteboard = |type_str: &str, path: &std::path::Path| -> bool {
            let script = format!(
                "ObjC.import('AppKit'); var pb = $.NSPasteboard.generalPasteboard; var data = pb.dataForType({type_str}); if (data && !data.isNil()) {{ data.writeToFileAtomically({:?}, true); }} else {{ $.exit(1); }}",
                path.to_str().unwrap_or("")
            );
            Command::new("osascript")
                .args(["-l", "JavaScript", "-e", &script])
                .output()
                .map(|out| out.status.success() && path.exists())
                .unwrap_or(false)
        };

        // Strategy 1: Read PNG directly (fast path).
        if read_pasteboard("$.NSPasteboardTypePNG", &tmp) {
            if let Ok(bytes) = std::fs::read(&tmp) {
                let _ = std::fs::remove_file(&tmp);
                if !bytes.is_empty() {
                    return Ok(ClipboardImage {
                        bytes,
                        mime_type: "image/png".into(),
                    });
                }
            }
        }

        // Strategy 2: Read TIFF and convert to PNG via sips.
        // macOS screenshots and image copies often put the full-res data in
        // the TIFF pasteboard type, with only a tiny placeholder in PNG.
        if read_pasteboard("$.NSPasteboardTypeTIFF", &tmp_tiff) {
            // Convert TIFF → PNG using the built-in macOS `sips` tool.
            let convert = Command::new("sips")
                .args(["-s", "format", "png"])
                .arg(&tmp_tiff)
                .args(["--out", tmp.to_str().unwrap_or("")])
                .output();
            let _ = std::fs::remove_file(&tmp_tiff);
            if let Ok(out) = convert
                && out.status.success()
            {
                if let Ok(bytes) = std::fs::read(&tmp) {
                    let _ = std::fs::remove_file(&tmp);
                    if !bytes.is_empty() {
                        return Ok(ClipboardImage {
                            bytes,
                            mime_type: "image/png".into(),
                        });
                    }
                }
            }
        }

        Err(ClipboardError::NoImage)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::process::Command;

    pub fn read_clipboard_image() -> Result<ClipboardImage, ClipboardError> {
        let candidates: [(&str, Vec<&str>, &str); 2] = [
            ("wl-paste", vec!["--type", "image/png"], "image/png"),
            (
                "xclip",
                vec!["-selection", "clipboard", "-t", "image/png", "-o"],
                "image/png",
            ),
        ];
        for (cmd, args, mime) in candidates {
            if let Ok(out) = Command::new(cmd).args(&args).output() {
                if out.status.success() && !out.stdout.is_empty() {
                    return Ok(ClipboardImage {
                        bytes: out.stdout,
                        mime_type: mime.into(),
                    });
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
        Ok(ClipboardImage {
            bytes,
            mime_type: "image/png".into(),
        })
    }
}
