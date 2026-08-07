use super::*;

#[test]
fn render_basic_box() {
    let comp = PlanBoxComponent::new("# Plan\n- Step 1", Some("/tmp/abc.md".to_string()));
    let lines = comp.render(40, &TuiTheme::default());
    assert!(lines.len() >= 3); // top border + content lines + bottom border
    let top = lines[0].to_ansi();
    assert!(top.contains("plan: abc.md"));
}

#[test]
fn render_title_uses_windows_path_basename() {
    let comp = PlanBoxComponent::new("# Plan", Some(r"C:\Users\alice\plan.md".to_string()));
    let lines = comp.render(50, &TuiTheme::default());
    let top = crate::primitive::strip_ansi(&lines[0].to_ansi());

    assert!(top.contains("plan: plan.md"), "top border: {top}");
    assert!(!top.contains(r"C:\Users"), "top border: {top}");
}

#[test]
fn render_empty_content() {
    let comp = PlanBoxComponent::new("", None);
    let lines = comp.render(20, &TuiTheme::default());
    assert!(lines.len() >= 3);
}

#[test]
fn top_border_has_right_corner() {
    let comp = PlanBoxComponent::new("hello", None);
    let lines = comp.render(40, &TuiTheme::default());
    let top = lines[0].to_ansi();
    assert!(
        top.contains('\u{2510}'),
        "top border must end with ┐, got: {top}"
    );
}

#[test]
fn top_border_fills_remaining_width_with_horizontal_rule() {
    let comp = PlanBoxComponent::new("hello", Some("/tmp/abc.md".to_string()));
    let lines = comp.render(40, &TuiTheme::default());
    let top = crate::primitive::strip_ansi(&lines[0].to_ansi());
    // Title " plan: abc.md " leaves the rest of the top border to be filled with ─.
    assert!(
        top.contains("plan: abc.md"),
        "top border should contain title: {top}"
    );
    assert!(
        top.ends_with('\u{2510}'),
        "top border should end with ┐: {top}"
    );
    assert!(
        top.contains('\u{2500}'),
        "top border should use horizontal rule between title and ┐: {top}"
    );
    assert_eq!(
        crate::primitive::visible_width(&top),
        40,
        "top border should span full width: {top}"
    );
}

#[test]
fn bottom_border_has_right_corner() {
    let comp = PlanBoxComponent::new("hello", None);
    let lines = comp.render(40, &TuiTheme::default());
    let bottom = lines.last().unwrap().to_ansi();
    assert!(
        bottom.contains('\u{2519}'),
        "bottom border must end with ┘, got: {bottom}"
    );
}

#[test]
fn wrap_text_long_line() {
    let wrapped = PlanBoxComponent::wrap_text("aaaa bbbb cccc dddd", 10);
    assert!(wrapped.len() > 1);
}

#[test]
fn markdown_content_renders_in_box() {
    let comp = PlanBoxComponent::new("# Title\n\nSome text", Some("/tmp/plan.md".to_string()));
    let lines = comp.render(60, &TuiTheme::default());
    assert!(lines.len() >= 4, "should have border + content lines");
    // The content should contain "Title" somewhere
    let all_text = lines.iter().map(Line::to_ansi).collect::<String>();
    assert!(
        all_text.contains("Title"),
        "markdown content should be rendered"
    );
    // Should have proper border structure
    let top = lines[0].to_ansi();
    assert!(top.contains('\u{2510}'), "top border must have ┐");
    let bottom = lines.last().unwrap().to_ansi();
    assert!(bottom.contains('\u{2519}'), "bottom border must have ┘");
}

#[test]
fn non_markdown_file_uses_plain_text() {
    let comp = PlanBoxComponent::new("plain text", Some("/tmp/plan.txt".to_string()));
    let lines = comp.render(40, &TuiTheme::default());
    assert!(lines.len() >= 3);
    let content = lines[1].to_ansi();
    assert!(content.contains("plain text"));
}

#[test]
fn rendered_lines_fit_width() {
    let comp = PlanBoxComponent::new(
        "# Title\n\nSome fairly long text that should wrap within the box.",
        Some("/tmp/plan.md".to_string()),
    );
    for width in [20, 40, 60, 80] {
        let lines = comp.render(width, &TuiTheme::default());
        for line in &lines {
            assert!(
                line.visible_width() <= width,
                "line width {} should be <= {width}: {:?}",
                line.visible_width(),
                line.to_ansi()
            );
        }
    }
}

#[test]
fn rendered_lines_are_exactly_width() {
    let comp = PlanBoxComponent::new(
        "# Title\n\nSome text that may wrap.",
        Some("/tmp/plan.md".to_string()),
    );
    for width in [20, 40, 60, 80] {
        let lines = comp.render(width, &TuiTheme::default());
        assert!(!lines.is_empty(), "should render at width {width}");
        for line in &lines {
            assert_eq!(
                line.visible_width(),
                width,
                "every rendered line should be exactly {width} columns: {:?}",
                line.to_ansi()
            );
        }
    }
}

#[test]
fn box_has_left_margin() {
    let comp = PlanBoxComponent::new("hello", Some("/tmp/plan.md".to_string()));
    let lines = comp.render(40, &TuiTheme::default());
    let top = lines[0].to_ansi();
    let plain = crate::primitive::strip_ansi(&top);
    assert!(
        plain.starts_with("  ┌"),
        "box should start with a 2-space left margin"
    );
}

#[test]
fn top_and_bottom_borders_have_same_width() {
    let comp = PlanBoxComponent::new("hello", Some("/tmp/plan.md".to_string()));
    let lines = comp.render(40, &TuiTheme::default());
    let top = lines.first().unwrap().visible_width();
    let bottom = lines.last().unwrap().visible_width();
    assert_eq!(top, bottom);
    assert_eq!(top, 40);
}

#[test]
fn source_mode_preserves_whitespace_and_reports_full_viewport() {
    let source = "function run()\n    local  x = 'a  b' -- keep  spaces\nend";
    let lines = PlanBoxComponent::source(source, "lua").render(100, &TuiTheme::default());
    let rendered = lines
        .iter()
        .map(|line| crate::primitive::strip_ansi(&line.to_ansi()))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("lua source · lines 1-3 / 3"));
    assert!(rendered.contains("    local  x = 'a  b' -- keep  spaces"));
}

#[test]
fn source_mode_wraps_long_lines_without_dropping_characters() {
    let source = "0123456789abcdefghijklmnopqrstuvwxyz";
    let highlighted = highlight_code_lines(source, "workflow.lua", &TuiTheme::default());
    let wrapped = wrap_spans(&highlighted[0], 7);
    let reconstructed = wrapped
        .into_iter()
        .flatten()
        .map(|span| span.text().to_owned())
        .collect::<String>();

    assert_eq!(reconstructed, source);
}
