use super::*;

#[test]
fn code_block_has_rounded_borders() {
    let text = "```bash\necho hi\n```";
    let lines = render_markdown(text, 40, &TuiTheme::default(), "● ", "  ");
    let top = lines[0].to_ansi();
    assert!(top.contains('╭'), "top must contain ╭");
    assert!(top.contains('╮'), "top must contain ╮");
    let bottom = lines.last().unwrap().to_ansi();
    assert!(bottom.contains('╰'), "bottom must contain ╰");
    assert!(bottom.contains('╯'), "bottom must contain ╯");
    let all_plain: String = lines
        .iter()
        .map(|l| crate::primitive::strip_ansi(&l.to_ansi()))
        .collect();
    assert!(all_plain.contains('│'), "output must contain side borders");
}

#[test]
fn code_block_width_is_consistent_and_within_bounds() {
    let text = "```rust\nfn main() {\n    println!();\n}\n```";
    for width in [20, 40, 60, 80] {
        let lines = render_markdown(text, width, &TuiTheme::default(), "● ", "  ");
        let first_width = lines[0].visible_width();
        for line in &lines {
            assert_eq!(
                line.visible_width(),
                first_width,
                "all lines must have the same width"
            );
            assert!(
                line.visible_width() <= width,
                "line width {} should be <= {width}",
                line.visible_width()
            );
        }
    }
    // Minimum width: rendering a code fence at 4 columns falls back to
    // unwrapped lines and must not panic.
    let min_lines = render_markdown(text, 4, &TuiTheme::default(), "● ", "  ");
    assert!(
        !min_lines.is_empty(),
        "min-width render must still produce rows"
    );
}

#[test]
fn json_code_block_keeps_right_border_aligned_after_highlighting() {
    let text = r#"```json
{
  "kind": "swarm",
  "swarm_id": "swarm_xxx",
  "status": "completed",
  "aggregate": { "total": 2, "completed": 2, ... },
  "items": [
{"index": 0, "agent_id": "agent_xxx", "status": "completed"},
{"index": 1, "agent_id": "agent_yyy", "status": "completed"}
  ]
}
```"#;
    let lines = render_markdown(text, 100, &TuiTheme::default(), "", "");
    let plain_lines = lines
        .iter()
        .map(|line| crate::primitive::strip_ansi(&line.to_ansi()))
        .collect::<Vec<_>>();
    let expected_width = crate::primitive::visible_width(&plain_lines[0]);

    for (index, raw_line) in lines.iter().map(Line::to_ansi).enumerate() {
        assert!(
            !raw_line.contains(['\n', '\r']),
            "rendered code block row {index} must not contain embedded line breaks: {raw_line:?}"
        );
    }

    for line in &plain_lines {
        assert_eq!(
            crate::primitive::visible_width(line),
            expected_width,
            "code block line should stay inside the same border columns: {line:?}"
        );
    }
}

#[test]
fn code_block_adapts_to_short_content() {
    let text = "```bash\necho hi\n```";
    let width = 40;
    let lines = render_markdown(text, width, &TuiTheme::default(), "● ", "  ");
    // Short content should not expand to the full 40 columns.
    assert!(
        lines[0].visible_width() < width,
        "box should be narrower than full width for short content: {:?}",
        lines[0].to_ansi()
    );
}

#[test]
fn code_block_language_in_header() {
    let text = "```bash\necho hi\n```";
    let lines = render_markdown(text, 40, &TuiTheme::default(), "● ", "  ");
    let top = lines[0].to_ansi();
    assert!(top.contains("bash"), "header must contain language: {top}");
    let all = lines
        .iter()
        .map(|l| crate::primitive::strip_ansi(&l.to_ansi()))
        .collect::<String>();
    assert!(!all.contains("```bash"), "must not use old fence style");
}

#[test]
fn code_block_no_fence_backticks() {
    let text = "```bash\necho hi\n```";
    let all = render_markdown(text, 40, &TuiTheme::default(), "● ", "  ")
        .into_iter()
        .map(|l| crate::primitive::strip_ansi(&l.to_ansi()))
        .collect::<String>();
    assert!(
        !all.contains("```"),
        "output must not contain fence backticks"
    );
}

#[test]
fn code_block_empty_content_renders_box() {
    let text = "```bash\n```";
    let lines = render_markdown(text, 30, &TuiTheme::default(), "● ", "  ");
    let top = lines[0].to_ansi();
    let bottom = lines.last().unwrap().to_ansi();
    assert!(top.contains('╭') && top.contains('╮'));
    assert!(bottom.contains('╰') && bottom.contains('╯'));
}

#[test]
fn code_block_in_list_renders_within_width() {
    let text = "- item\n\n  ```bash\n  echo hi\n  ```\n";
    let width = 40;
    let lines = render_markdown(text, width, &TuiTheme::default(), "● ", "  ");
    for line in &lines {
        assert!(
            line.visible_width() <= width,
            "line width {} should be <= {width}: {:?}",
            line.visible_width(),
            line.to_ansi()
        );
    }
}
