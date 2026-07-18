//! Metadata-only transcript card for one instruction epoch.
//!
//! The card is a finalized semantic entry (never a live spinner). Every
//! display string is built from epoch metadata — display-safe paths,
//! revisions, token/source/import counts, and typed reasons — and the card
//! never reads [`InstructionEpochData::model_content`]. Paths are rendered
//! workspace-relative, `$NEO_HOME`-relative, or as a stable safe placeholder.

use std::path::{Path, PathBuf};

use neo_agent_core::instructions::{
    InstructionBundleMetadata, InstructionEpochData, InstructionEpochOutcome,
    InstructionOmissionReason, InstructionReplacement, InstructionScopeData, InstructionScopeKind,
};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Color, Component, Expandable, Finalization, Line, Span, Style};

/// Renders one [`InstructionEpochData`] as a compact semantic card with an
/// optional expanded (Ctrl+O) metadata view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionCardComponent {
    epoch: InstructionEpochData,
    primary_workspace: PathBuf,
    neo_home: Option<PathBuf>,
    expanded: bool,
    id: String,
}

impl InstructionCardComponent {
    #[must_use]
    pub fn new(
        mut epoch: InstructionEpochData,
        primary_workspace: PathBuf,
        neo_home: Option<PathBuf>,
    ) -> Self {
        let id = format!("instruction-epoch-{}-{}", epoch.agent_id, epoch.generation);
        epoch.model_content = None;
        if let Some(failure) = &mut epoch.failure {
            failure.detail.clear();
        }
        let primary_workspace = primary_workspace
            .canonicalize()
            .unwrap_or(primary_workspace);
        let neo_home = neo_home.map(|path| path.canonicalize().unwrap_or(path));
        Self {
            epoch,
            primary_workspace,
            neo_home,
            expanded: false,
            id,
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn is_expanded(&self) -> bool {
        self.expanded
    }

    #[must_use]
    pub fn render_with_theme(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        let accent = Style::default().fg(outcome_color(self.epoch.outcome, theme));
        let muted = Style::default().fg(theme.text_muted);
        let detail_style = match self.epoch.outcome {
            InstructionEpochOutcome::Updated
            | InstructionEpochOutcome::PartiallyLoaded
            | InstructionEpochOutcome::Blocked => accent,
            InstructionEpochOutcome::Ready
            | InstructionEpochOutcome::Activated
            | InstructionEpochOutcome::Reactivated
            | InstructionEpochOutcome::Removed => muted,
        };

        let mut lines = vec![Line::styled(self.compact_header(), accent).truncate_to_width(width)];
        if let Some(detail) = self.compact_detail() {
            lines.push(Line::styled(format!("  {detail}"), detail_style).truncate_to_width(width));
        }
        if self.expanded {
            self.render_expanded_sections(width, theme, &mut lines);
        }
        lines
    }

    /// Clipboard text for the card in its current expansion state. Built
    /// from metadata only — instruction bodies are never copied.
    #[must_use]
    pub fn copy_text(&self) -> String {
        self.render_with_theme(200, &TuiTheme::default())
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn compact_header(&self) -> String {
        let verb = match self.epoch.outcome {
            InstructionEpochOutcome::Ready => "◆ Instructions ready",
            InstructionEpochOutcome::Activated => "◆ Instructions loaded",
            InstructionEpochOutcome::Updated => "↻ Instructions updated",
            InstructionEpochOutcome::Removed => "− Instructions removed",
            InstructionEpochOutcome::Reactivated => "◆ Instructions reactivated",
            InstructionEpochOutcome::PartiallyLoaded => "⚠ Instructions partially loaded",
            InstructionEpochOutcome::Blocked => "✕ Instructions blocked",
        };
        format!("{verb} · {}", self.primary_scope_label())
    }

    fn compact_detail(&self) -> Option<String> {
        match self.epoch.outcome {
            InstructionEpochOutcome::Ready => {
                let sources = self.total_sources();
                let imports = self.total_imports();
                let tokens = self.total_selected_tokens();
                Some(format!(
                    "{} · {} · {} tokens",
                    count_label(sources, "source"),
                    count_label(imports, "import"),
                    format_tokens(tokens),
                ))
            }
            InstructionEpochOutcome::Activated => {
                let bundle = self.primary_bundle()?;
                Some(format!(
                    "AGENTS.md · {} · {} tokens",
                    count_label(u64::from(bundle.import_count), "import"),
                    format_tokens(bundle.token_estimate),
                ))
            }
            InstructionEpochOutcome::Updated => {
                let revision = self
                    .primary_replacement()
                    .map(|replacement| replacement.new_revision.as_str())
                    .or_else(|| self.primary_bundle().map(|bundle| bundle.revision.as_str()))?;
                Some(format!("revision {revision}"))
            }
            InstructionEpochOutcome::Removed | InstructionEpochOutcome::Reactivated => None,
            InstructionEpochOutcome::PartiallyLoaded => {
                let needed = self.total_selected_tokens()
                    + self
                        .epoch
                        .ignored_bundles
                        .iter()
                        .map(|bundle| bundle.token_estimate)
                        .sum::<u64>();
                Some(format!(
                    "{} of {} tokens · {} ignored",
                    format_tokens(needed),
                    format_budget_tokens(self.epoch.budget.actual),
                    count_label(self.epoch.ignored_bundles.len() as u64, "bundle"),
                ))
            }
            InstructionEpochOutcome::Blocked => {
                let failure = self.epoch.failure.as_ref()?;
                let kind = title_case(failure.kind.describe());
                if failure.display_path.as_os_str().is_empty() {
                    Some(kind)
                } else {
                    Some(format!(
                        "{kind}: {}",
                        self.display_path(&failure.display_path)
                    ))
                }
            }
        }
    }

    fn render_expanded_sections(&self, width: usize, theme: &TuiTheme, lines: &mut Vec<Line>) {
        let muted = Style::default().fg(theme.text_muted);

        // Scope rows: one per discovered scope (global → deepest nested).
        let scope_rows: Vec<Line> = if self.epoch.scopes.is_empty() {
            vec![section_row(&self.primary_scope_label(), width, muted)]
        } else {
            self.epoch
                .scopes
                .iter()
                .map(|scope| section_row(&self.scope_label(scope), width, muted))
                .collect()
        };
        push_section(lines, width, theme, "Scope", scope_rows);

        // Loaded bundle metadata: redacted AGENTS.md path + token estimate.
        let loaded_rows = Self::aligned_rows(
            self.epoch
                .selected_bundles
                .iter()
                .map(|bundle| {
                    (
                        self.bundle_file_label(bundle),
                        format_tokens(bundle.token_estimate),
                        String::new(),
                    )
                })
                .collect(),
            width,
            theme,
        );
        push_section(lines, width, theme, "Loaded", loaded_rows);

        // Ignored bundle metadata + typed reasons.
        let ignored_rows = Self::aligned_rows(
            self.epoch
                .ignored_bundles
                .iter()
                .map(|bundle| {
                    (
                        format!("{}/AGENTS.md", self.display_path(&bundle.display_path)),
                        format_tokens(bundle.token_estimate),
                        omission_reason_label(bundle.reason).to_owned(),
                    )
                })
                .collect(),
            width,
            theme,
        );
        push_section(lines, width, theme, "Ignored", ignored_rows);

        // Imported source paths in resolver expansion order. The epoch carries
        // metadata only, never import source bodies.
        let import_rows: Vec<Line> = self
            .epoch
            .selected_bundles
            .iter()
            .flat_map(|bundle| bundle.import_paths.iter())
            .map(|path| section_row(&self.display_path(path), width, muted))
            .collect();
        push_section(lines, width, theme, "Imports", import_rows);

        // Revisions: replacements show old → new; otherwise the pinned
        // revision of each loaded bundle.
        let revision_rows = if self.epoch.replacements.is_empty() {
            Self::aligned_rows(
                self.epoch
                    .selected_bundles
                    .iter()
                    .map(|bundle| {
                        (
                            self.bundle_file_label(bundle),
                            bundle.revision.clone(),
                            String::new(),
                        )
                    })
                    .collect(),
                width,
                theme,
            )
        } else {
            Self::aligned_rows(
                self.epoch
                    .replacements
                    .iter()
                    .map(|replacement| {
                        (
                            format!("{}/AGENTS.md", self.display_path(&replacement.display_path)),
                            format!(
                                "{} → {}",
                                replacement.previous_revision, replacement.new_revision
                            ),
                            String::new(),
                        )
                    })
                    .collect(),
                width,
                theme,
            )
        };
        push_section(lines, width, theme, "Revision", revision_rows);
    }

    /// Builds aligned `path  value  extra` rows with the path column padded
    /// to the widest entry.
    fn aligned_rows(
        rows: Vec<(String, String, String)>,
        width: usize,
        theme: &TuiTheme,
    ) -> Vec<Line> {
        let primary = Style::default().fg(theme.text_primary);
        let muted = Style::default().fg(theme.text_muted);
        let path_width = rows
            .iter()
            .map(|(path, _, _)| path.chars().count())
            .max()
            .unwrap_or(0);
        rows.into_iter()
            .map(|(path, value, extra)| {
                let mut spans = vec![
                    Span::raw("  "),
                    Span::styled(format!("{path:<path_width$}"), primary),
                ];
                if !value.is_empty() {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(value, muted));
                }
                if !extra.is_empty() {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(extra, muted));
                }
                Line::from_spans(spans).truncate_to_width(width)
            })
            .collect()
    }

    /// Display label for one scope: `workspace` for the root, otherwise the
    /// redacted directory path with a `/**` suffix.
    fn scope_label(&self, scope: &InstructionScopeData) -> String {
        match scope.kind {
            InstructionScopeKind::WorkspaceRoot => "workspace".to_owned(),
            InstructionScopeKind::Global
            | InstructionScopeKind::Ancestor
            | InstructionScopeKind::Nested => {
                format!("{}/**", self.display_path(&scope.display_path))
            }
        }
    }

    /// The scope the card headlines: the deepest discovered scope, the
    /// failure path for blocked epochs without scopes, else `workspace`.
    fn primary_scope_label(&self) -> String {
        if let Some(scope) = self.epoch.scopes.last() {
            return self.scope_label(scope);
        }
        if let Some(failure) = &self.epoch.failure
            && !failure.display_path.as_os_str().is_empty()
        {
            return format!("{}/**", self.display_path(&failure.display_path));
        }
        "workspace".to_owned()
    }

    /// Bundle attached to the headline scope, else the last admitted bundle.
    fn primary_bundle(&self) -> Option<&InstructionBundleMetadata> {
        let scope_path = self.epoch.scopes.last().map(|scope| &scope.display_path);
        self.epoch
            .selected_bundles
            .iter()
            .rev()
            .find(|bundle| Some(&bundle.display_path) == scope_path)
            .or_else(|| self.epoch.selected_bundles.last())
    }

    /// Replacement attached to the headline scope, else the last one.
    fn primary_replacement(&self) -> Option<&InstructionReplacement> {
        let scope_path = self.epoch.scopes.last().map(|scope| &scope.display_path);
        self.epoch
            .replacements
            .iter()
            .rev()
            .find(|replacement| Some(&replacement.display_path) == scope_path)
            .or_else(|| self.epoch.replacements.last())
    }

    fn bundle_file_label(&self, bundle: &InstructionBundleMetadata) -> String {
        format!("{}/AGENTS.md", self.display_path(&bundle.display_path))
    }

    fn total_sources(&self) -> u64 {
        self.epoch
            .selected_bundles
            .iter()
            .map(|bundle| u64::from(bundle.source_count))
            .sum()
    }

    fn total_imports(&self) -> u64 {
        self.epoch
            .selected_bundles
            .iter()
            .map(|bundle| u64::from(bundle.import_count))
            .sum()
    }

    fn total_selected_tokens(&self) -> u64 {
        self.epoch
            .selected_bundles
            .iter()
            .map(|bundle| bundle.token_estimate)
            .sum()
    }

    /// Redacts a path for display: workspace-relative inside the primary
    /// workspace, `$NEO_HOME`-relative inside Neo's home, and a stable safe
    /// placeholder for every other absolute path.
    fn display_path(&self, path: &Path) -> String {
        if !self.primary_workspace.as_os_str().is_empty() {
            if path == self.primary_workspace {
                return ".".to_owned();
            }
            if let Ok(relative) = path.strip_prefix(&self.primary_workspace) {
                return relative.display().to_string();
            }
        }
        if let Some(neo_home) = &self.neo_home
            && !neo_home.as_os_str().is_empty()
        {
            if path == neo_home {
                return "$NEO_HOME".to_owned();
            }
            if let Ok(relative) = path.strip_prefix(neo_home) {
                return Path::new("$NEO_HOME").join(relative).display().to_string();
            }
        }
        if path.is_absolute() {
            return "<outside-workspace>".to_owned();
        }
        path.display().to_string()
    }
}

impl Expandable for InstructionCardComponent {
    fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }
}

