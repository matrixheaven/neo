use std::collections::BTreeSet;

use crate::ansi::{clip_plain_to_width, truncate_to_width, visible_width};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffStats {
    pub files_changed: usize,
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffModel {
    files: Vec<DiffFile>,
    stats: DiffStats,
}

impl DiffModel {
    #[must_use]
    pub fn from_tool_details(details: &serde_json::Value) -> Option<Self> {
        let diff = details.get("diff")?.as_str()?;
        Self::parse_unified(diff)
    }

    #[must_use]
    pub fn parse_unified(diff: &str) -> Option<Self> {
        let mut files = Vec::new();
        let mut current_file: Option<DiffFile> = None;
        let mut current_hunk: Option<DiffHunk> = None;
        let mut pending_old_path: Option<String> = None;

        for line in diff.lines() {
            if let Some(path) = line.strip_prefix("--- ") {
                flush_hunk(&mut current_file, &mut current_hunk);
                if let Some(file) = current_file.take() {
                    files.push(file);
                }
                pending_old_path = Some(normalize_diff_path(path));
                continue;
            }
            if let Some(path) = line.strip_prefix("+++ ") {
                let old_path = pending_old_path
                    .take()
                    .unwrap_or_else(|| normalize_diff_path(path));
                current_file = Some(DiffFile {
                    old_path,
                    new_path: normalize_diff_path(path),
                    hunks: Vec::new(),
                });
                continue;
            }
            if line.starts_with("@@") {
                flush_hunk(&mut current_file, &mut current_hunk);
                let (old_start, new_start) = parse_hunk_starts(line);
                current_hunk = Some(DiffHunk {
                    header: line.to_owned(),
                    old_start,
                    new_start,
                    lines: Vec::new(),
                    stats: DiffStats::default(),
                });
                continue;
            }

            let Some(diff_line) = DiffLine::parse(line) else {
                continue;
            };
            if current_hunk.is_none() {
                continue;
            }
            if let Some(hunk) = &mut current_hunk {
                hunk.stats.add_line(&diff_line);
                hunk.lines.push(diff_line);
            }
        }

        flush_hunk(&mut current_file, &mut current_hunk);
        if let Some(file) = current_file {
            files.push(file);
        }

        files.retain(|file| !file.hunks.is_empty());
        if files.is_empty() {
            return None;
        }

        let mut stats = DiffStats {
            files_changed: files.len(),
            ..DiffStats::default()
        };
        for file in &files {
            for hunk in &file.hunks {
                stats.added += hunk.stats.added;
                stats.removed += hunk.stats.removed;
            }
        }
        if stats.added == 0 && stats.removed == 0 {
            return None;
        }

        Some(Self { files, stats })
    }

    #[must_use]
    pub fn files(&self) -> &[DiffFile] {
        &self.files
    }

    #[must_use]
    pub const fn stats(&self) -> &DiffStats {
        &self.stats
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub old_path: String,
    pub new_path: String,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: String,
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<DiffLine>,
    pub stats: DiffStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    Context(String),
    Added(String),
    Removed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRenderLineKind {
    Summary,
    Context,
    Added,
    Removed,
    Separator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRenderLine {
    pub kind: DiffRenderLineKind,
    pub text: String,
}

impl DiffLine {
    fn parse(line: &str) -> Option<Self> {
        if let Some(text) = line.strip_prefix('+') {
            Some(Self::Added(text.to_owned()))
        } else if let Some(text) = line.strip_prefix('-') {
            Some(Self::Removed(text.to_owned()))
        } else {
            line.strip_prefix(' ')
                .map(|text| Self::Context(text.to_owned()))
        }
    }

    fn display_text(&self) -> String {
        match self {
            Self::Context(text) => format!(" {text}"),
            Self::Added(text) => format!("+{text}"),
            Self::Removed(text) => format!("-{text}"),
        }
    }
}

impl DiffStats {
    fn add_line(&mut self, line: &DiffLine) {
        match line {
            DiffLine::Added(_) => self.added += 1,
            DiffLine::Removed(_) => self.removed += 1,
            DiffLine::Context(_) => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRenderState {
    model: DiffModel,
    active_file: usize,
    active_hunk: usize,
    folded_files: BTreeSet<usize>,
    folded_hunks: BTreeSet<(usize, usize)>,
}

impl DiffRenderState {
    #[must_use]
    pub fn new(model: DiffModel) -> Self {
        Self {
            model,
            active_file: 0,
            active_hunk: 0,
            folded_files: BTreeSet::new(),
            folded_hunks: BTreeSet::new(),
        }
    }

    #[must_use]
    pub const fn active_file_index(&self) -> usize {
        self.active_file
    }

    #[must_use]
    pub const fn active_hunk_index(&self) -> usize {
        self.active_hunk
    }

    #[must_use]
    pub const fn stats(&self) -> &DiffStats {
        self.model.stats()
    }

    pub fn next_file(&mut self) {
        if self.active_file + 1 < self.model.files.len() {
            self.active_file += 1;
            self.active_hunk = 0;
        }
    }

    pub fn previous_file(&mut self) {
        if self.active_file > 0 {
            self.active_file -= 1;
            self.active_hunk = 0;
        }
    }

    pub fn next_hunk(&mut self) {
        if self.model.files.is_empty() {
            return;
        }
        if self.active_hunk + 1 < self.model.files[self.active_file].hunks.len() {
            self.active_hunk += 1;
            return;
        }
        if self.active_file + 1 < self.model.files.len() {
            self.active_file += 1;
            self.active_hunk = 0;
        }
    }

    pub fn previous_hunk(&mut self) {
        if self.model.files.is_empty() {
            return;
        }
        if self.active_hunk > 0 {
            self.active_hunk -= 1;
            return;
        }
        if self.active_file > 0 {
            self.active_file -= 1;
            self.active_hunk = self.model.files[self.active_file]
                .hunks
                .len()
                .saturating_sub(1);
        }
    }

    pub fn toggle_active_hunk_fold(&mut self) {
        let key = (self.active_file, self.active_hunk);
        if !self.folded_hunks.remove(&key) {
            self.folded_hunks.insert(key);
        }
    }

    pub fn toggle_active_file_fold(&mut self) {
        if !self.folded_files.remove(&self.active_file) {
            self.folded_files.insert(self.active_file);
        }
    }

    pub fn unfold_active_hunk(&mut self) {
        self.folded_hunks
            .remove(&(self.active_file, self.active_hunk));
    }

    #[must_use]
    pub fn is_active_hunk_folded(&self) -> bool {
        self.folded_hunks
            .contains(&(self.active_file, self.active_hunk))
    }

    #[must_use]
    pub fn is_active_file_folded(&self) -> bool {
        self.folded_files.contains(&self.active_file)
    }

    #[must_use]
    pub fn copy_active_hunk(&self) -> Option<String> {
        let file = self.model.files.get(self.active_file)?;
        let hunk = file.hunks.get(self.active_hunk)?;
        let mut copied = diff_file_header(file);
        copied.push_str(&hunk.to_unified());
        Some(copied)
    }

    #[must_use]
    pub fn copy_active_file(&self) -> Option<String> {
        let file = self.model.files.get(self.active_file)?;
        let mut copied = diff_file_header(file);
        for hunk in &file.hunks {
            copied.push_str(&hunk.to_unified());
        }
        Some(copied)
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        self.render_display_lines(width)
            .into_iter()
            .map(|line| line.text)
            .collect()
    }

    #[must_use]
    pub fn render_display_lines(&self, width: usize) -> Vec<DiffRenderLine> {
        let mut lines = Vec::new();
        let line_number_width = self.line_number_width();
        for (file_index, file) in self.model.files.iter().enumerate() {
            if file_index > 0 {
                lines.push(DiffRenderLine {
                    kind: DiffRenderLineKind::Separator,
                    text: "⋮".to_owned(),
                });
            }
            if self.folded_files.contains(&file_index) {
                lines.push(DiffRenderLine {
                    kind: DiffRenderLineKind::Summary,
                    text: fit_width(
                        &format!(
                            "+{} -{} {} folded",
                            file.stats().added,
                            file.stats().removed,
                            file.display_path()
                        ),
                        width,
                    ),
                });
                continue;
            }

            lines.push(DiffRenderLine {
                kind: DiffRenderLineKind::Summary,
                text: truncate_to_width(
                    &format!(
                        "+{} -{} {}",
                        file.stats().added,
                        file.stats().removed,
                        file.display_path()
                    ),
                    width,
                ),
            });
            for (hunk_index, hunk) in file.hunks.iter().enumerate() {
                if hunk_index > 0 {
                    lines.push(DiffRenderLine {
                        kind: DiffRenderLineKind::Separator,
                        text: format!(" {}⋮", " ".repeat(line_number_width)),
                    });
                }
                if self.folded_hunks.contains(&(file_index, hunk_index)) {
                    lines.push(DiffRenderLine {
                        kind: DiffRenderLineKind::Separator,
                        text: fit_width(
                            &format!(
                                " {}⋮ folded {} changes",
                                " ".repeat(line_number_width),
                                hunk.change_count()
                            ),
                            width,
                        ),
                    });
                    continue;
                }

                render_hunk_lines(hunk, line_number_width, width, &mut lines);
            }
        }
        lines
    }

    fn line_number_width(&self) -> usize {
        self.model
            .files
            .iter()
            .flat_map(|file| &file.hunks)
            .flat_map(max_hunk_line_number)
            .max()
            .unwrap_or(1)
            .to_string()
            .len()
            .max(1)
    }
}

impl DiffFile {
    fn display_path(&self) -> &str {
        if self.new_path.is_empty() {
            &self.old_path
        } else {
            &self.new_path
        }
    }

    fn change_count(&self) -> usize {
        self.hunks
            .iter()
            .map(|hunk| hunk.stats.added + hunk.stats.removed)
            .sum()
    }

    fn stats(&self) -> DiffStats {
        let mut stats = DiffStats::default();
        for hunk in &self.hunks {
            stats.added += hunk.stats.added;
            stats.removed += hunk.stats.removed;
        }
        stats
    }
}

impl DiffHunk {
    fn change_count(&self) -> usize {
        self.stats.added + self.stats.removed
    }

    fn to_unified(&self) -> String {
        let mut copied = String::new();
        copied.push_str(&self.header);
        copied.push('\n');
        for line in &self.lines {
            copied.push_str(&line.display_text());
            copied.push('\n');
        }
        copied
    }
}

fn render_hunk_lines(
    hunk: &DiffHunk,
    line_number_width: usize,
    width: usize,
    lines: &mut Vec<DiffRenderLine>,
) {
    let mut old_line = hunk.old_start;
    let mut new_line = hunk.new_start;
    for line in &hunk.lines {
        match line {
            DiffLine::Context(text) => {
                push_diff_line(
                    DiffRenderLineKind::Context,
                    new_line,
                    ' ',
                    text,
                    line_number_width,
                    width,
                    lines,
                );
                old_line += 1;
                new_line += 1;
            }
            DiffLine::Added(text) => {
                push_diff_line(
                    DiffRenderLineKind::Added,
                    new_line,
                    '+',
                    text,
                    line_number_width,
                    width,
                    lines,
                );
                new_line += 1;
            }
            DiffLine::Removed(text) => {
                push_diff_line(
                    DiffRenderLineKind::Removed,
                    old_line,
                    '-',
                    text,
                    line_number_width,
                    width,
                    lines,
                );
                old_line += 1;
            }
        }
    }
}

fn push_diff_line(
    kind: DiffRenderLineKind,
    line_number: usize,
    sign: char,
    text: &str,
    line_number_width: usize,
    width: usize,
    lines: &mut Vec<DiffRenderLine>,
) {
    let prefix = format!(" {line_number:>line_number_width$} {sign} ");
    let continuation_prefix = " ".repeat(visible_width(&prefix));
    let content_width = width.saturating_sub(visible_width(&prefix)).max(1);
    let chunks = wrap_plain(text, content_width);
    for (index, chunk) in chunks.into_iter().enumerate() {
        let text = if index == 0 {
            format!("{prefix}{chunk}")
        } else {
            format!("{continuation_prefix}{chunk}")
        };
        lines.push(DiffRenderLine { kind, text });
    }
}

fn wrap_plain(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut remaining = text;
    let mut rows = Vec::new();
    while !remaining.is_empty() {
        let chunk = clip_plain_to_width(remaining, width.max(1));
        if chunk.is_empty() {
            break;
        }
        remaining = &remaining[chunk.len()..];
        rows.push(chunk);
    }
    rows
}

fn max_hunk_line_number(hunk: &DiffHunk) -> Option<usize> {
    let mut old_line = hunk.old_start;
    let mut new_line = hunk.new_start;
    let mut max_line = None;
    for line in &hunk.lines {
        match line {
            DiffLine::Context(_) => {
                max_line = Some(max_line.unwrap_or(0).max(new_line));
                old_line += 1;
                new_line += 1;
            }
            DiffLine::Added(_) => {
                max_line = Some(max_line.unwrap_or(0).max(new_line));
                new_line += 1;
            }
            DiffLine::Removed(_) => {
                max_line = Some(max_line.unwrap_or(0).max(old_line));
                old_line += 1;
            }
        }
    }
    max_line
}

fn diff_file_header(file: &DiffFile) -> String {
    format!("--- {}\n+++ {}\n", file.old_path, file.new_path)
}

fn flush_hunk(current_file: &mut Option<DiffFile>, current_hunk: &mut Option<DiffHunk>) {
    let Some(hunk) = current_hunk.take() else {
        return;
    };
    let Some(file) = current_file else {
        return;
    };
    file.hunks.push(hunk);
}

fn normalize_diff_path(path: &str) -> String {
    path.trim()
        .strip_prefix("a/")
        .or_else(|| path.trim().strip_prefix("b/"))
        .unwrap_or_else(|| path.trim())
        .to_owned()
}

fn parse_hunk_starts(header: &str) -> (usize, usize) {
    let mut parts = header.split_whitespace();
    let _at = parts.next();
    let old_part = parts.next().unwrap_or("-1");
    let new_part = parts.next().unwrap_or("+1");
    (
        parse_hunk_start(old_part, '-').unwrap_or(1),
        parse_hunk_start(new_part, '+').unwrap_or(1),
    )
}

fn parse_hunk_start(part: &str, prefix: char) -> Option<usize> {
    let part = part.strip_prefix(prefix)?;
    let start = part.split(',').next()?;
    start.parse().ok()
}

fn fit_width(text: &str, width: usize) -> String {
    if width == 0 || text.chars().count() <= width {
        return text.to_owned();
    }
    text.chars().take(width).collect()
}
