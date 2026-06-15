use crate::core::Line;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffKind {
    Context,
    Add,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffLine {
    kind: DiffKind,
    line_num: usize,
    code: String,
}

#[must_use]
pub fn render_diff_lines_clustered(
    old_text: &str,
    new_text: &str,
    path: &str,
    context_lines: usize,
    max_body_lines: Option<usize>,
) -> Vec<Line> {
    let diff = compute_diff_lines(old_text, new_text);
    let added = diff
        .iter()
        .filter(|line| line.kind == DiffKind::Add)
        .count();
    let removed = diff
        .iter()
        .filter(|line| line.kind == DiffKind::Delete)
        .count();
    let changed = added + removed;
    let mut rows = vec![Line::raw(format!("+{added} -{removed} {path}"))];
    if changed == 0 {
        return rows;
    }

    let change_indices: Vec<usize> = diff
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line.kind != DiffKind::Context).then_some(index))
        .collect();
    let mut emitted = 0usize;
    let mut shown_changes = 0usize;
    let cap = max_body_lines.unwrap_or(usize::MAX);
    let mut previous_end: Option<usize> = None;

    for cluster in build_clusters(&change_indices, diff.len(), context_lines) {
        if emitted >= cap {
            break;
        }
        if let Some(previous_end) = previous_end {
            let gap = cluster.0.saturating_sub(previous_end + 1);
            if gap > 0 && emitted < cap {
                rows.push(Line::raw(format!("     … {gap} unchanged lines …")));
                emitted += 1;
            }
        }
        for index in cluster.0..=cluster.1 {
            if emitted >= cap {
                break;
            }
            let line = &diff[index];
            rows.push(format_diff_row(line));
            emitted += 1;
            if line.kind != DiffKind::Context {
                shown_changes += 1;
            }
        }
        previous_end = Some(cluster.1);
    }

    let hidden = changed.saturating_sub(shown_changes);
    if hidden > 0 {
        rows.push(Line::raw(format!(
            "     … {hidden} more changes hidden (ctrl+o to expand)"
        )));
    }
    rows
}

fn compute_diff_lines(old_text: &str, new_text: &str) -> Vec<DiffLine> {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();
    let mut dp = vec![vec![0usize; new_lines.len() + 1]; old_lines.len() + 1];
    for i in 1..=old_lines.len() {
        for j in 1..=new_lines.len() {
            if old_lines[i - 1] == new_lines[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    let mut reversed = Vec::new();
    let mut i = old_lines.len();
    let mut j = new_lines.len();
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old_lines[i - 1] == new_lines[j - 1] {
            reversed.push(DiffLine {
                kind: DiffKind::Context,
                line_num: j,
                code: new_lines[j - 1].to_owned(),
            });
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            reversed.push(DiffLine {
                kind: DiffKind::Add,
                line_num: j,
                code: new_lines[j - 1].to_owned(),
            });
            j -= 1;
        } else {
            reversed.push(DiffLine {
                kind: DiffKind::Delete,
                line_num: i,
                code: old_lines[i - 1].to_owned(),
            });
            i -= 1;
        }
    }
    reversed.reverse();
    reversed
}

fn build_clusters(changes: &[usize], len: usize, context: usize) -> Vec<(usize, usize)> {
    let Some((&first, rest)) = changes.split_first() else {
        return Vec::new();
    };
    let mut clusters = Vec::new();
    let mut start = first;
    let mut end = first;
    for &index in rest {
        if index.saturating_sub(end) <= context * 2 {
            end = index;
        } else {
            clusters.push((start.saturating_sub(context), (end + context).min(len - 1)));
            start = index;
            end = index;
        }
    }
    clusters.push((start.saturating_sub(context), (end + context).min(len - 1)));
    clusters
}

fn format_diff_row(line: &DiffLine) -> Line {
    let marker = match line.kind {
        DiffKind::Context => ' ',
        DiffKind::Add => '+',
        DiffKind::Delete => '-',
    };
    Line::raw(format!("{:>4} {marker} {}", line.line_num, line.code))
}
