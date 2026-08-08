use std::io::SeekFrom;

use schemars::JsonSchema;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};

use super::{
    Tool, ToolContext, ToolError, ToolFuture, ToolResult, parse_input, schema,
    text_encoding::{
        ENCODING_DETECTION_SAMPLE_BYTES, Utf16ByteOrder, Utf16Decoder, detect_utf16_byte_order,
    },
};
use crate::workspace_policy::normalize_path;

const MAX_LINES: usize = 1000;
const DEFAULT_LINES: usize = 400;
const MAX_LINE_LENGTH: usize = 2000;
const MAX_BYTES: usize = 100 * 1024;
const READ_CHUNK_SIZE: usize = 64 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadInput {
    #[schemars(
        description = "Path to the text file. Relative paths resolve against the working directory; absolute paths are used as-is, including paths outside the working directory."
    )]
    path: std::path::PathBuf,
    #[schemars(
        description = "1-based line number to start reading from. Omit to start at line 1. Negative values read from the end of the file; the absolute value must not exceed 1000."
    )]
    line_offset: Option<i64>,
    #[schemars(
        description = "Maximum number of lines to read. Omit to read up to the internal cap."
    )]
    n_lines: Option<usize>,
}

pub struct ReadTool;

impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "Read"
    }

    fn description(&self) -> &'static str {
        "Read a UTF-8 or UTF-16 text file.\
        \
        If the user provides a concrete file path, call Read directly. Do not use Glob, ls, or \
        other pre-checks for known text file paths; missing or invalid paths return errors you can \
        handle. Use Glob for pattern searches and Bash `ls` for directories.\
        \
        Prefer targeted reads: for files over ~200 lines, use `line_offset` and `n_lines` to read \
        only the range you need instead of the whole file. Small windows keep the context and the \
        provider cache small; a full read of a large file costs tokens for every line.\
        \
        Parameters:\
        - path: Path to the text file. Relative paths resolve against the working directory; \
          absolute paths are used as-is, including paths outside the working directory.\
        - line_offset: 1-based line number to start reading from. Omit to start at line 1. Negative \
          values read from the end (e.g. -100 reads the last 100 lines); the absolute value must \
          not exceed 1000.\
        - n_lines: Maximum number of lines to read (default 400; cap 1000).\
        \
        Behavior:\
        - Returns up to 400 lines by default (1000 max) or 100 KB per call, whichever comes first.\
        - Lines longer than 2000 characters are truncated mid-line and marked with `...`.\
        - Output format: each line is prefixed with `<line-number>\\t<content>`.\
        - A `<system>...</system>` status block is appended after the content; it summarizes how \
          much was read and is not part of the file itself.\
        - Page larger files with multiple Read calls using line_offset and n_lines.\
        - When you need several files, prefer reading them in parallel.\
        - UTF-16LE/BE text is converted transparently. Binary files, images, and videos are refused."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<ReadInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_file_read_allowed()?;
            let input: ReadInput = parse_input(self.name(), input)?;
            let path = resolve_read_path(ctx, &input.path);

            match run_read(&path, input.line_offset, input.n_lines).await {
                Ok(result) => Ok(ToolResult::ok(result.finish_output())),
                Err(ReadError::Io(source)) => Err(ToolError::Io(source)),
                Err(ReadError::InvalidInput(message)) => Err(ToolError::InvalidInput {
                    tool: self.name().to_owned(),
                    message,
                }),
                // NotReadable and Missing are semantically distinct but both surface as a failed
                // tool result to the model; keep them separate so callers can tell them apart.
                #[allow(clippy::match_same_arms)]
                Err(ReadError::NotReadable(message)) => Ok(ToolResult::error(message)),
                Err(ReadError::Missing(message)) => Ok(ToolResult::error(message)),
            }
        })
    }
}

fn resolve_read_path(ctx: &ToolContext, path: &std::path::Path) -> std::path::PathBuf {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        ctx.workspace_root().join(path)
    };
    normalize_path(
        &candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.clone()),
    )
}

