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
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn is_vision_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    )
}

#[cfg(any(target_os = "linux", test))]
fn linux_output_error(cmd: &str, status: &str, stderr: &[u8]) -> String {
    let stderr = &stderr[..stderr.len().min(512)];
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("{cmd} exited with {status}")
    } else {
        format!("{cmd} exited with {status}: {stderr}")
    }
}

#[cfg(any(target_os = "linux", test))]
fn classify_linux_image_output(
    cmd: &str,
    success: bool,
    status: &str,
    stdout: Vec<u8>,
    stderr: &[u8],
) -> Result<ClipboardImage, String> {
    if !success {
        return Err(linux_output_error(cmd, status, stderr));
    }
    let Some(mime) = detect_image_mime(&stdout).filter(|mime| is_vision_mime(mime)) else {
        return Err(format!("{cmd} returned invalid image data"));
    };
    Ok(ClipboardImage {
        bytes: stdout,
        mime_type: mime.to_owned(),
    })
}

#[cfg(any(target_os = "linux", test))]
fn linux_targets_include_png(stdout: &[u8]) -> bool {
    stdout
        .split(|byte| *byte == b'\n')
        .any(|target| target.trim_ascii() == b"image/png")
}

#[cfg(any(target_os = "linux", test))]
fn linux_clipboard_failure(
    image_error: Option<String>,
    confirmed_no_image: bool,
    probe_error: Option<String>,
) -> ClipboardError {
    if let Some(error) = image_error {
        ClipboardError::ReadFailed(error)
    } else if confirmed_no_image {
        ClipboardError::NoImage
    } else if let Some(error) = probe_error {
        ClipboardError::ReadFailed(error)
    } else {
        ClipboardError::ReadFailed(
            "no clipboard image backend found (wl-paste or xclip)".to_owned(),
        )
    }
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
    use super::{
        ClipboardError, ClipboardImage, Command, classify_linux_image_output,
        linux_clipboard_failure, linux_output_error, linux_targets_include_png,
    };

    pub fn read_clipboard_image() -> Result<ClipboardImage, ClipboardError> {
        let candidates: [(&str, &[&str], &[&str]); 2] = [
            ("wl-paste", &["--list-types"], &["--type", "image/png"]),
            (
                "xclip",
                &["-selection", "clipboard", "-t", "TARGETS", "-o"],
                &["-selection", "clipboard", "-t", "image/png", "-o"],
            ),
        ];

        let mut image_error = None;
        let mut probe_error = None;
        let mut confirmed_no_image = false;

        for (cmd, probe_args, read_args) in candidates {
            let probe = match Command::new(cmd).args(probe_args).output() {
                Ok(out) => out,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    probe_error = Some(format!("{cmd}: {e}"));
                    continue;
                }
            };
            if !probe.status.success() {
                probe_error = Some(linux_output_error(
                    cmd,
                    &probe.status.to_string(),
                    &probe.stderr,
                ));
                continue;
            }
            if !linux_targets_include_png(&probe.stdout) {
                confirmed_no_image = true;
                continue;
            }

            let out = match Command::new(cmd).args(read_args).output() {
                Ok(out) => out,
                Err(e) => {
                    image_error = Some(format!("{cmd}: {e}"));
                    continue;
                }
            };

            let success = out.status.success();
            let status = out.status.to_string();
            match classify_linux_image_output(cmd, success, &status, out.stdout, &out.stderr) {
                Ok(image) => return Ok(image),
                Err(error) => image_error = Some(error),
            }
        }

        Err(linux_clipboard_failure(
            image_error,
            confirmed_no_image,
            probe_error,
        ))
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

#[cfg(test)]
mod tests {
    use super::{
        ClipboardError, classify_linux_image_output, linux_clipboard_failure,
        linux_targets_include_png,
    };

    #[test]
    fn linux_clipboard_probes_targets_before_reading_image() {
        assert!(linux_targets_include_png(b"text/plain\r\n image/png \n"));
        assert!(linux_targets_include_png(b"\xff-invalid\nimage/png\n"));
        assert!(!linux_targets_include_png(b"text/plain\nUTF8_STRING\n"));

        let mut stderr = vec![b'x'; 600];
        stderr.extend_from_slice(b"unbounded-tail");
        let failed =
            classify_linux_image_output("wl-paste", false, "exit status: 1", Vec::new(), &stderr);
        let Err(failed) = failed else {
            panic!("non-zero exit must be a read failure");
        };
        assert!(failed.contains("wl-paste exited with"));
        assert!(!failed.contains("unbounded-tail"));

        let invalid = classify_linux_image_output(
            "xclip",
            true,
            "exit status: 0",
            b"not an image".to_vec(),
            &[],
        );
        assert!(matches!(invalid, Err(error) if error == "xclip returned invalid image data"));

        assert!(matches!(
            linux_clipboard_failure(
                Some("advertised image read failed".to_owned()),
                true,
                Some("probe failed".to_owned()),
            ),
            ClipboardError::ReadFailed(error) if error == "advertised image read failed"
        ));
        assert!(matches!(
            linux_clipboard_failure(None, true, Some("probe failed".to_owned())),
            ClipboardError::NoImage
        ));
        assert!(matches!(
            linux_clipboard_failure(None, false, None),
            ClipboardError::ReadFailed(error) if error.contains("wl-paste or xclip")
        ));
    }
}
