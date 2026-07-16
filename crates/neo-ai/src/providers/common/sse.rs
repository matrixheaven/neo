//! Shared SSE byte-stream framing utilities used by all streaming providers.
//!
//! These functions handle the low-level Server-Sent Events frame boundary
//! detection and `data:` payload extraction. Each provider keeps its own
//! `IncrementalSse` / `ParseState` because the JSON payload interpretation
//! differs per provider; only the framing layer is shared here.

use crate::AiError;

/// A single chunk produced by the HTTP byte stream, or the synthetic `End`
/// sentinel appended after the stream completes.
pub(crate) enum StreamChunk {
    Data(Result<Vec<u8>, reqwest::Error>),
    End,
}

/// Locate the first SSE frame boundary in `buffer`.
///
/// Returns `(index, delimiter_len)` where `index` is the byte offset of the
/// boundary start and `delimiter_len` is either `2` (for `\n\n`) or `4` (for
/// `\r\n\r\n`). Returns `None` if no complete frame boundary is present yet.
pub(crate) fn find_frame_end(buffer: &[u8]) -> Option<(usize, usize)> {
    buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (index, 2))
        .or_else(|| {
            buffer
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| (index, 4))
        })
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
