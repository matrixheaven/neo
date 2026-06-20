use schemars::JsonSchema;
use serde::Deserialize;

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, parse_input, schema};

const MAX_LINES: usize = 1000;
const MAX_LINE_LENGTH: usize = 2000;
const MAX_BYTES: usize = 100 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadInput {
    path: std::path::PathBuf,
    /// 1-based line number to start reading from. Omit to start at line 1.
    /// Negative values read from the end of the file.
    line_offset: Option<i64>,
    /// Maximum number of lines to read. Omit to read up to the internal cap.
    n_lines: Option<usize>,
}

pub struct ReadTool;

impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "Read"
    }

    fn description(&self) -> &'static str {
        "Read a UTF-8 text file from the workspace.\
        \
        If the user provides a concrete file path, call Read directly. Do not use Glob, ls, or \
        other pre-checks for known text file paths; missing or invalid paths return errors you can \
        handle. Use Glob for pattern searches and Bash `ls` for directories.\
        \
        Parameters:\
        - path: Path to the text file. Relative paths resolve against the working directory; paths \
          outside the working directory must be absolute.\
        - line_offset: 1-based line number to start reading from. Omit to start at line 1. Negative \
          values read from the end (e.g. -100 reads the last 100 lines). The absolute value must \
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
            let path = ctx.resolve_workspace_path(&input.path)?;
            let content = tokio::fs::read_to_string(&path)
                .await
                .map_err(ToolError::Io)?;

            let result =
                render_read(&content, input.line_offset, input.n_lines).map_err(|message| {
                    ToolError::InvalidInput {
                        tool: self.name().to_owned(),
                        message,
                    }
                })?;

            Ok(ToolResult::ok(result.finish_output()))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEndingStyle {
    Lf,
    Crlf,
    Mixed,
}

impl LineEndingStyle {
    fn detect(text: &str) -> Self {
        let mut has_crlf = false;
        let mut has_lf = false;
        let mut has_lone_cr = false;

        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\r' {
                if chars.peek() == Some(&'\n') {
                    has_crlf = true;
                    chars.next();
                } else {
                    has_lone_cr = true;
                }
            } else if ch == '\n' {
                has_lf = true;
            }
        }

        if has_lone_cr || (has_crlf && has_lf) {
            Self::Mixed
        } else if has_crlf {
            Self::Crlf
        } else {
            Self::Lf
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
                "Mixed or lone carriage-return line endings are shown as \\r. Use exact \\r\\n or \\r escapes in Edit.old_string for those lines.".to_owned(),
            );
        }

        parts.join(" ")
    }
}

fn render_read(
    content: &str,
    line_offset: Option<i64>,
    n_lines: Option<usize>,
) -> Result<ReadRenderResult, String> {
    let line_offset = line_offset.unwrap_or(1);
    if line_offset == 0 {
        return Err("line_offset must not be 0".to_owned());
    }
    let abs_offset = usize::try_from(line_offset.unsigned_abs()).unwrap_or(usize::MAX);
    if abs_offset > MAX_LINES {
        return Err(format!(
            "absolute value of line_offset must not exceed {MAX_LINES}"
        ));
    }

    let requested_lines = n_lines.unwrap_or(MAX_LINES);
    if requested_lines == 0 {
        return Err("n_lines must be greater than 0".to_owned());
    }

    let effective_limit = requested_lines.min(MAX_LINES);
    let line_ending_style = LineEndingStyle::detect(content);

    let all_lines: Vec<(usize, &str)> = content
        .split_inclusive('\n')
        .map(strip_trailing_lf)
        .enumerate()
        .map(|(idx, line)| (idx + 1, line))
        .collect();

    let total_lines = all_lines.len();

    let start_idx = if line_offset < 0 {
        total_lines.saturating_sub(abs_offset)
    } else {
        abs_offset.saturating_sub(1)
    };

    let candidate_lines: Vec<(usize, &str)> = all_lines
        .into_iter()
        .skip(start_idx)
        .take(effective_limit)
        .collect();

    let max_lines_reached = effective_limit >= MAX_LINES
        && candidate_lines.len() >= effective_limit
        && start_idx + effective_limit < total_lines;

    let mut rendered_lines = Vec::new();
    let mut truncated_line_numbers = Vec::new();
    let mut bytes_used: usize = 0;
    let mut max_bytes_reached = false;

    for (line_no, raw_line) in candidate_lines {
        let (truncated, was_truncated) = truncate_line(raw_line, MAX_LINE_LENGTH);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_whole_small_file() {
        let content = "line one\nline two\nline three\n";
        let result = render_read(content, None, None).unwrap();
        assert_eq!(result.rendered_lines.len(), 3);
        assert_eq!(result.rendered_lines[0], "1\tline one");
        assert_eq!(result.rendered_lines[2], "3\tline three");
        assert!(result.finish_output().contains("Total lines in file: 3."));
        assert!(result.finish_output().contains("End of file reached."));
    }

    #[test]
    fn reads_from_positive_offset() {
        let content = "a\nb\nc\nd\ne\n";
        let result = render_read(content, Some(3), Some(2)).unwrap();
        assert_eq!(result.rendered_lines.len(), 2);
        assert_eq!(result.rendered_lines[0], "3\tc");
        assert_eq!(result.rendered_lines[1], "4\td");
        assert!(result.finish_output().contains("starting from line 3"));
    }

    #[test]
    fn reads_from_negative_offset() {
        let content = "a\nb\nc\nd\ne\n";
        let result = render_read(content, Some(-2), None).unwrap();
        assert_eq!(result.rendered_lines.len(), 2);
        assert_eq!(result.rendered_lines[0], "4\td");
        assert_eq!(result.rendered_lines[1], "5\te");
    }

    #[test]
    fn zero_line_offset_is_rejected() {
        let err = render_read("x\n", Some(0), None).unwrap_err();
        assert!(err.contains("line_offset must not be 0"));
    }

    #[test]
    fn zero_n_lines_is_rejected() {
        let err = render_read("x\n", None, Some(0)).unwrap_err();
        assert!(err.contains("n_lines must be greater than 0"));
    }

    #[test]
    fn line_offset_too_negative_is_rejected() {
        let err = render_read("x\n", Some(-1001), None).unwrap_err();
        assert!(err.contains("absolute value of line_offset"));
    }

    #[test]
    fn max_lines_cap_is_reported() {
        let content = (1..=MAX_LINES + 10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let result = render_read(&content, None, None).unwrap();
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
        let result = render_read(&content, None, None).unwrap();
        assert_eq!(result.rendered_lines.len(), 1);
        assert!(result.rendered_lines[0].ends_with("..."));
        assert!(result.truncated_line_numbers.contains(&1));
        assert!(result.finish_output().contains("Lines [1] were truncated."));
    }

    #[test]
    fn crlf_is_normalized() {
        let content = "one\r\ntwo\r\n";
        let result = render_read(content, None, None).unwrap();
        assert_eq!(result.rendered_lines[0], "1\tone");
        assert_eq!(result.rendered_lines[1], "2\ttwo");
    }

    #[test]
    fn mixed_line_endings_show_escape() {
        let content = "one\r\ntwo\rthree\n";
        let result = render_read(content, None, None).unwrap();
        assert_eq!(result.rendered_lines[0], "1\tone\\r");
        assert_eq!(result.rendered_lines[1], "2\ttwo\\rthree");
        assert!(
            result
                .finish_message()
                .contains("Mixed or lone carriage-return line endings are shown as \\r")
        );
    }
}
