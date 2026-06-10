use std::collections::BTreeSet;

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
                current_hunk = Some(DiffHunk {
                    header: line.to_owned(),
                    lines: Vec::new(),
                    stats: DiffStats::default(),
                });
                continue;
            }

            let Some(diff_line) = DiffLine::parse(line) else {
                continue;
            };
            if current_file.is_none() {
                current_file = Some(DiffFile {
                    old_path: pending_old_path.take().unwrap_or_default(),
                    new_path: String::new(),
                    hunks: Vec::new(),
                });
            }
            if current_hunk.is_none() {
                current_hunk = Some(DiffHunk {
                    header: "@@".to_owned(),
                    lines: Vec::new(),
                    stats: DiffStats::default(),
                });
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
    pub lines: Vec<DiffLine>,
    pub stats: DiffStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    Context(String),
    Added(String),
    Removed(String),
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
        let mut lines = Vec::new();
        for (file_index, file) in self.model.files.iter().enumerate() {
            let active_file = file_index == self.active_file;
            let file_prefix = if active_file { ">" } else { " " };
            let file_change_count = file.change_count();
            let hunk_label = if file.hunks.len() == 1 {
                "1 hunk".to_owned()
            } else {
                format!("{} hunks", file.hunks.len())
            };
            if self.folded_files.contains(&file_index) {
                lines.push(fit_width(
                    &format!(
                        "{file_prefix} {} folded {hunk_label}, {file_change_count} changes",
                        file.display_path()
                    ),
                    width,
                ));
                continue;
            }

            lines.push(fit_width(
                &format!(
                    "{file_prefix} {} ({hunk_label}, {file_change_count} changes)",
                    file.display_path()
                ),
                width,
            ));
            lines.push(format!("--- {}", file.old_path));
            lines.push(format!("+++ {}", file.new_path));
            for (hunk_index, hunk) in file.hunks.iter().enumerate() {
                let active = file_index == self.active_file && hunk_index == self.active_hunk;
                let prefix = if active { ">" } else { " " };
                if self.folded_hunks.contains(&(file_index, hunk_index)) {
                    lines.push(fit_width(
                        &format!(
                            "{prefix} {} folded {} changes",
                            hunk.header,
                            hunk.stats.added + hunk.stats.removed
                        ),
                        width,
                    ));
                    continue;
                }

                lines.push(fit_width(&format!("{prefix} {}", hunk.header), width));
                for line in &hunk.lines {
                    lines.push(fit_width(&line.display_text(), width));
                }
            }
        }
        lines
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
}

impl DiffHunk {
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

fn fit_width(text: &str, width: usize) -> String {
    if width == 0 || text.chars().count() <= width {
        return text.to_owned();
    }
    text.chars().take(width).collect()
}
