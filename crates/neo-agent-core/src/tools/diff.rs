use std::fmt::Write;

use similar::{Algorithm, ChangeTag, TextDiff};

pub(super) fn unified_diff(path: &str, before: &str, after: &str) -> String {
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_lines(before, after);

    let mut result = format!("--- {path}\n+++ {path}\n");

    for group in diff.grouped_ops(3) {
        let (first, last) = (group.first().unwrap(), group.last().unwrap());

        let (old_line, old_count) = hunk_range(
            first.old_range().start,
            last.old_range().end - first.old_range().start,
        );
        let (new_line, new_count) = hunk_range(
            first.new_range().start,
            last.new_range().end - first.new_range().start,
        );

        let _ = writeln!(
            result,
            "@@ -{old_line},{old_count} +{new_line},{new_count} @@"
        );

        for op in &group {
            for change in diff.iter_changes(op) {
                let prefix = match change.tag() {
                    ChangeTag::Equal => ' ',
                    ChangeTag::Delete => '-',
                    ChangeTag::Insert => '+',
                };
                let line = change.value();
                result.push(prefix);
                result.push_str(line);
                if !line.ends_with('\n') {
                    result.push('\n');
                }
            }
        }
    }

    result
}

pub(super) fn diff_stats(diff: &str) -> (usize, usize) {
    let mut lines = diff.lines();
    let _old_header = lines.next();
    let _new_header = lines.next();
    lines.fold((0, 0), |(added, removed), line| {
        if line.starts_with('+') {
            (added + 1, removed)
        } else if line.starts_with('-') {
            (added, removed + 1)
        } else {
            (added, removed)
        }
    })
}

/// Convert a 0-based half-open `[start, start+len)` range into the `(line, count)`
/// pair used in unified-diff hunk headers (`@@ -line,count +line,count @@`).
fn hunk_range(start: usize, len: usize) -> (usize, usize) {
    if len == 0 {
        (start, 0)
    } else {
        (start + 1, len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_stats_counts_body_lines_that_resemble_headers() {
        let diff = "--- file.txt\n+++ file.txt\n@@ -1,2 +1,2 @@\n---removed body\n+++added body\n";

        assert_eq!(diff_stats(diff), (1, 1));
    }
}
