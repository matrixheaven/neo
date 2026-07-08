//! Extracted: prompt completion engine — slash commands, @-mentions, file/path completion.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use skim::fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};

use crate::prompt::templates::{
    PromptTemplateLocation, discover_prompt_template_commands, load_project_prompt_templates,
};
use neo_agent_core::skills::SkillStore;
use neo_tui::shell::PickerItem;

pub(super) const MAX_FILE_REFERENCE_COMPLETIONS: usize = 100;
pub(super) const MAX_FILE_REFERENCE_INSPECTED_ENTRIES: usize = 2000;

pub(super) fn prompt_completions(
    root: &Path,
    prefix: &str,
    skill_store: Option<&SkillStore>,
    project_trusted: bool,
) -> Result<Vec<PickerItem>> {
    let (slash_prompts, prompt_packages) = if prefix.starts_with('/') {
        (
            slash_prompt_template_completion_items(root, project_trusted),
            prompt_package_completion_items(root, project_trusted)?,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let catalog = CompletionCatalog {
        slash_prompts,
        prompt_packages,
        session_commands: session_completion_items(skill_store),
    };
    Ok(completion_source_candidates(root, prefix, &catalog)?
        .into_iter()
        .map(|candidate| candidate.to_picker_item())
        .collect())
}

fn prompt_package_completion_items(root: &Path, project_trusted: bool) -> Result<Vec<PickerItem>> {
    let mut items = discover_prompt_template_commands(root, None, &[], project_trusted)?
        .into_iter()
        .filter(|command| command.location == PromptTemplateLocation::Project)
        .filter_map(|command| {
            let relative_path = command
                .template
                .path
                .strip_prefix(root.join(".neo/prompts"))
                .ok()?;
            relative_path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .and_then(|parent| parent.components().next())
                .and_then(|component| component.as_os_str().to_str())
                .filter(|provider| !provider.is_empty())?;
            let value = format!("/{}", command.template.name);
            let description =
                (!command.template.description.is_empty()).then_some(command.template.description);
            Some(PickerItem::new(value.clone(), value, description))
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.value.cmp(&right.value));
    items.dedup_by(|left, right| left.value == right.value);
    Ok(items)
}

static STATIC_SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/resume", "Resume a local session"),
    ("/new", "Start a fresh local session"),
    ("/clear", "Alias for /new"),
    ("/fork", "Fork the current session"),
    ("/help", "Show help information"),
    ("/init", "Create or refresh AGENTS.md"),
    ("/model", "Switch active model"),
    ("/provider", "View configured providers"),
    ("/mcp", "View and manage MCP servers"),
    ("/add-workspace", "Manage additional workspace directories"),
    ("/tasks", "View active background tasks"),
    ("/plan", "Toggle plan mode (on / off / clear)"),
    ("/compact", "Request manual context compaction"),
    ("/permissions", "select permission mode"),
    ("/ask", "ask permission mode"),
    ("/auto", "auto permission mode"),
    ("/yolo", "yolo permission mode"),
    ("/btw", "Open a temporary side-question panel"),
];

pub(super) fn session_completion_items(skill_store: Option<&SkillStore>) -> Vec<PickerItem> {
    let mut items: Vec<PickerItem> = STATIC_SLASH_COMMANDS
        .iter()
        .map(|(value, description)| {
            PickerItem::new((*value).to_owned(), (*value).to_owned(), Some(*description))
        })
        .collect();
    if let Some(skill_store) = skill_store {
        for skill in skill_store.iter() {
            let value = format!("/skill:{}", skill.name);
            items.push(PickerItem::new(
                value.clone(),
                value,
                Some(skill.manifest.description.clone()),
            ));
        }
    }
    items
}

fn slash_prompt_template_completion_items(root: &Path, project_trusted: bool) -> Vec<PickerItem> {
    let project_prompts_dir = root.join(".neo/prompts");
    let mut completions = load_project_prompt_templates(root, project_trusted)
        .into_iter()
        .filter(|template| {
            template
                .path
                .strip_prefix(&project_prompts_dir)
                .is_ok_and(|relative| {
                    relative
                        .parent()
                        .is_none_or(|parent| parent.as_os_str().is_empty())
                })
        })
        .map(|template| {
            let value = format!("/{}", template.name);
            let description = (!template.description.is_empty()).then_some(template.description);
            PickerItem::new(value.clone(), value, description)
        })
        .collect::<Vec<_>>();
    completions.sort_by(|left, right| left.value.cmp(&right.value));
    completions.truncate(100);
    completions
}

fn filesystem_completion_candidates(root: &Path, prefix: &str) -> Result<Vec<CompletionCandidate>> {
    let Some(request) = FilesystemCompletionRequest::from_prefix(root, prefix) else {
        return Ok(Vec::new());
    };

    let entries = match fs::read_dir(&request.search_dir) {
        Ok(entries) => entries,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read {}", request.search_dir.display()));
        }
    };

    let mut completions = Vec::new();
    for entry in entries {
        let entry =
            entry.with_context(|| format!("failed to inspect {}", request.search_dir.display()))?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !request.name_prefix.starts_with('.') && name.starts_with('.') {
            continue;
        }
        if !name.starts_with(&request.name_prefix) {
            continue;
        }

        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", entry.path().display()))?;
        let suffix = if file_type.is_dir() { "/" } else { "" };
        let value = format!(
            "{}{}{}{}",
            request.mention_prefix, request.display_dir, name, suffix
        );
        let description = if file_type.is_dir() {
            "directory"
        } else {
            "file"
        };
        completions.push(CompletionCandidate::new(
            value.clone(),
            value,
            Some(description.to_owned()),
            CompletionSource::LocalFile,
        ));
    }

    completions.sort_by(|left, right| left.value.cmp(&right.value));
    completions.truncate(100);
    Ok(completions)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CompletionCatalog {
    pub(super) slash_prompts: Vec<PickerItem>,
    pub(super) prompt_packages: Vec<PickerItem>,
    pub(super) session_commands: Vec<PickerItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompletionSource {
    LocalFile,
    FileReference,
    SlashPrompt,
    PromptPackage,
    SessionCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompletionCandidate {
    pub(super) value: String,
    pub(super) label: String,
    pub(super) description: Option<String>,
    pub(super) source: CompletionSource,
}

impl CompletionCandidate {
    fn new(
        value: impl Into<String>,
        label: impl Into<String>,
        description: Option<String>,
        source: CompletionSource,
    ) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description,
            source,
        }
    }

    fn from_picker(item: PickerItem, source: CompletionSource) -> Self {
        Self::new(item.value, item.label, item.description, source)
    }

    pub(super) fn to_picker_item(&self) -> PickerItem {
        PickerItem::new(
            self.value.clone(),
            self.label.clone(),
            self.description.clone(),
        )
    }
}

pub(super) fn completion_source_candidates(
    root: &Path,
    prefix: &str,
    catalog: &CompletionCatalog,
) -> Result<Vec<CompletionCandidate>> {
    if prefix.starts_with('/') {
        let mut candidates = slash_source_candidates(prefix, catalog);
        candidates.truncate(100);
        return Ok(candidates);
    }

    if prefix.starts_with('@') {
        let mut candidates = file_reference_completion_candidates(root, prefix)?;
        candidates.truncate(MAX_FILE_REFERENCE_COMPLETIONS);
        return Ok(candidates);
    }

    let mut candidates = filesystem_completion_candidates(root, prefix)?;
    candidates.sort_by(|left, right| {
        completion_source_rank(left.source)
            .cmp(&completion_source_rank(right.source))
            .then_with(|| left.value.cmp(&right.value))
    });
    candidates.truncate(100);
    Ok(candidates)
}

fn slash_source_candidates(prefix: &str, catalog: &CompletionCatalog) -> Vec<CompletionCandidate> {
    let query = slash_query(prefix);
    let candidates = collect_slash_candidates(catalog);
    if query.is_empty() {
        return candidates
            .into_iter()
            .map(|candidate| candidate.candidate)
            .collect();
    }

    let matcher = SkimMatcherV2::default().smart_case();
    let mut scored = candidates
        .into_iter()
        .filter_map(|candidate| score_slash_candidate(candidate, query, &matcher))
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        left.tier
            .cmp(&right.tier)
            .then_with(|| right.score.cmp(&left.score))
            .then_with(|| left.source_rank.cmp(&right.source_rank))
            .then_with(|| left.collection_index.cmp(&right.collection_index))
            .then_with(|| left.candidate.value.cmp(&right.candidate.value))
    });
    scored
        .into_iter()
        .map(|candidate| candidate.candidate)
        .collect()
}

