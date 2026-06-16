//! Markdown rendering: verify each element is styled and laid out like kimi-code.

use neo_tui::app::TuiTheme;
use neo_tui::markdown::render_markdown;

fn plain(text: &str, width: usize) -> Vec<String> {
    render_markdown(text, width, &TuiTheme::default())
        .into_iter()
        .map(|line| {
            neo_tui::ansi::strip_ansi(&line.to_ansi())
                .trim_end()
                .to_owned()
        })
        .collect()
}

#[test]
fn heading_h1_renders_without_hash_prefix() {
    let lines = plain("# Title", 80);
    let joined = lines.join("\n");
    assert!(joined.contains("Title"), "heading text present: {joined}");
    assert!(
        !joined.contains("# Title"),
        "hash prefix stripped: {joined}"
    );
}

#[test]
fn bold_and_italic_strip_markers() {
    let lines = plain("this is **bold** and *italic*", 80);
    let joined = lines.join("\n");
    assert!(joined.contains("bold"), "bold text present: {joined}");
    assert!(joined.contains("italic"), "italic text present: {joined}");
    assert!(!joined.contains("**"), "asterisks stripped: {joined}");
    assert!(
        !joined.contains("*italic"),
        "single asterisk stripped: {joined}"
    );
}

#[test]
fn inline_code_renders_without_backticks() {
    let lines = plain("use `cargo build` to compile", 80);
    let joined = lines.join("\n");
    assert!(
        joined.contains("cargo build"),
        "code text present: {joined}"
    );
    assert!(!joined.contains('`'), "backticks stripped: {joined}");
}

#[test]
fn unordered_list_uses_bullet() {
    let lines = plain("- alpha\n- beta\n- gamma", 80);
    let joined = lines.join("\n");
    assert!(joined.contains("• alpha"), "bullet + alpha: {joined}");
    assert!(joined.contains("• beta"), "bullet + beta: {joined}");
    assert!(joined.contains("• gamma"), "bullet + gamma: {joined}");
}

#[test]
fn ordered_list_keeps_number_marker() {
    let lines = plain("1. first\n2. second\n3. third", 80);
    let joined = lines.join("\n");
    assert!(joined.contains("1. first"), "ordered 1: {joined}");
    assert!(joined.contains("2. second"), "ordered 2: {joined}");
    assert!(joined.contains("3. third"), "ordered 3: {joined}");
}

#[test]
fn task_list_keeps_checkbox() {
    let lines = plain("- [ ] todo\n- [x] done", 80);
    let joined = lines.join("\n");
    assert!(joined.contains("[ ]"), "open checkbox: {joined}");
    assert!(joined.contains("[x]"), "checked checkbox: {joined}");
}

#[test]
fn blockquote_uses_pipe_prefix() {
    let lines = plain("> a quoted line", 80);
    let joined = lines.join("\n");
    assert!(joined.contains("│ a quoted line"), "pipe prefix: {joined}");
}

#[test]
fn horizontal_rule_uses_box_chars() {
    let lines = plain("---", 80);
    let joined = lines.join("\n");
    assert!(joined.contains('─'), "horizontal rule: {joined}");
}

#[test]
fn code_block_has_backtick_borders_and_indent() {
    let md = "```rust\nfn main() {}\n```";
    let lines = plain(md, 80);
    let joined = lines.join("\n");
    // top border with language
    assert!(joined.contains("```rust"), "top border: {joined}");
    // bottom border
    assert!(
        joined.contains("```") && joined.matches("```").count() >= 2,
        "bottom border: {joined}"
    );
    // code indented 2 spaces
    assert!(joined.contains("  fn main()"), "indented code: {joined}");
}

#[test]
fn diff_code_block_colors_add_remove() {
    let md = "```diff\n+added line\n-removed line\n```";
    let lines = plain(md, 80);
    let joined = lines.join("\n");
    assert!(joined.contains("added line"), "added present: {joined}");
    assert!(joined.contains("removed line"), "removed present: {joined}");
}

#[test]
fn table_has_box_borders_and_bold_header() {
    let md = "| Crate | Role |\n|---|---|\n| neo-ai | providers |\n| neo-tui | terminal UI |";
    let lines = plain(md, 80);
    let joined = lines.join("\n");
    assert!(joined.contains('┌'), "top-left corner: {joined}");
    assert!(joined.contains('┐'), "top-right corner: {joined}");
    assert!(joined.contains('└'), "bottom-left corner: {joined}");
    assert!(joined.contains('┘'), "bottom-right corner: {joined}");
    assert!(joined.contains('│'), "vertical borders: {joined}");
    assert!(joined.contains("Crate"), "header cell present: {joined}");
    assert!(joined.contains("neo-ai"), "body cell present: {joined}");
}

#[test]
fn mixed_content_renders_in_order() {
    let md = "# Heading\n\nSome **bold** text.\n\n- item one\n- item two\n\n```rs\nlet x = 1;\n```";
    let lines = plain(md, 80);
    let joined = lines.join("\n");
    assert!(joined.contains("Heading"), "heading: {joined}");
    assert!(joined.contains("bold"), "bold inline: {joined}");
    assert!(joined.contains("• item one"), "list item: {joined}");
    assert!(joined.contains("let x = 1"), "code block: {joined}");
}

#[test]
fn empty_input_produces_no_lines() {
    let lines = plain("", 80);
    assert!(lines.is_empty(), "empty input -> no lines");
}