#[derive(Debug)]
enum ReadError {
    Io(std::io::Error),
    InvalidInput(String),
    NotReadable(String),
    Missing(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(source) => write!(f, "io error: {source}"),
            Self::InvalidInput(message) | Self::NotReadable(message) | Self::Missing(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(source) => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ReadError {
    fn from(source: std::io::Error) -> Self {
        Self::Io(source)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEndingStyle {
    Lf,
    Crlf,
    Mixed,
}

impl LineEndingStyle {
    fn from_flags(flags: LineEndingFlags) -> Self {
        if flags.has_lone_cr || (flags.has_crlf && flags.has_lf) {
            Self::Mixed
        } else if flags.has_crlf {
            Self::Crlf
        } else {
            Self::Lf
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct LineEndingFlags {
    has_crlf: bool,
    has_lf: bool,
    has_lone_cr: bool,
}

impl LineEndingFlags {
    fn update(&mut self, text: &str) {
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\r' {
                if chars.peek() == Some(&'\n') {
                    self.has_crlf = true;
                    chars.next();
                } else {
                    self.has_lone_cr = true;
                }
            } else if ch == '\n' {
                self.has_lf = true;
            }
        }
    }
}

fn strip_trailing_lf(line: &str) -> &str {
    line.strip_suffix('\n').unwrap_or(line)
}

fn truncate_line(line: &str, max_len: usize) -> (String, bool) {
    let count = line.chars().count();
    if count <= max_len {
        return (line.to_owned(), false);
    }
    let marker = "...";
    let keep = max_len.saturating_sub(marker.len());
    let mut truncated = String::with_capacity(max_len);
    for ch in line.chars().take(keep) {
        truncated.push(ch);
    }
    truncated.push_str(marker);
    (truncated, true)
}

fn render_line_content(raw: &str, style: LineEndingStyle) -> String {
    match style {
        LineEndingStyle::Crlf => raw.strip_suffix('\r').unwrap_or(raw).to_owned(),
        LineEndingStyle::Mixed => raw.replace('\r', "\\r"),
        LineEndingStyle::Lf => raw.to_owned(),
    }
}

#[derive(Debug)]
struct ReadRenderResult {
    rendered_lines: Vec<String>,
    start_line: usize,
    total_lines: usize,
    requested_lines: usize,
    max_lines_reached: bool,
    max_bytes_reached: bool,
    truncated_line_numbers: Vec<usize>,
    line_ending_style: LineEndingStyle,
}

impl ReadRenderResult {
    fn finish_output(&self) -> String {
        let rendered = self.rendered_lines.join("\n");
        let message = self.finish_message();
        if rendered.is_empty() {
            format!("<system>{message}</system>")
        } else {
            format!("{rendered}\n<system>{message}</system>")
        }
    }

    fn finish_message(&self) -> String {
        let line_count = self.rendered_lines.len();
        let line_word = if line_count == 1 { "line" } else { "lines" };
        let mut parts = Vec::new();

        if line_count > 0 {
            parts.push(format!(
                "{line_count} {line_word} read from file starting from line {}.",
                self.start_line
            ));
        } else {
            parts.push("No lines read from file.".to_owned());
        }

        parts.push(format!("Total lines in file: {}.", self.total_lines));

        if self.max_lines_reached {
            parts.push(format!("Max {MAX_LINES} lines reached."));
        } else if self.max_bytes_reached {
            parts.push(format!("Max {MAX_BYTES} bytes reached."));
        } else if line_count < self.requested_lines {
            parts.push("End of file reached.".to_owned());
        }

        if !self.truncated_line_numbers.is_empty() {
            parts.push(format!(
                "Lines [{}] were truncated.",
                self.truncated_line_numbers
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        if self.line_ending_style == LineEndingStyle::Mixed {
            parts.push(
                "Mixed or lone carriage-return line endings are shown as \\r. Use exact \\r\\n or \\r escapes in Edit.old for those lines.".to_owned(),
            );
        }

        parts.join(" ")
    }
}

async fn run_read(
    path: &std::path::Path,
    line_offset: Option<i64>,
    n_lines: Option<usize>,
) -> Result<ReadRenderResult, ReadError> {
    let line_offset = line_offset.unwrap_or(1);
    if line_offset == 0 {
        return Err(ReadError::InvalidInput(
            "line_offset must not be 0".to_owned(),
        ));
    }
    let abs_offset = usize::try_from(line_offset.unsigned_abs()).unwrap_or(usize::MAX);
    if line_offset < 0 && abs_offset > MAX_LINES {
        return Err(ReadError::InvalidInput(format!(
            "absolute value of negative line_offset must not exceed {MAX_LINES}"
        )));
    }

    let requested_lines = n_lines.unwrap_or(DEFAULT_LINES);
    if requested_lines == 0 {
        return Err(ReadError::InvalidInput(
            "n_lines must be greater than 0".to_owned(),
        ));
    }
    let effective_limit = requested_lines.min(MAX_LINES);

    if !path.exists() {
        return Err(ReadError::Missing(format!(
            "\"{}\" does not exist.",
            path.display()
        )));
    }

    let metadata = tokio::fs::metadata(path).await?;
    if !metadata.is_file() {
        return Err(ReadError::Missing(format!(
            "\"{}\" is not a file.",
            path.display()
        )));
    }

    if is_sensitive_path(path) {
        return Err(ReadError::NotReadable(format!(
            "\"{}\" matches a sensitive-file pattern and is refused to protect secrets.",
            path.display()
        )));
    }

    if line_offset < 0 {
        read_tail(path, abs_offset, effective_limit, requested_lines).await
    } else {
        read_forward(path, abs_offset, effective_limit, requested_lines).await
    }
}

async fn read_forward(
    path: &std::path::Path,
    line_offset: usize,
    effective_limit: usize,
    requested_lines: usize,
) -> Result<ReadRenderResult, ReadError> {
    let mut reader = open_text_line_reader(path).await?;

    let mut flags = LineEndingFlags::default();
    let mut current_line_no: usize = 0;
    let mut selected: Vec<(usize, String)> = Vec::new();
    let mut max_lines_reached = false;
    let mut collection_closed = false;

    loop {
        let mut raw = String::new();
        let bytes_read = reader.read_line(&mut raw).await?;
        if bytes_read == 0 {
            break;
        }
        if contains_nul(&raw) {
            return Err(not_readable_error(path));
        }
        current_line_no += 1;
        flags.update(&raw);

        if collection_closed {
            if effective_limit >= MAX_LINES && current_line_no >= line_offset {
                max_lines_reached = true;
            }
            continue;
        }

        if current_line_no < line_offset {
            continue;
        }

        if selected.len() >= effective_limit {
            if effective_limit >= MAX_LINES {
                max_lines_reached = true;
            }
            collection_closed = true;
            continue;
        }

        selected.push((current_line_no, strip_trailing_lf(&raw).to_owned()));
        if selected.len() >= effective_limit {
            collection_closed = true;
        }
    }

    render_entries(
        selected,
        flags,
        max_lines_reached,
        false,
        current_line_no,
        requested_lines,
    )
}

async fn read_tail(
    path: &std::path::Path,
    tail_count: usize,
    effective_limit: usize,
    requested_lines: usize,
) -> Result<ReadRenderResult, ReadError> {
    let mut reader = open_text_line_reader(path).await?;

    let mut flags = LineEndingFlags::default();
    let mut current_line_no: usize = 0;
    let mut entries: std::collections::VecDeque<(usize, String)> =
        std::collections::VecDeque::with_capacity(tail_count);

    loop {
        let mut raw = String::new();
        let bytes_read = reader.read_line(&mut raw).await?;
        if bytes_read == 0 {
            break;
        }
        if contains_nul(&raw) {
            return Err(not_readable_error(path));
        }
        current_line_no += 1;
        flags.update(&raw);
        entries.push_back((current_line_no, strip_trailing_lf(&raw).to_owned()));
        if entries.len() > tail_count {
            entries.pop_front();
        }
    }

    let selected: Vec<(usize, String)> = entries.into_iter().take(effective_limit).collect();
    render_entries(
        selected,
        flags,
        false,
        false,
        current_line_no,
        requested_lines,
    )
}

#[allow(clippy::unnecessary_wraps)]
fn render_entries(
    entries: Vec<(usize, String)>,
    flags: LineEndingFlags,
    mut max_lines_reached: bool,
    max_bytes_reached_input: bool,
    total_lines: usize,
    requested_lines: usize,
) -> Result<ReadRenderResult, ReadError> {
    let line_ending_style = LineEndingStyle::from_flags(flags);
    let mut rendered_lines = Vec::new();
    let mut truncated_line_numbers = Vec::new();
    let mut bytes_used: usize = 0;
    let mut max_bytes_reached = max_bytes_reached_input;

    for (line_no, raw_line) in entries {
        let (truncated, was_truncated) = truncate_line(&raw_line, MAX_LINE_LENGTH);
        if was_truncated {
            truncated_line_numbers.push(line_no);
        }
        let visible = render_line_content(&truncated, line_ending_style);
        let rendered = format!("{line_no}\t{visible}");
        let line_bytes = rendered.len() + usize::from(!rendered_lines.is_empty());

        if !rendered_lines.is_empty() && bytes_used + line_bytes > MAX_BYTES {
            max_bytes_reached = true;
            break;
        }

        bytes_used += line_bytes;
        rendered_lines.push(rendered);
    }

    // If we stopped early because of bytes, max_lines_reached is no longer accurate.
    if max_bytes_reached {
        max_lines_reached = false;
    }

    let start_line = rendered_lines
        .first()
        .and_then(|line| line.split('\t').next())
        .and_then(|num| num.parse().ok())
        .unwrap_or(0);

    Ok(ReadRenderResult {
        rendered_lines,
        start_line,
        total_lines,
        requested_lines,
        max_lines_reached,
        max_bytes_reached,
        truncated_line_numbers,
        line_ending_style,
    })
}

fn contains_nul(text: &str) -> bool {
    text.contains('\0')
}

enum TextLineReader {
    Utf8(BufReader<tokio::fs::File>),
    Utf16(Utf16LineReader),
}

impl TextLineReader {
    async fn read_line(&mut self, output: &mut String) -> Result<usize, ReadError> {
        match self {
            Self::Utf8(reader) => Ok(reader.read_line(output).await?),
            Self::Utf16(reader) => reader.read_line(output).await,
        }
    }
}

struct Utf16LineReader {
    file: tokio::fs::File,
    byte_order: Utf16ByteOrder,
    byte_buffer: Vec<u8>,
    decoded: String,
    pending_byte: Option<u8>,
    decoder: Utf16Decoder,
    eof: bool,
}

impl Utf16LineReader {
    fn new(file: tokio::fs::File, byte_order: Utf16ByteOrder) -> Self {
        Self {
            file,
            byte_order,
            byte_buffer: vec![0; READ_CHUNK_SIZE],
            decoded: String::new(),
            pending_byte: None,
            decoder: Utf16Decoder::new(),
            eof: false,
        }
    }

    async fn read_line(&mut self, output: &mut String) -> Result<usize, ReadError> {
        let start_len = output.len();
        loop {
            if let Some(newline) = self.decoded.find('\n') {
                output.push_str(&self.decoded[..=newline]);
                self.decoded.drain(..=newline);
                return Ok(output.len() - start_len);
            }
            if self.eof {
                if self.decoded.is_empty() {
                    return Ok(0);
                }
                output.push_str(&self.decoded);
                self.decoded.clear();
                return Ok(output.len() - start_len);
            }
            self.fill().await?;
        }
    }

    async fn fill(&mut self) -> Result<(), ReadError> {
        let bytes_read = self.file.read(&mut self.byte_buffer).await?;
        if bytes_read == 0 {
            self.eof = true;
            self.finish_decoding();
            return Ok(());
        }

        for index in 0..bytes_read {
            self.push_byte(self.byte_buffer[index]);
        }
        Ok(())
    }

    fn push_byte(&mut self, byte: u8) {
        let Some(first) = self.pending_byte.replace(byte) else {
            return;
        };
        self.pending_byte = None;
        let unit = match self.byte_order {
            Utf16ByteOrder::LittleEndian => u16::from_le_bytes([first, byte]),
            Utf16ByteOrder::BigEndian => u16::from_be_bytes([first, byte]),
        };
        self.decoder.push_unit(&mut self.decoded, unit);
    }

    fn finish_decoding(&mut self) {
        self.decoder.finish(&mut self.decoded);
        if self.pending_byte.take().is_some() {
            self.decoded.push(char::REPLACEMENT_CHARACTER);
        }
    }
}

async fn open_text_line_reader(path: &std::path::Path) -> Result<TextLineReader, ReadError> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut sample = [0; ENCODING_DETECTION_SAMPLE_BYTES];
    let sample_len = file.read(&mut sample).await?;
    let (byte_order, bom_len) = detect_utf16_byte_order(&sample[..sample_len]);
    file.seek(SeekFrom::Start(
        u64::try_from(bom_len).expect("BOM length fits in u64"),
    ))
    .await?;

    Ok(match byte_order {
        Some(byte_order) => TextLineReader::Utf16(Utf16LineReader::new(file, byte_order)),
        None => TextLineReader::Utf8(BufReader::with_capacity(READ_CHUNK_SIZE, file)),
    })
}

fn not_readable_error(path: &std::path::Path) -> ReadError {
    ReadError::NotReadable(format!(
        "\"{}\" is not readable as text. If it is an image or video, use ReadMediaFile. For other binary formats, use Bash or an MCP tool if available.",
        path.display()
    ))
}

const SENSITIVE_NAMES: &[&str] = &[
    ".env",
    ".env.local",
    ".env.production",
    ".env.development",
    ".envrc",
    ".npmrc",
    ".pypirc",
    ".netrc",
    ".git-credentials",
    ".dockerconfigjson",
    "id_rsa",
    "id_rsa.pub",
    "id_ed25519",
    "id_ed25519.pub",
    "id_ecdsa",
    "id_ecdsa.pub",
    "id_dsa",
    "id_dsa.pub",
    ".aws",
    ".ssh",
    "credentials.json",
    "service-account.json",
];

const SENSITIVE_EXTENSIONS: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".crt", ".cer", ".der"];

fn is_sensitive_path(path: &std::path::Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };

    if SENSITIVE_NAMES.contains(&name) {
        return true;
    }

    let lower = name.to_lowercase();
    SENSITIVE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

#[cfg(test)]
#[path = "test_cases/render.rs"]
mod render;

#[cfg(test)]
#[path = "test_cases/offsets.rs"]
mod offsets;

#[cfg(test)]
#[path = "test_cases/access.rs"]
mod access;