fn slash_query(prefix: &str) -> &str {
    prefix.strip_prefix('/').unwrap_or(prefix).trim()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SlashMatchTier {
    Exact,
    Prefix,
    SegmentPrefix,
    Fuzzy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScoredSlashCandidate {
    candidate: CompletionCandidate,
    tier: SlashMatchTier,
    score: i64,
    source_rank: u8,
    collection_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SlashCandidate {
    candidate: CompletionCandidate,
    source_rank: u8,
    collection_index: usize,
}

fn collect_slash_candidates(catalog: &CompletionCatalog) -> Vec<SlashCandidate> {
    let sources = [
        (&catalog.slash_prompts, CompletionSource::SlashPrompt),
        (&catalog.prompt_packages, CompletionSource::PromptPackage),
        (&catalog.session_commands, CompletionSource::SessionCommand),
    ];
    sources
        .into_iter()
        .flat_map(|(items, source)| {
            items
                .iter()
                .cloned()
                .enumerate()
                .map(move |(collection_index, item)| SlashCandidate {
                    candidate: CompletionCandidate::from_picker(item, source),
                    source_rank: completion_source_rank(source),
                    collection_index,
                })
        })
        .collect()
}

fn score_slash_candidate(
    candidate: SlashCandidate,
    query: &str,
    matcher: &SkimMatcherV2,
) -> Option<ScoredSlashCandidate> {
    slash_search_keys(&candidate.candidate.value)
        .into_iter()
        .filter_map(|key| score_slash_key(&key, query, matcher))
        .min_by(|left, right| left.0.cmp(&right.0).then_with(|| right.1.cmp(&left.1)))
        .map(|(tier, score)| ScoredSlashCandidate {
            candidate: candidate.candidate,
            tier,
            score,
            source_rank: candidate.source_rank,
            collection_index: candidate.collection_index,
        })
}

fn score_slash_key(
    key: &str,
    query: &str,
    matcher: &SkimMatcherV2,
) -> Option<(SlashMatchTier, i64)> {
    if key == query {
        Some((SlashMatchTier::Exact, 0))
    } else if key.starts_with(query) {
        Some((SlashMatchTier::Prefix, 0))
    } else if slash_key_segments(key)
        .into_iter()
        .any(|segment| segment.starts_with(query))
    {
        Some((SlashMatchTier::SegmentPrefix, 0))
    } else {
        matcher
            .fuzzy_match(key, query)
            .map(|score| (SlashMatchTier::Fuzzy, score))
    }
}

fn slash_search_keys(value: &str) -> Vec<String> {
    let command = value.strip_prefix('/').unwrap_or(value);
    let mut keys = vec![command.to_owned()];
    if let Some(skill_name) = command.strip_prefix("skill:") {
        keys.push(skill_name.to_owned());
    }
    keys
}

fn slash_key_segments(key: &str) -> Vec<&str> {
    key.split([':', '-', '_', '.', '/'])
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn completion_source_rank(source: CompletionSource) -> u8 {
    match source {
        CompletionSource::LocalFile => 0,
        CompletionSource::FileReference => 1,
        CompletionSource::SlashPrompt => 2,
        CompletionSource::PromptPackage => 3,
        CompletionSource::SessionCommand => 4,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileReferenceCandidate {
    value: String,
    label: String,
    parent: String,
    is_dir: bool,
    score: FileReferenceScore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FileReferenceTier {
    ExactBasename,
    BasenamePrefix,
    SegmentPrefix,
    Fuzzy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileReferenceScore {
    tier: FileReferenceTier,
    skim_score: i64,
    path_len: usize,
}

fn file_reference_completion_candidates(
    root: &Path,
    prefix: &str,
) -> Result<Vec<CompletionCandidate>> {
    file_reference_completion_candidates_with_limits(
        root,
        prefix,
        MAX_FILE_REFERENCE_INSPECTED_ENTRIES,
        MAX_FILE_REFERENCE_COMPLETIONS,
    )
}

pub(super) fn file_reference_completion_candidates_with_limits(
    root: &Path,
    prefix: &str,
    max_inspected_entries: usize,
    max_completions: usize,
) -> Result<Vec<CompletionCandidate>> {
    let query = prefix.strip_prefix('@').unwrap_or(prefix).trim();
    let query_segment = query.rsplit('/').next().unwrap_or(query);
    let show_dotfiles = query_segment.starts_with('.');
    let matcher = SkimMatcherV2::default().smart_case();
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .max_depth(Some(6))
        .filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            let Some(name) = entry.file_name().to_str() else {
                return false;
            };
            !matches!(name, ".git" | ".neo") && (show_dotfiles || !name.starts_with('.'))
        });

    let mut candidates = Vec::new();
    let mut inspected = 0;
    for entry in builder.build() {
        let Ok(entry) = entry else {
            continue;
        };
        if entry.depth() == 0 {
            continue;
        }

        let path = entry.path();
        let Ok(relative_path) = path.strip_prefix(root) else {
            continue;
        };
        let Some(relative_text) = display_completion_path(relative_path) else {
            continue;
        };
        if relative_text.is_empty() {
            continue;
        }

        let Some(basename) = relative_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if inspected >= max_inspected_entries {
            break;
        }
        inspected += 1;

        let is_dir = entry
            .file_type()
            .is_some_and(|file_type| file_type.is_dir());
        let Some(score) =
            score_file_reference(query, query_segment, basename, &relative_text, &matcher)
        else {
            continue;
        };

        let label = if is_dir {
            format!("{basename}/")
        } else {
            basename.to_owned()
        };
        let parent = relative_text
            .rsplit_once('/')
            .map_or_else(String::new, |(parent, _)| format!("{parent}/"));
        candidates.push(FileReferenceCandidate {
            value: format!("@{relative_text}"),
            label,
            parent,
            is_dir,
            score,
        });
    }

    candidates.sort_by(|left, right| {
        left.score
            .tier
            .cmp(&right.score.tier)
            .then_with(|| right.score.skim_score.cmp(&left.score.skim_score))
            .then_with(|| left.is_dir.cmp(&right.is_dir))
            .then_with(|| left.score.path_len.cmp(&right.score.path_len))
            .then_with(|| left.value.cmp(&right.value))
    });
    candidates.truncate(max_completions);
    Ok(candidates
        .into_iter()
        .map(|candidate| {
            CompletionCandidate::new(
                candidate.value,
                candidate.label,
                (!candidate.parent.is_empty()).then_some(candidate.parent),
                CompletionSource::FileReference,
            )
        })
        .collect())
}

fn score_file_reference(
    query: &str,
    query_segment: &str,
    basename: &str,
    relative_text: &str,
    matcher: &SkimMatcherV2,
) -> Option<FileReferenceScore> {
    let extensionless = basename.rsplit_once('.').map_or(basename, |(stem, _)| stem);
    let tier_and_score = if query.is_empty() {
        Some((FileReferenceTier::ExactBasename, 0))
    } else if basename == query_segment
        || (!extensionless.is_empty() && extensionless == query_segment)
    {
        let score = [basename, extensionless]
            .into_iter()
            .filter_map(|key| matcher.fuzzy_match(key, query_segment))
            .max()
            .unwrap_or(0);
        Some((FileReferenceTier::ExactBasename, score))
    } else if basename.starts_with(query_segment)
        || (!extensionless.is_empty() && extensionless.starts_with(query_segment))
    {
        let score = [basename, extensionless]
            .into_iter()
            .filter_map(|key| matcher.fuzzy_match(key, query_segment))
            .max()
            .unwrap_or(0);
        Some((FileReferenceTier::BasenamePrefix, score))
    } else if relative_text
        .split('/')
        .any(|segment| segment.starts_with(query_segment))
    {
        let score = relative_text
            .split('/')
            .filter(|segment| segment.starts_with(query_segment))
            .filter_map(|segment| matcher.fuzzy_match(segment, query_segment))
            .max()
            .unwrap_or(0);
        Some((FileReferenceTier::SegmentPrefix, score))
    } else {
        file_reference_fuzzy_keys(basename, extensionless, relative_text)
            .into_iter()
            .filter_map(|key| matcher.fuzzy_match(&key, query))
            .max()
            .map(|score| (FileReferenceTier::Fuzzy, score))
    }?;

    Some(FileReferenceScore {
        tier: tier_and_score.0,
        skim_score: tier_and_score.1,
        path_len: relative_text.len(),
    })
}

fn file_reference_fuzzy_keys(
    basename: &str,
    extensionless: &str,
    relative_text: &str,
) -> Vec<String> {
    let mut keys = vec![basename.to_owned(), relative_text.to_owned()];
    if !extensionless.is_empty() && extensionless != basename {
        keys.push(extensionless.to_owned());
    }
    keys.push(relative_text.replace(['/', '-', '_', '.'], " "));
    keys
}

fn display_completion_path(path: &Path) -> Option<String> {
    path.components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
        .map(|components| components.join("/"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FilesystemCompletionRequest {
    mention_prefix: &'static str,
    display_dir: String,
    name_prefix: String,
    search_dir: PathBuf,
}

impl FilesystemCompletionRequest {
    fn from_prefix(root: &Path, prefix: &str) -> Option<Self> {
        if prefix.is_empty() {
            return None;
        }

        let (mention_prefix, path_prefix) = if let Some(path_prefix) = prefix.strip_prefix('@') {
            ("@", path_prefix)
        } else {
            ("", prefix)
        };
        let (display_dir, name_prefix) = split_completion_path(path_prefix);
        let search_dir = if Path::new(&display_dir).is_absolute() {
            PathBuf::from(&display_dir)
        } else {
            root.join(&display_dir)
        };

        Some(Self {
            mention_prefix,
            display_dir,
            name_prefix,
            search_dir,
        })
    }
}

fn split_completion_path(prefix: &str) -> (String, String) {
    if prefix.ends_with('/') {
        return (prefix.to_owned(), String::new());
    }
    match prefix.rsplit_once('/') {
        Some((directory, name)) => (format!("{directory}/"), name.to_owned()),
        None => (String::new(), prefix.to_owned()),
    }
}

pub(super) fn longest_common_completion_prefix(completions: &[PickerItem]) -> Option<String> {
    let first = completions.first()?.value.clone();
    let mut prefix = first.chars().collect::<Vec<_>>();
    for completion in completions.iter().skip(1) {
        let candidate = completion.value.chars().collect::<Vec<_>>();
        let len = prefix
            .iter()
            .zip(candidate.iter())
            .take_while(|(left, right)| left == right)
            .count();
        prefix.truncate(len);
        if prefix.is_empty() {
            break;
        }
    }
    Some(prefix.into_iter().collect())
}