impl Component for InstructionCardComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.render_with_theme(width, &TuiTheme::default())
    }

    fn finalization(&self) -> Finalization {
        // The epoch is a finalized semantic entry, never a live spinner.
        Finalization::Finalized
    }
}

fn outcome_color(outcome: InstructionEpochOutcome, theme: &TuiTheme) -> Color {
    match outcome {
        InstructionEpochOutcome::Ready
        | InstructionEpochOutcome::Activated
        | InstructionEpochOutcome::Reactivated => theme.brand,
        InstructionEpochOutcome::Updated | InstructionEpochOutcome::PartiallyLoaded => {
            theme.status_warn
        }
        InstructionEpochOutcome::Blocked => theme.status_error,
        InstructionEpochOutcome::Removed => theme.text_muted,
    }
}

fn push_section(
    lines: &mut Vec<Line>,
    width: usize,
    theme: &TuiTheme,
    title: &str,
    rows: Vec<Line>,
) {
    if rows.is_empty() {
        return;
    }
    if !lines.is_empty() {
        lines.push(Line::raw(""));
    }
    let header = Style::default().fg(theme.text_primary).bold();
    lines.push(Line::styled(title, header).truncate_to_width(width));
    lines.extend(rows);
}

fn section_row(text: &str, width: usize, style: Style) -> Line {
    Line::styled(format!("  {text}"), style).truncate_to_width(width)
}

