use super::*;

#[test]
fn expand_braces_simple() {
    assert_eq!(expand_braces("*.rs"), vec!["*.rs"]);
}

#[test]
fn expand_braces_alternation() {
    let result = expand_braces("*.{rs,toml}");
    assert!(result.contains(&"*.rs".to_string()));
    assert!(result.contains(&"*.toml".to_string()));
    assert_eq!(result.len(), 2);
}

#[test]
fn expand_braces_prefix() {
    assert_eq!(expand_braces("{foo,bar}.rs"), vec!["foo.rs", "bar.rs"]);
}

#[test]
fn expand_braces_multiple_groups() {
    let result = expand_braces("{a,b}/{c,d}");
    assert!(result.contains(&"a/c".to_string()));
    assert!(result.contains(&"a/d".to_string()));
    assert!(result.contains(&"b/c".to_string()));
    assert!(result.contains(&"b/d".to_string()));
    assert_eq!(result.len(), 4);
}
