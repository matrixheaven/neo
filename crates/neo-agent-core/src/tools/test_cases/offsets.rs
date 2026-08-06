use super::*;

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
    let result = render_from_content(&content, None, Some(MAX_LINES)).unwrap();
    assert_eq!(result.rendered_lines.len(), MAX_LINES);
    assert!(
        result
            .finish_output()
            .contains(&format!("Max {MAX_LINES} lines reached."))
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
