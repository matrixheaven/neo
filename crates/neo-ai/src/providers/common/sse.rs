//! Shared SSE byte-stream framing utilities used by all streaming providers.
//!
//! [`SseFramer`] owns pending bytes, split-delimiter handling, and a fixed
//! machine-safety bound. Each provider keeps its own `IncrementalSse` /
//! `ParseState` because the JSON payload interpretation differs per provider;
//! only the framing layer is shared here.

use crate::AiError;

/// Maximum size of a single SSE frame, including its delimiter.
pub(crate) const MAX_SSE_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// A single chunk produced by the HTTP byte stream, or the synthetic `End`
/// sentinel appended after the stream completes.
pub(crate) enum StreamChunk {
    Data(Result<Vec<u8>, reqwest::Error>),
    End,
}

/// Raw SSE frame bytes, including the delimiter.
#[derive(Debug)]
pub(crate) struct SseFrame {
    bytes: Vec<u8>,
}

impl SseFrame {
    /// Parse the `data:` payload from this frame.
    pub(crate) fn parse(&self) -> Result<Option<String>, AiError> {
        parse_sse_frame(&self.bytes)
    }
}

/// Buffered SSE framer with a fixed machine-safety bound.
///
/// Owns pending bytes, tracks a consumed cursor, compacts occasionally, and
/// returns a non-retryable protocol error when a single frame exceeds
/// [`MAX_SSE_FRAME_BYTES`].
pub(crate) struct SseFramer {
    buffer: Vec<u8>,
    /// Byte offset into `buffer` where the next frame search starts.
    cursor: usize,
    /// Set once an oversized frame is detected so the framer stays stopped.
    stopped: bool,
}

impl SseFramer {
    /// Maximum size of a single SSE frame, including its delimiter.
    pub(crate) const MAX_FRAME_BYTES: usize = MAX_SSE_FRAME_BYTES;

    const COMPACT_THRESHOLD: usize = 4 * 1024;

    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            buffer: Vec::new(),
            cursor: 0,
            stopped: false,
        }
    }

    /// Append bytes and return all complete frames now available.
    ///
    /// Returns a non-retryable [`AiError::Protocol`] if an incomplete pending
    /// frame or a complete frame exceeds [`MAX_FRAME_BYTES`].
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseFrame>, AiError> {
        if self.stopped {
            return Ok(Vec::new());
        }
        self.buffer.extend_from_slice(bytes);
        self.drain_complete_frames()
    }

    /// Take the unconsumed pending bytes and reset the cursor.
    ///
    /// This is used by providers that allow one trailing payload without a
    /// final delimiter.
    pub(crate) fn take_pending(&mut self) -> Vec<u8> {
        let pending = self.buffer[self.cursor..].to_vec();
        self.buffer.clear();
        self.cursor = 0;
        pending
    }

    fn drain_complete_frames(&mut self) -> Result<Vec<SseFrame>, AiError> {
        let mut frames = Vec::new();
        loop {
            let window = &self.buffer[self.cursor..];
            if let Some(index) = window.windows(2).position(|window| window == b"\n\n") {
                let end = self.cursor + index + 2;
                if end - self.cursor > Self::MAX_FRAME_BYTES {
                    self.stopped = true;
                    return Err(oversized_frame_error());
                }
                let bytes = self.buffer[self.cursor..end].to_vec();
                self.cursor = end;
                frames.push(SseFrame { bytes });
                self.maybe_compact();
                continue;
            }
            if let Some(index) = window.windows(4).position(|window| window == b"\r\n\r\n") {
                let end = self.cursor + index + 4;
                if end - self.cursor > Self::MAX_FRAME_BYTES {
                    self.stopped = true;
                    return Err(oversized_frame_error());
                }
                let bytes = self.buffer[self.cursor..end].to_vec();
                self.cursor = end;
                frames.push(SseFrame { bytes });
                self.maybe_compact();
                continue;
            }
            break;
        }

        if self.buffer.len() - self.cursor > Self::MAX_FRAME_BYTES {
            self.stopped = true;
            return Err(oversized_frame_error());
        }

        Ok(frames)
    }

    fn maybe_compact(&mut self) {
        if self.cursor >= Self::COMPACT_THRESHOLD {
            let remaining = self.buffer.len() - self.cursor;
            self.buffer.copy_within(self.cursor.., 0);
            self.buffer.truncate(remaining);
            self.cursor = 0;
        }
    }
}

impl Default for SseFramer {
    fn default() -> Self {
        Self::new()
    }
}

fn oversized_frame_error() -> AiError {
    AiError::Protocol {
        message: format!(
            "SSE frame exceeds {} MiB limit",
            MAX_SSE_FRAME_BYTES / (1024 * 1024)
        ),
    }
}

/// Extract the `data:` payload from a raw SSE frame.
///
/// Joins all `data:` lines (trimmed) with `\n`. Returns `Ok(None)` when the
/// frame carries no data payload.
pub(crate) fn parse_sse_frame(frame: &[u8]) -> Result<Option<String>, AiError> {
    let text = std::str::from_utf8(frame).map_err(|err| AiError::Protocol {
        message: format!("invalid SSE UTF-8: {err}"),
    })?;
    let data = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n");
    Ok((!data.is_empty()).then_some(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framer_rejects_oversized_incomplete_frame() {
        let mut framer = SseFramer::new();
        let partial = vec![b'x'; SseFramer::MAX_FRAME_BYTES];
        let frames = framer
            .push(&partial)
            .expect("at-limit partial should be accepted");
        assert!(frames.is_empty());

        let err = framer
            .push(b"x")
            .expect_err("oversized incomplete frame should be rejected");
        assert!(matches!(err, AiError::Protocol { .. }));
        assert!(!err.is_retryable());
    }

    #[test]
    fn framer_accepts_each_delimiter_split_at_every_byte() {
        let delimiters: &[&[u8]] = &[b"\n\n", b"\r\n\r\n"];
        for delimiter in delimiters {
            let frame = [b"data: hello".as_slice(), delimiter].concat();
            for split_at in 0..=frame.len() {
                let mut framer = SseFramer::new();
                let mut frames = framer
                    .push(&frame[..split_at])
                    .expect("first chunk should be valid");
                frames.extend(
                    framer
                        .push(&frame[split_at..])
                        .expect("second chunk should be valid"),
                );
                assert_eq!(
                    frames.len(),
                    1,
                    "delimiter {delimiter:?} split at {split_at} should produce one frame"
                );
                assert_eq!(
                    frames[0].parse().expect("valid frame").as_deref(),
                    Some("hello")
                );
            }
        }
    }
}
