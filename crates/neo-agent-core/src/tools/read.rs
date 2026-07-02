use schemars::JsonSchema;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::{
    Tool, ToolContext, ToolError, ToolFuture, ToolResult, normalize_path, parse_input, schema,
};

const MAX_LINES: usize = 1000;
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
        "Read a UTF-8 text file.\
        \
        If the user provides a concrete file path, call Read directly. Do not use Glob, ls, or \
        other pre-checks for known text file paths; missing or invalid paths return errors you can \
        handle. Use Glob for pattern searches and Bash `ls` for directories.\
        \
        Parameters:\
        - path: Path to the text file. Relative paths resolve against the working directory; \
          absolute paths are used as-is, including paths outside the working directory.\
        - line_offset: 1-based line number to start reading from. Omit to start at line 1. Negative \
          values read from the end (e.g. -100 reads the last 100 lines); the absolute value must \
          not exceed 1000.\
        - n_lines: Maximum number of lines to read. Omit to read up to the internal cap of 1000 \
          lines.\
        \
        Behavior:\
        - Returns up to 1000 lines or 100 KB per call, whichever comes first.\
        - Lines longer than 2000 characters are truncated mid-line and marked with `...`.\
        - Output format: each line is prefixed with `<line-number>\\t<content>`.\
        - A `<system>...</system>` status block is appended after the content; it summarizes how \
          much was read and is not part of the file itself.\
        - Page larger files with multiple Read calls using line_offset and n_lines.\
        - When you need several files, prefer reading them in parallel.\
        - Only UTF-8 text files can be read. Binary files, images, and videos are refused."
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

    let requested_lines = n_lines.unwrap_or(MAX_LINES);
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
    let file = tokio::fs::File::open(path).await?;
    let mut reader = BufReader::with_capacity(READ_CHUNK_SIZE, file);

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
    let file = tokio::fs::File::open(path).await?;
    let mut reader = BufReader::with_capacity(READ_CHUNK_SIZE, file);

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