fn count_label(count: u64, singular: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {singular}s")
    }
}

fn omission_reason_label(reason: InstructionOmissionReason) -> &'static str {
    match reason {
        InstructionOmissionReason::OverBudget => "budget exceeded",
    }
}

fn title_case(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Compact token counts: `800`, `8.2K`, `92K`, `1.5M`.
fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format_scaled(tokens, 1_000_000, "M")
    } else if tokens >= 1_000 {
        format_scaled(tokens, 1_000, "K")
    } else {
        tokens.to_string()
    }
}

/// Compact instruction-budget capacity using binary token units: `64K`, `1.5M`.
fn format_budget_tokens(tokens: u64) -> String {
    const KIB: u64 = 1_024;
    const MIB: u64 = KIB * KIB;
    if tokens >= MIB {
        format_scaled(tokens, MIB, "M")
    } else if tokens >= KIB {
        format_scaled(tokens, KIB, "K")
    } else {
        tokens.to_string()
    }
}

fn format_scaled(tokens: u64, unit: u64, suffix: &str) -> String {
    let whole = tokens / unit;
    let tenths = (tokens % unit) * 10 / unit;
    if tenths == 0 {
        format!("{whole}{suffix}")
    } else {
        format!("{whole}.{tenths}{suffix}")
    }
}
