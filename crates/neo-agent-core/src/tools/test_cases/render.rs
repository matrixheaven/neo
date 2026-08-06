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
fn default_window_is_four_hundred_lines() {
    let content = (1..=MAX_LINES + 10)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let result = render_from_content(&content, None, None).unwrap();
    assert_eq!(result.rendered_lines.len(), DEFAULT_LINES);
    assert!(
        result
            .finish_output()
            .contains(&format!("Total lines in file: {}.", MAX_LINES + 10))
    );
}

#[test]
fn description_guides_targeted_reads() {
    let description = ReadTool.description();
    assert!(description.contains("Prefer targeted reads"));
    assert!(description.contains("default 400"));
    assert!(description.contains("line_offset"));
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
