//! Extracted: prompt completion engine — slash commands, @-mentions, file/path completion.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::prompt::templates::{
    PromptTemplateLocation, discover_prompt_template_commands, load_project_prompt_templates,
};
use neo_agent_core::skills::SkillStore;
use neo_tui::shell::PickerItem;

pub(super) fn prompt_completions(
    root: &Path,
    prefix: &str,
    model_items: &[PickerItem],
    skill_store: Option<&SkillStore>,
    project_trusted: bool,
) -> Result<Vec<PickerItem>> {
    let catalog = CompletionCatalog {
        slash_prompts: slash_prompt_template_completion_items(root, prefix, project_trusted)
            .unwrap_or_default(),
        prompt_packages: prompt_package_completion_items(root, project_trusted)?,
        extension_commands: extension_command_completion_items(root, project_trusted)?,
        session_commands: session_completion_items(skill_store),
        model_items: model_items.to_vec(),
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
            let provider = relative_path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .and_then(|parent| parent.components().next())
                .and_then(|component| component.as_os_str().to_str())
                .filter(|provider| !provider.is_empty())?;
            let value = format!("/{}", command.template.name);
            let description = prompt_source_description(
                (!command.template.description.is_empty())
                    .then_some(command.template.description.as_str()),
                Some(provider),
                None,
            );
            Some(PickerItem::new(value.clone(), value, Some(description)))
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.value.cmp(&right.value));
    items.dedup_by(|left, right| left.value == right.value);
    Ok(items)
}

fn extension_command_completion_items(
    root: &Path,
    project_trusted: bool,
) -> Result<Vec<PickerItem>> {
    let mut items = Vec::new();
    if project_trusted {
        let project_extension_root = root.join(".neo/extensions");
        if project_extension_root.exists() {
            items.extend(
                discover_extension_commands(&project_extension_root).with_context(|| {
                    format!(
                        "failed to discover project extensions under {}",
                        project_extension_root.display()
                    )
                })?,
            );
        }
    }
    if let Some(neo_home) = crate::config::neo_home() {
        let user_extension_root =
            neo_agent_core::tools::extensions::default_extension_root(&neo_home);
        if user_extension_root.exists() {
            items.extend(
                discover_extension_commands(&user_extension_root).with_context(|| {
                    format!(
                        "failed to discover user extensions under {}",
                        user_extension_root.display()
                    )
                })?,
            );
        }
    }
    items.sort_by(|left, right| left.value.cmp(&right.value));
    items.dedup_by(|left, right| left.value == right.value);
    items.truncate(100);
    Ok(items)
}

fn discover_extension_commands(extension_root: &Path) -> Result<Vec<PickerItem>> {
    Ok(
        neo_agent_core::tools::extensions::ExtensionDiscovery::new(extension_root)
            .discover()
            .with_context(|| {
                format!(
                    "failed to discover extensions under {}",
                    extension_root.display()
                )
            })?
            .into_iter()
            .map(|extension| {
                let value = format!("/{}", extension.manifest.id);
                let description = prompt_source_description(
                    extension.manifest.description.as_deref(),
                    Some(&extension.manifest.id),
                    Some("local extension"),
                );
                PickerItem::new(value.clone(), value, Some(description))
            })
            .collect::<Vec<_>>(),
    )
}

static STATIC_SLASH_COMMANDS: &[(&str, &str, Option<&str>, Option<&str>)] = &[
    (
        "/resume",
        "Resume a local session",
        Some("local sessions"),
        Some("local"),
    ),
    (
        "/new",
        "Start a fresh local session",
        Some("session"),
        Some("local"),
    ),
    ("/clear", "Alias for /new", Some("session"), Some("local")),
    (
        "/model",
        "Switch active model",
        Some("model picker"),
        Some("local"),
    ),
    (
        "/provider",
        "View configured providers",
        Some("provider picker"),
        Some("local"),
    ),
    (
        "/mcp",
        "View and manage MCP servers",
        Some("MCP manager"),
        Some("local"),
    ),
    (
        "/tasks",
        "View active background tasks",
        Some("background tasks"),
        Some("local"),
    ),
    (
        "/plan",
        "Toggle plan mode (on / off / clear)",
        Some("plan mode"),
        Some("local"),
    ),
    (
        "/compact",
        "Request manual context compaction",
        Some("session"),
        Some("local"),
    ),
    (
        "/permissions",
        "select permission mode",
        Some("permission mode"),
        Some("local"),
    ),
    (
        "/ask",
        "ask permission mode",
        Some("permission mode"),
        Some("local"),
    ),
    (
        "/auto",
        "auto permission mode",
        Some("permission mode"),
        Some("local"),
    ),
    (
        "/yolo",
        "yolo permission mode",
        Some("permission mode"),
        Some("local"),
    ),
    (
        "/btw",
        "Open a temporary side-question panel",
        Some("sidecar dialog"),
        Some("local"),
    ),
];

pub(super) fn session_completion_items(skill_store: Option<&SkillStore>) -> Vec<PickerItem> {
    let mut items: Vec<PickerItem> = STATIC_SLASH_COMMANDS
        .iter()
        .map(|(value, description, provider, trust)| {
            PickerItem::new(
                (*value).to_owned(),
                (*value).to_owned(),
                Some(prompt_source_description(
                    Some(description),
                    *provider,
                    *trust,
                )),
            )
        })
        .collect();
    if let Some(skill_store) = skill_store {
        for skill in skill_store.iter() {
            let value = format!("/skill:{}", skill.name);
            items.push(PickerItem::new(
                value.clone(),
                value,
                Some(prompt_source_description(
                    Some(&skill.manifest.description),
                    Some("skill"),
                    Some("local"),
                )),
            ));
        }
    }
    items
}

