use super::*;

#[test]
fn lang_from_path_maps_common_extensions() {
    let cases = [
        ("main.rs", Some("rust")),
        ("lib.ts", Some("typescript")),
        ("app.tsx", Some("typescript")),
        ("index.js", Some("javascript")),
        ("page.jsx", Some("javascript")),
        ("script.py", Some("python")),
        ("main.go", Some("go")),
        ("Foo.java", Some("java")),
        ("deploy.sh", Some("bash")),
        ("config.json", Some("json")),
        ("values.yaml", Some("yaml")),
        ("values.yml", Some("yaml")),
        ("Cargo.toml", Some("toml")),
        ("README.md", Some("markdown")),
        ("style.css", Some("css")),
        ("index.html", Some("html")),
        ("query.sql", Some("sql")),
        ("foo.c", Some("c")),
        ("foo.h", Some("c")),
        ("foo.cpp", Some("cpp")),
        ("foo.hpp", Some("cpp")),
        ("no_extension", None),
        ("file.unknown", None),
    ];
    for (path, expected) in cases {
        assert_eq!(
            lang_from_path(path),
            expected,
            "extension mismatch for {path}"
        );
    }
}

#[test]
fn highlight_code_lines_returns_lines_for_known_lang() {
    let theme = TuiTheme::default();
    let lines = highlight_code_lines("fn main() {}", "main.rs", &theme);
    assert_eq!(lines.len(), 1);
    assert!(!lines[0].is_empty());
}

#[test]
fn highlight_code_lines_falls_back_for_unknown_lang() {
    let theme = TuiTheme::default();
    let lines = highlight_code_lines("hello world", "file.unknown", &theme);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].len(), 1);
    assert_eq!(lines[0][0].text(), "hello world");
}

#[test]
fn highlight_code_lines_preserves_trailing_blank_lines() {
    let theme = TuiTheme::default();
    let content = "---\nkey: value\n---\n\n# Title\n\n";
    let lines = highlight_code_lines(content, "plan.md", &theme);

    assert_eq!(lines.len(), content.lines().count());
}