fn not_readable_error(path: &std::path::Path) -> ReadError {
    ReadError::NotReadable(format!(
        "\"{}\" is not readable as UTF-8 text. If it is an image or video, use ReadMediaFile. For other binary formats, use Bash or an MCP tool if available.",
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
mod tests {
    use super::*;

    #[test]
    fn reads_whole_small_file() {
        let content = "line one\nline two\nline three\n";
        let result = render_from_content(content, None, None).unwrap();
        assert_eq!(result.rendered_lines.len(), 3);
        assert_eq!(result.rendered_lines[0], "1\tline one");
        assert_eq!(result.rendered_lines[2], "3\tline three");
        assert!(result.finish_output().contains("Total lines in file: 3."));
        assert!(result.finish_output().contains("End of file reached."));
    }

    #[test]
    fn reads_from_positive_offset() {
        let content = "a\nb\nc\nd\ne\n";
        let result = render_from_content(content, Some(3), Some(2)).unwrap();
        assert_eq!(result.rendered_lines.len(), 2);
        assert_eq!(result.rendered_lines[0], "3\tc");
        assert_eq!(result.rendered_lines[1], "4\td");
        assert!(result.finish_output().contains("starting from line 3"));
    }

    #[test]
    fn reads_from_negative_offset() {
        let content = "a\nb\nc\nd\ne\n";
        let result = render_from_content(content, Some(-2), None).unwrap();
        assert_eq!(result.rendered_lines.len(), 2);
        assert_eq!(result.rendered_lines[0], "4\td");
        assert_eq!(result.rendered_lines[1], "5\te");
    }

    #[test]
    fn zero_line_offset_is_rejected() {
        let err = render_from_content("x\n", Some(0), None).unwrap_err();
        assert!(err.to_string().contains("line_offset must not be 0"));
    }

    #[test]
    fn zero_n_lines_is_rejected() {
        let err = render_from_content("x\n", None, Some(0)).unwrap_err();
        assert!(err.to_string().contains("n_lines must be greater than 0"));
    }

    #[test]
    fn positive_line_offset_beyond_cap_is_allowed() {
        let content = (1..=2500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let result = render_from_content(&content, Some(1500), Some(5)).unwrap();
        assert_eq!(result.rendered_lines.len(), 5);
        assert_eq!(result.rendered_lines[0], "1500\tline 1500");
        assert_eq!(result.rendered_lines[4], "1504\tline 1504");
        assert!(
            result
                .finish_output()
                .contains("Total lines in file: 2500.")
        );
    }

    #[test]
    fn negative_line_offset_beyond_cap_is_rejected() {
        let err = render_from_content("x\n", Some(-1001), None).unwrap_err();
        assert!(
            err.to_string()
                .contains("absolute value of negative line_offset")
        );
    }

    #[test]
    fn reads_from_negative_offset_beyond_default_cap() {
        let content = (1..=2500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let result = render_from_content(&content, Some(-100), None).unwrap();
        assert_eq!(result.rendered_lines.len(), 100);
        assert_eq!(result.rendered_lines[0], "2401\tline 2401");
        assert_eq!(result.rendered_lines[99], "2500\tline 2500");
    }

    #[test]
    fn max_lines_cap_is_reported() {
        let content = (1..=MAX_LINES + 10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let result = render_from_content(&content, None, None).unwrap();
        assert_eq!(result.rendered_lines.len(), MAX_LINES);
        assert!(
            result
                .finish_output()
                .contains(&format!("Max {MAX_LINES} lines reached."))
        );
    }

    #[test]
    fn long_lines_are_truncated() {
        let long = "x".repeat(MAX_LINE_LENGTH + 10);
        let content = format!("{long}\n");
        let result = render_from_content(&content, None, None).unwrap();
        assert_eq!(result.rendered_lines.len(), 1);
        assert!(result.rendered_lines[0].ends_with("..."));
        assert!(result.truncated_line_numbers.contains(&1));
        assert!(result.finish_output().contains("Lines [1] were truncated."));
    }

    #[test]
    fn crlf_is_normalized() {
        let content = "one\r\ntwo\r\n";
        let result = render_from_content(content, None, None).unwrap();
        assert_eq!(result.rendered_lines[0], "1\tone");
        assert_eq!(result.rendered_lines[1], "2\ttwo");
    }

    #[test]
    fn mixed_line_endings_show_escape() {
        let content = "one\r\ntwo\rthree\n";
        let result = render_from_content(content, None, None).unwrap();
        assert_eq!(result.rendered_lines[0], "1\tone\\r");
        assert_eq!(result.rendered_lines[1], "2\ttwo\\rthree");
        assert!(
            result
                .finish_message()
                .contains("Mixed or lone carriage-return line endings are shown as \\r")
        );
    }

    #[tokio::test]
    async fn read_tool_allows_absolute_paths_outside_workspace() {
        use super::{ReadTool, Tool};
        use serde_json::json;

        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let external_dir = temp.path().join("external");
        std::fs::create_dir_all(&external_dir).expect("external dir");
        let external_file = external_dir.join("note.md");
        std::fs::write(&external_file, "external content\n").expect("write external");

        let ctx = crate::ToolContext::new(&workspace)
            .expect("tool context")
            .with_access(crate::ToolAccess {
                file_read: true,
                file_write: false,
                shell: false,
                tool: false,
                user_question: false,
            });

        let tool = ReadTool;
        let input = json!({
            "path": external_file.to_str().unwrap(),
        });
        let result = tool.execute(&ctx, input).await.expect("execute");
        assert!(!result.is_error);
        assert!(result.content.contains("external content"));
    }

    #[tokio::test]
    async fn read_tool_resolves_relative_paths_against_workspace() {
        use super::{ReadTool, Tool};
        use serde_json::json;

        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        std::fs::write(workspace.join("note.md"), "workspace content\n").expect("write note");

        let ctx = crate::ToolContext::new(&workspace)
            .expect("tool context")
            .with_access(crate::ToolAccess {
                file_read: true,
                file_write: false,
                shell: false,
                tool: false,
                user_question: false,
            });

        let tool = ReadTool;
        let input = json!({"path": "note.md"});
        let result = tool.execute(&ctx, input).await.expect("execute");
        assert!(!result.is_error);
        assert!(result.content.contains("workspace content"));
    }

    #[tokio::test]
    async fn read_tool_rejects_missing_file() {
        use super::{ReadTool, Tool};
        use serde_json::json;

        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");

        let ctx = crate::ToolContext::new(&workspace)
            .expect("tool context")
            .with_access(crate::ToolAccess {
                file_read: true,
                file_write: false,
                shell: false,
                tool: false,
                user_question: false,
            });

        let tool = ReadTool;
        let input = json!({"path": "missing.txt"});
        let result = tool.execute(&ctx, input).await.expect("execute");
        assert!(result.is_error);
        assert!(result.content.contains("does not exist"));
    }

    #[tokio::test]
    async fn read_tool_rejects_directories() {
        use super::{ReadTool, Tool};
        use serde_json::json;

        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        std::fs::create_dir_all(workspace.join("src")).expect("src dir");

        let ctx = crate::ToolContext::new(&workspace)
            .expect("tool context")
            .with_access(crate::ToolAccess {
                file_read: true,
                file_write: false,
                shell: false,
                tool: false,
                user_question: false,
            });

        let tool = ReadTool;
        let input = json!({"path": "src"});
        let result = tool.execute(&ctx, input).await.expect("execute");
        assert!(result.is_error);
        assert!(result.content.contains("is not a file"));
    }

    #[tokio::test]
    async fn read_tool_rejects_sensitive_files() {
        use super::{ReadTool, Tool};
        use serde_json::json;

        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        std::fs::write(workspace.join(".env"), "SECRET=value\n").expect("write env");
        std::fs::write(workspace.join("key.pem"), "secret key\n").expect("write pem");

        let ctx = crate::ToolContext::new(&workspace)
            .expect("tool context")
            .with_access(crate::ToolAccess {
                file_read: true,
                file_write: false,
                shell: false,
                tool: false,
                user_question: false,
            });

        let tool = ReadTool;

        let dot_env = tool
            .execute(&ctx, json!({"path": ".env"}))
            .await
            .expect("execute");
        assert!(dot_env.is_error);
        assert!(dot_env.content.contains("sensitive-file pattern"));

        let pem = tool
            .execute(&ctx, json!({"path": "key.pem"}))
            .await
            .expect("execute");
        assert!(pem.is_error);
        assert!(pem.content.contains("sensitive-file pattern"));
    }

    #[tokio::test]
    async fn read_tool_rejects_nul_bytes() {
        use super::{ReadTool, Tool};
        use serde_json::json;

        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let mut bytes = b"text\n".to_vec();
        bytes.push(0);
        bytes.extend_from_slice(b"tail\n");
        std::fs::write(workspace.join("blob.bin"), &bytes).expect("write binary");

        let ctx = crate::ToolContext::new(&workspace)
            .expect("tool context")
            .with_access(crate::ToolAccess {
                file_read: true,
                file_write: false,
                shell: false,
                tool: false,
                user_question: false,
            });

        let tool = ReadTool;
        let input = json!({"path": "blob.bin"});
        let result = tool.execute(&ctx, input).await.expect("execute");
        assert!(result.is_error);
        assert!(result.content.contains("not readable as UTF-8 text"));
    }

    fn render_from_content(
        content: &str,
        line_offset: Option<i64>,
        n_lines: Option<usize>,
    ) -> Result<ReadRenderResult, ReadError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let temp = tempfile::NamedTempFile::new().expect("tempfile");
        std::fs::write(temp.path(), content).expect("write temp");
        runtime.block_on(run_read(temp.path(), line_offset, n_lines))
    }
}