fn prompt_source_description(
    description: Option<&str>,
    provider: Option<&str>,
    trust: Option<&str>,
) -> String {
    let mut details = Vec::new();
    if let Some(description) = description.filter(|description| !description.is_empty()) {
        details.push(description.to_owned());
    }
    if let Some(provider) = provider {
        details.push(format!("provider: {provider}"));
    }
    if let Some(trust) = trust {
        details.push(format!("trust: {trust}"));
    }
    details.join(" | ")
}

fn slash_prompt_template_completion_items(
    root: &Path,
    prefix: &str,
    project_trusted: bool,
) -> Option<Vec<PickerItem>> {
    let name_prefix = prefix.strip_prefix('/')?;
    if name_prefix.contains('/') {
        return None;
    }

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
        .filter(|template| template.name.starts_with(name_prefix))
        .map(|template| {
            let value = format!("/{}", template.name);
            let description = (!template.description.is_empty()).then_some(template.description);
            PickerItem::new(value.clone(), value, description)
        })
        .collect::<Vec<_>>();
    completions.sort_by(|left, right| left.value.cmp(&right.value));
    completions.truncate(100);
    Some(completions)
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

fn model_completion_candidates(
    prefix: &str,
    model_items: &[PickerItem],
) -> Option<Vec<CompletionCandidate>> {
    let model_prefix = prefix.strip_prefix('@')?;
    if model_items.is_empty() {
        return None;
    }

    let mut completions = model_items
        .iter()
        .filter(|item| item.value.starts_with(model_prefix))
        .map(|item| {
            let value = format!("@{}", item.value);
            CompletionCandidate::new(
                value.clone(),
                value,
                item.description.clone(),
                CompletionSource::ProviderModel,
            )
        })
        .collect::<Vec<_>>();
    completions.sort_by(|left, right| left.value.cmp(&right.value));
    completions.truncate(100);
    Some(completions)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CompletionCatalog {
    pub(super) slash_prompts: Vec<PickerItem>,
    pub(super) prompt_packages: Vec<PickerItem>,
    pub(super) extension_commands: Vec<PickerItem>,
    pub(super) session_commands: Vec<PickerItem>,
    pub(super) model_items: Vec<PickerItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(super) enum CompletionSource {
    LocalFile,
    SlashPrompt,
    PromptPackage,
    ExtensionCommand,
    SessionCommand,
    ProviderModel,
}

impl CompletionSource {
    const fn label(self) -> &'static str {
        match self {
            Self::LocalFile => "local file",
            Self::SlashPrompt => "slash prompt",
            Self::PromptPackage => "prompt package",
            Self::ExtensionCommand => "extension command",
            Self::SessionCommand => "session command",
            Self::ProviderModel => "provider model",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompletionCandidate {
    pub(super) value: String,
    pub(super) label: String,
    pub(super) description: Option<String>,
    pub(super) source: CompletionSource,
    pub(super) source_label: &'static str,
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
            source_label: source.label(),
        }
    }

    fn from_picker(item: PickerItem, source: CompletionSource) -> Self {
        Self::new(item.value, item.label, item.description, source)
    }

    pub(super) fn to_picker_item(&self) -> PickerItem {
        PickerItem::new(
            self.value.clone(),
            self.label.clone(),
            Some(completion_description(
                self.description.as_deref(),
                self.source_label,
            )),
        )
    }
}

pub(super) fn completion_source_candidates(
    root: &Path,
    prefix: &str,
    catalog: &CompletionCatalog,
) -> Result<Vec<CompletionCandidate>> {
    let mut candidates = if prefix.starts_with('/') {
        slash_source_candidates(prefix, catalog)
    } else if prefix.starts_with('@') {
        model_completion_candidates(prefix, &catalog.model_items).unwrap_or_default()
    } else {
        filesystem_completion_candidates(root, prefix)?
    };
    candidates.sort_by(|left, right| {
        completion_source_rank(left.source)
            .cmp(&completion_source_rank(right.source))
            .then_with(|| left.value.cmp(&right.value))
    });
    candidates.truncate(100);
    Ok(candidates)
}

fn slash_source_candidates(prefix: &str, catalog: &CompletionCatalog) -> Vec<CompletionCandidate> {
    let sources = [
        (&catalog.slash_prompts, CompletionSource::SlashPrompt),
        (&catalog.prompt_packages, CompletionSource::PromptPackage),
        (
            &catalog.extension_commands,
            CompletionSource::ExtensionCommand,
        ),
        (&catalog.session_commands, CompletionSource::SessionCommand),
    ];
    sources
        .into_iter()
        .flat_map(|(items, source)| {
            items
                .iter()
                .filter(move |item| item.value.starts_with(prefix))
                .cloned()
                .map(move |item| CompletionCandidate::from_picker(item, source))
        })
        .collect()
}

fn completion_source_rank(source: CompletionSource) -> u8 {
    [0, 1, 2, 3, 4, 5][source as usize]
}

fn completion_description(description: Option<&str>, source_label: &str) -> String {
    match description {
        Some(description) if !description.is_empty() => {
            format!("{description} | source: {source_label}")
        }
        _ => format!("source: {source_label}"),
    }
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
