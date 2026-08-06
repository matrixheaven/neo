//! Host-owned `ThemeDraft` tool adapter.
//!
//! AI-assisted custom-theme creation needs a host-controlled preview/save
//! contract that an ordinary `Write` cannot provide: the model proposes theme
//! content, the host materializes a canonical, fully independent theme file
//! payload, and only a later `save` (keyed by the opaque `draft_id` returned
//! by `preview`) persists anything — always inside `$NEO_HOME/themes/`.
//!
//! Ownership boundary:
//! - `ThemeRepository` (Task 1) remains the only theme-file owner; this tool
//!   reuses `save_as_new`/`overwrite` and never touches theme files directly.
//! - Drafts live in a bounded in-memory store (`Arc<Mutex<ThemeDraftStore>>`,
//!   most recent 8) that expires with the runtime. Preview is the only way to
//!   create a draft; save accepts only `draft_id` + `overwrite`.
//! - Save never calls `NeoChromeState::set_theme`, never changes the
//!   transcript theme, never appends a user message, and never rewrites
//!   context or session metadata. Every successful save reports
//!   `applied: false`.
//!
//! Permission contract (enforced by the runtime permission layer, not here):
//! preview is a non-mutating tool action; save is the dedicated
//! [`PermissionOperation::ThemeSave`] with a one-time Ask approval and no
//! session-wide grant. The tool re-checks its typed action before acting.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use neo_agent_core::tools::{Tool, ToolContext, ToolError, ToolFuture, ToolResult};
use neo_tui::primitive::Color;
use neo_tui::shell::TuiTheme;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::themes::{
    ThemeId, ThemeOverrides, ThemeRepository, color_from_string, color_to_string,
    materialize_theme_with_overrides,
};

/// Maximum number of canonical drafts kept in the in-memory store. The most
/// recent drafts win; the oldest is evicted deterministically.
const DRAFT_STORE_CAPACITY: usize = 8;

/// Bounded display-name length in characters.
const MAX_DISPLAY_NAME_CHARS: usize = 64;

/// The current semantic-token allowlist, in canonical order. Mirrors the
/// strict `ThemeColors` schema owned by the repository.
const CANONICAL_TOKENS: &[&str] = &[
    "text_primary",
    "prompt",
    "brand",
    "status_ok",
    "status_error",
    "status_warn",
    "text_muted",
    "user_message",
    "diff_added",
    "diff_removed",
    "diff_hunk",
    "diff_context",
    "selection_bg",
    "status_pending",
    "status_cancelled",
    "approval_border",
    "selected_fg",
    "selected_bg",
    "overlay_border",
    "footer_permission_allow",
    "footer_permission_ask",
    "footer_permission_deny",
    "footer_working",
    "footer_context_ok",
    "footer_context_warn",
    "footer_context_critical",
    "shell_mode",
];

/// Foreground tokens checked for contrast against the background tokens.
const FOREGROUND_TOKENS: &[&str] = &[
    "text_primary",
    "text_muted",
    "prompt",
    "user_message",
    "brand",
    "status_ok",
    "status_error",
    "status_warn",
    "status_pending",
    "status_cancelled",
    "shell_mode",
    "footer_permission_allow",
    "footer_permission_ask",
    "footer_permission_deny",
    "footer_working",
    "footer_context_ok",
    "footer_context_warn",
    "footer_context_critical",
    "selected_fg",
];

/// Background tokens used as the contrast reference surface.
const BACKGROUND_TOKENS: &[&str] = &["selection_bg", "selected_bg"];

/// Minimum WCAG contrast ratio (AA for large text) before a warning is emitted.
const MIN_CONTRAST_RATIO: f64 = 3.0;

/// Maximum number of contrast warnings returned per preview.
const MAX_CONTRAST_WARNINGS: usize = 6;

// ---------------------------------------------------------------------------
// Wire types (typed, strictly validated)
// ---------------------------------------------------------------------------

/// The typed `ThemeDraft` input. The `action` tag selects preview vs save;
/// unknown fields on either branch are rejected.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ThemeDraftInput {
    /// Materialize a canonical draft from a base theme plus semantic-token
    /// overrides. No persistent side effect.
    Preview(ThemeDraftPreviewInput),
    /// Persist a previously previewed draft inside `$NEO_HOME/themes/`.
    Save(ThemeDraftSaveInput),
}

/// Preview branch input.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThemeDraftPreviewInput {
    /// Display name for the draft and the persisted theme. Bounded to 64
    /// characters, no control characters, no separators, no platform-reserved
    /// names. The save destination id is derived from this name.
    #[schemars(
        description = "Display name for the new theme. 1-64 characters; no control characters, '/' or '\\\\' separators, or platform-reserved names. The save destination theme id is derived from this name, so changing it requires a new preview."
    )]
    pub name: String,
    /// Optional logical id of an existing base theme under `$NEO_HOME/themes/`
    /// (e.g. `default.json`). Omit to start from the built-in default theme.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Optional base theme id (relative path under $NEO_HOME/themes/, e.g. \"default.json\"). Omit to start from the built-in default theme."
    )]
    pub base_theme: Option<String>,
    /// Semantic color-token overrides keyed by the canonical token allowlist.
    /// Values are hex (`#rrggbb`) or named colors from the existing parser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Semantic color overrides. Allowed tokens: text_primary, prompt, brand, status_ok, status_error, status_warn, text_muted, user_message, diff_added, diff_removed, diff_hunk, diff_context, selection_bg, status_pending, status_cancelled, approval_border, selected_fg, selected_bg, overlay_border, footer_permission_allow, footer_permission_ask, footer_permission_deny, footer_working, footer_context_ok, footer_context_warn, footer_context_critical, shell_mode. Values are #rrggbb hex or named colors (e.g. \"darkgray\")."
    )]
    pub colors: Option<BTreeMap<String, String>>,
}

/// Save branch input.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThemeDraftSaveInput {
    /// Opaque draft id returned by a prior `preview`.
    #[schemars(
        description = "The opaque draft_id returned by a prior ThemeDraft preview. Save accepts no new colors, names, or paths; create a new preview to change any of them."
    )]
    pub draft_id: String,
    /// Set true to replace the existing theme at the draft's destination id.
    /// Defaults to false; an existing destination then fails with a conflict.
    #[serde(default)]
    #[schemars(
        description = "Set true to overwrite the existing theme at the draft's destination id. Defaults to false."
    )]
    pub overwrite: bool,
}

// ---------------------------------------------------------------------------
// Bounded draft store
// ---------------------------------------------------------------------------

/// One canonical, fully materialized draft produced by a preview.
#[derive(Debug, Clone)]
pub struct StoredThemeDraft {
    /// Opaque id returned to the model; never parsed or constructed by callers.
    pub id: String,
    /// Validated display name (also the persisted theme's `name` field).
    pub display_name: String,
    /// Save destination logical id, derived from the display name.
    pub candidate_theme_id: ThemeId,
    /// Canonical string form of the base theme id, or `None` for the built-in
    /// default. Never a file path.
    pub base_theme_id: Option<String>,
    /// `sha256:` fingerprint of the canonical payload.
    pub fingerprint: String,
    /// Canonical JSON payload: a complete, independent theme file.
    pub payload: String,
    /// Materialized theme used by the preview card renderer and the save.
    pub theme: TuiTheme,
    /// Tokens the model overrode, in input order, for the color samples.
    pub overridden_tokens: Vec<String>,
    /// Deterministic contrast warnings for the materialized theme.
    pub contrast_warnings: Vec<String>,
}

/// Bounded in-memory store of canonical drafts. Owns no files; expires with
/// the runtime that created it. Eviction is deterministic (oldest first).
#[derive(Debug, Default)]
pub struct ThemeDraftStore {
    drafts: VecDeque<StoredThemeDraft>,
}

impl ThemeDraftStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a draft and return its opaque id. Evicts the oldest draft once
    /// the capacity is exceeded.
    pub fn insert(&mut self, draft: StoredThemeDraft) -> String {
        let id = Self::new_id();
        let mut draft = draft;
        draft.id.clone_from(&id);
        self.drafts.push_back(draft);
        while self.drafts.len() > DRAFT_STORE_CAPACITY {
            self.drafts.pop_front();
        }
        id
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&StoredThemeDraft> {
        self.drafts.iter().find(|draft| draft.id == id)
    }

    #[must_use]
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.drafts.len()
    }

    /// Opaque and unguessable draft id: v4 entropy, so callers can never
    /// construct a valid id and can only receive one from `preview`.
    fn new_id() -> String {
        format!("draft-{}", Uuid::new_v4().simple())
    }
}

// ---------------------------------------------------------------------------
// Tool
// ---------------------------------------------------------------------------

/// The host-owned `ThemeDraft` tool. Registered only in the root interactive
/// runtime registry; child/delegate registries never acquire it.
pub struct ThemeDraftTool {
    repository: ThemeRepository,
    store: Arc<Mutex<ThemeDraftStore>>,
}

impl ThemeDraftTool {
    /// Tool for the single Neo home with a caller-shared draft store (the
    /// runtime shares the same `Arc` so the store lives as long as the runtime).
    #[must_use]
    pub fn new(repository: ThemeRepository, store: Arc<Mutex<ThemeDraftStore>>) -> Self {
        Self { repository, store }
    }

    /// Tool for the default Neo home with a fresh bounded store.
    #[must_use]
    pub fn default_with_store() -> Self {
        Self::new(
            ThemeRepository::default(),
            Arc::new(Mutex::new(ThemeDraftStore::new())),
        )
    }

    #[must_use]
    #[cfg(test)]
    pub fn store(&self) -> &Arc<Mutex<ThemeDraftStore>> {
        &self.store
    }

    fn preview(&self, input: &ThemeDraftPreviewInput) -> ToolResult {
        match self.try_preview(input) {
            Ok(result) => result,
            Err((category, message)) => theme_draft_error(category, message),
        }
    }

    fn try_preview(
        &self,
        input: &ThemeDraftPreviewInput,
    ) -> Result<ToolResult, (&'static str, String)> {
        let name = validate_display_name(&input.name)?;
        let name_id = ThemeId::new(name)
            .map_err(|error| ("invalid_input", format!("invalid display name: {error}")))?;
        let candidate_id = candidate_theme_id(name)?;

        let (base_theme_id, base_theme) = match &input.base_theme {
            Some(raw) => {
                let id = ThemeId::new(raw).map_err(|error| {
                    ("invalid_input", format!("invalid base theme id: {error}"))
                })?;
                let entry = self
                    .repository
                    .resolve(&id)
                    .map_err(|error| ("missing_base", format!("base theme not found: {error}")))?;
                (Some(id.as_str().to_owned()), entry.theme)
            }
            None => (None, TuiTheme::default()),
        };

        let overrides = parse_overrides(input.colors.as_ref())?;
        let theme = apply_overrides(&base_theme, &overrides).map_err(|error| {
            (
                "invalid_input",
                format!("failed to apply overrides: {error}"),
            )
        })?;
        let payload = materialize_theme_with_overrides(&name_id, &base_theme, &overrides).map_err(
            |error| {
                (
                    "invalid_input",
                    format!("failed to materialize theme: {error}"),
                )
            },
        )?;
        let fingerprint = fingerprint_of(&payload);
        let normalized_colors = payload_colors(&payload)?;
        let contrast_warnings = contrast_warnings_for(&theme);
        let overridden_tokens: Vec<String> = input
            .colors
            .as_ref()
            .map(|colors| colors.keys().cloned().collect())
            .unwrap_or_default();

        let draft = StoredThemeDraft {
            id: String::new(), // assigned by the store
            display_name: name.to_owned(),
            candidate_theme_id: candidate_id.clone(),
            base_theme_id: base_theme_id.clone(),
            fingerprint: fingerprint.clone(),
            payload,
            theme,
            overridden_tokens: overridden_tokens.clone(),
            contrast_warnings: contrast_warnings.clone(),
        };
        let draft_id = self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(draft);

        let content = format!(
            "Preview ready: \"{name}\" (draft {draft_id}, candidate id {}) with {} color override(s). \
             Fingerprint {fingerprint}. Call ThemeDraft with action \"save\" and this draft_id to \
             persist it; applied: false.",
            candidate_id.as_str(),
            overridden_tokens.len(),
        );
        Ok(ToolResult::ok(content).with_details(json!({
            "kind": "theme_draft_preview",
            "draft_id": draft_id,
            "fingerprint": fingerprint,
            "display_name": name,
            "candidate_theme_id": candidate_id.as_str(),
            "base_theme_id": base_theme_id,
            "normalized_colors": normalized_colors,
            "overridden_tokens": overridden_tokens,
            "contrast_warnings": contrast_warnings,
            "applied": false,
        })))
    }

    fn save(&self, input: &ThemeDraftSaveInput) -> ToolResult {
        if !self.repository.root().is_absolute() {
            return theme_draft_error(
                "permission",
                "the theme home is not configured; cannot save a theme".to_owned(),
            );
        }
        let draft = self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&input.draft_id)
            .cloned();
        let Some(draft) = draft else {
            return theme_draft_error(
                "expired_draft",
                format!(
                    "draft {} not found or expired; create a new preview",
                    input.draft_id
                ),
            );
        };
        if let Err(message) = revalidate_draft(&draft) {
            return theme_draft_error("invalid_input", message);
        }

        let catalog = match self.repository.catalog() {
            Ok(catalog) => catalog,
            Err(error) => {
                return theme_draft_error(
                    "atomic_write",
                    format!("failed to read the theme catalog: {error:#}"),
                );
            }
        };
        let exists = catalog.by_id(&draft.candidate_theme_id).is_some();
        let written = if exists {
            if !input.overwrite {
                return theme_draft_error(
                    "conflict",
                    format!(
                        "theme {:?} already exists; pass overwrite: true to replace it",
                        draft.candidate_theme_id.as_str()
                    ),
                );
            }
            self.repository
                .overwrite(&draft.candidate_theme_id, &draft.display_name, &draft.theme)
        } else {
            self.repository.save_as_new(
                &draft.candidate_theme_id,
                &draft.display_name,
                &draft.theme,
            )
        };
        let entry = match written {
            Ok(entry) => entry,
            Err(error) => {
                let message = format!("{error:#}");
                let category = if message.contains("already exists") {
                    "conflict"
                } else {
                    "atomic_write"
                };
                return theme_draft_error(category, message);
            }
        };

        // The persisted bytes must match the previewed canonical payload
        // exactly; otherwise the save surface drifted from the preview.
        let persisted_fingerprint = std::fs::read(&entry.path)
            .map(|bytes| fingerprint_of_bytes(&bytes))
            .ok();
        if persisted_fingerprint.as_deref() != Some(draft.fingerprint.as_str()) {
            return theme_draft_error(
                "atomic_write",
                "saved theme content does not match the previewed fingerprint".to_owned(),
            );
        }

        ToolResult::ok(format!(
            "Theme saved as {:?} (fingerprint {}, applied: false). Use the /theme command to apply it.",
            draft.candidate_theme_id.as_str(),
            draft.fingerprint,
        ))
        .with_details(json!({
            "kind": "theme_draft_saved",
            "theme_id": draft.candidate_theme_id.as_str(),
            "base_theme_id": draft.base_theme_id,
            "fingerprint": draft.fingerprint,
            "overridden_tokens": draft.overridden_tokens,
            "contrast_warnings": draft.contrast_warnings,
            "applied": false,
        }))
    }
}

impl Tool for ThemeDraftTool {
    fn name(&self) -> &'static str {
        "ThemeDraft"
    }

    fn description(&self) -> &'static str {
        "Create and save custom themes under $NEO_HOME/themes/ through a host-controlled preview/save flow.\n\
         Use this tool only for the explicit custom-theme creation workflow, never as a general file writer.\n\n\
         Actions:\n\
         - preview: materialize a canonical draft from an optional base theme id plus semantic color-token overrides. No files are written and no theme is applied. Returns an opaque draft_id, the derived candidate theme id, a stable fingerprint, normalized colors, and contrast warnings. In plan mode preview is allowed.\n\
         - save: persist a previously previewed draft by its draft_id. Accepts only draft_id and overwrite; new colors, names, or paths require a new preview. Writes strictly inside $NEO_HOME/themes/. In ask permission mode save requires a one-time approval. Saving never applies the theme — the /theme command applies it afterwards."
    }

    fn input_schema(&self) -> serde_json::Value {
        neo_ai::tool_schema::schema_for::<ThemeDraftInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            if !ctx.access.tool {
                return Err(ToolError::PermissionDenied { operation: "tool" });
            }
            let input: ThemeDraftInput = match serde_json::from_value(input) {
                Ok(input) => input,
                Err(error) => {
                    return Ok(theme_draft_error(
                        "invalid_input",
                        format!("invalid ThemeDraft input: {error}"),
                    ));
                }
            };
            match input {
                ThemeDraftInput::Preview(preview) => Ok(self.preview(&preview)),
                ThemeDraftInput::Save(save) => Ok(self.save(&save)),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Validation and materialization helpers
// ---------------------------------------------------------------------------

/// Validate the display name: non-empty, bounded, no control characters, no
/// separators, and representable as a theme id (rejects platform-reserved
/// names and leading-dot/trailing-dot/space component shapes).
fn validate_display_name(name: &str) -> Result<&str, (&'static str, String)> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(("invalid_input", "display name must not be empty".to_owned()));
    }
    if trimmed.chars().count() > MAX_DISPLAY_NAME_CHARS {
        return Err((
            "invalid_input",
            format!("display name must be at most {MAX_DISPLAY_NAME_CHARS} characters"),
        ));
    }
    if trimmed
        .chars()
        .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err((
            "invalid_input",
            "display name must not contain control characters or separators".to_owned(),
        ));
    }
    if ThemeId::new(trimmed).is_err() {
        return Err((
            "invalid_input",
            "display name cannot be used as a theme name (reserved or invalid shape)".to_owned(),
        ));
    }
    Ok(trimmed)
}

/// Derive the save destination logical id from the display name: lowercase,
/// non-alphanumeric characters become single dashes, `.json` is appended, and
/// the result must be a valid `ThemeId`.
fn candidate_theme_id(name: &str) -> Result<ThemeId, (&'static str, String)> {
    let mut slug = String::new();
    let mut pending_dash = false;
    for character in name.to_lowercase().chars() {
        if character.is_alphanumeric() {
            slug.push(character);
            pending_dash = false;
        } else if !slug.is_empty() && !pending_dash {
            slug.push('-');
            pending_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    let candidate = format!("{slug}.json");
    ThemeId::new(&candidate).map_err(|error| {
        (
            "invalid_input",
            format!("display name cannot be represented as a theme id: {error}"),
        )
    })
}

/// Parse the model-supplied color map against the canonical token allowlist.
/// Values are parsed by the existing named/hex parser and normalized to their
/// canonical string form so the payload and fingerprint are deterministic.
fn parse_overrides(
    colors: Option<&BTreeMap<String, String>>,
) -> Result<ThemeOverrides, (&'static str, String)> {
    let mut overrides = ThemeOverrides::default();
    let Some(colors) = colors else {
        return Ok(overrides);
    };
    for (token, value) in colors {
        let parsed = color_from_string(value).map_err(|error| {
            (
                "invalid_input",
                format!("invalid color {value:?} for token {token:?}: {error}"),
            )
        })?;
        let normalized = color_to_string(parsed).map_err(|error| {
            (
                "invalid_input",
                format!("color {value:?} for token {token:?} cannot be persisted: {error}"),
            )
        })?;
        set_override(&mut overrides, token, normalized)?;
    }
    Ok(overrides)
}

fn set_override(
    overrides: &mut ThemeOverrides,
    token: &str,
    value: String,
) -> Result<(), (&'static str, String)> {
    if !CANONICAL_TOKENS.contains(&token) {
        return Err((
            "invalid_input",
            format!("unknown semantic color token {token:?}"),
        ));
    }
    let field = match token {
        "text_primary" => &mut overrides.text_primary,
        "prompt" => &mut overrides.prompt,
        "brand" => &mut overrides.brand,
        "status_ok" => &mut overrides.status_ok,
        "status_error" => &mut overrides.status_error,
        "status_warn" => &mut overrides.status_warn,
        "text_muted" => &mut overrides.text_muted,
        "user_message" => &mut overrides.user_message,
        "diff_added" => &mut overrides.diff_added,
        "diff_removed" => &mut overrides.diff_removed,
        "diff_hunk" => &mut overrides.diff_hunk,
        "diff_context" => &mut overrides.diff_context,
        "selection_bg" => &mut overrides.selection_bg,
        "status_pending" => &mut overrides.status_pending,
        "status_cancelled" => &mut overrides.status_cancelled,
        "approval_border" => &mut overrides.approval_border,
        "selected_fg" => &mut overrides.selected_fg,
        "selected_bg" => &mut overrides.selected_bg,
        "overlay_border" => &mut overrides.overlay_border,
        "footer_permission_allow" => &mut overrides.footer_permission_allow,
        "footer_permission_ask" => &mut overrides.footer_permission_ask,
        "footer_permission_deny" => &mut overrides.footer_permission_deny,
        "footer_working" => &mut overrides.footer_working,
        "footer_context_ok" => &mut overrides.footer_context_ok,
        "footer_context_warn" => &mut overrides.footer_context_warn,
        "footer_context_critical" => &mut overrides.footer_context_critical,
        "shell_mode" => &mut overrides.shell_mode,
        _ => {
            return Err((
                "invalid_input",
                format!("unknown semantic color token {token:?}"),
            ));
        }
    };
    *field = Some(value);
    Ok(())
}

/// Apply parsed overrides onto a copy of the base theme. All 27 persisted
/// tokens are present in the result because the base supplies every token the
/// overrides do not.
fn apply_overrides(base: &TuiTheme, overrides: &ThemeOverrides) -> anyhow::Result<TuiTheme> {
    let mut theme = *base;
    macro_rules! apply {
        ($field:ident) => {
            if let Some(value) = &overrides.$field {
                theme.$field = color_from_string(value)?;
            }
        };
    }
    apply!(text_primary);
    apply!(prompt);
    apply!(brand);
    apply!(status_ok);
    apply!(status_error);
    apply!(status_warn);
    apply!(text_muted);
    apply!(user_message);
    apply!(diff_added);
    apply!(diff_removed);
    apply!(diff_hunk);
    apply!(diff_context);
    apply!(selection_bg);
    apply!(status_pending);
    apply!(status_cancelled);
    apply!(approval_border);
    apply!(selected_fg);
    apply!(selected_bg);
    apply!(overlay_border);
    apply!(footer_permission_allow);
    apply!(footer_permission_ask);
    apply!(footer_permission_deny);
    apply!(footer_working);
    apply!(footer_context_ok);
    apply!(footer_context_warn);
    apply!(footer_context_critical);
    apply!(shell_mode);
    Ok(theme)
}

/// Extract the canonical `colors` object from a materialized payload.
fn payload_colors(payload: &str) -> Result<BTreeMap<String, String>, (&'static str, String)> {
    let value: serde_json::Value = serde_json::from_str(payload).map_err(|error| {
        (
            "invalid_input",
            format!("payload is not valid JSON: {error}"),
        )
    })?;
    let colors = value
        .get("colors")
        .and_then(serde_json::Value::as_object)
        .ok_or(("invalid_input", "payload has no colors object".to_owned()))?;
    let mut normalized = BTreeMap::new();
    for (token, color) in colors {
        let color = color
            .as_str()
            .ok_or(("invalid_input", "payload color is not a string".to_owned()))?;
        normalized.insert(token.clone(), color.to_owned());
    }
    Ok(normalized)
}

/// Stable `sha256:` fingerprint of canonical payload bytes.
fn fingerprint_of(payload: &str) -> String {
    fingerprint_of_bytes(payload.as_bytes())
}

fn fingerprint_of_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{}", hex_digest(&digest))
}

fn hex_digest(digest: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}

/// Revalidate a stored draft before saving: the payload must still match its
/// fingerprint and must parse as a complete canonical theme.
fn revalidate_draft(draft: &StoredThemeDraft) -> Result<(), String> {
    if fingerprint_of(&draft.payload) != draft.fingerprint {
        return Err("stored draft payload no longer matches its fingerprint".to_owned());
    }
    let value: serde_json::Value = serde_json::from_str(&draft.payload)
        .map_err(|error| format!("stored draft payload is not valid JSON: {error}"))?;
    let colors = value
        .get("colors")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "stored draft payload has no colors object".to_owned())?;
    if colors.len() != CANONICAL_TOKENS.len() {
        return Err(format!(
            "stored draft payload has {} colors; expected {}",
            colors.len(),
            CANONICAL_TOKENS.len()
        ));
    }
    for (token, color) in colors {
        if !CANONICAL_TOKENS.contains(&token.as_str()) {
            return Err(format!("stored draft contains unknown token {token:?}"));
        }
        let color = color
            .as_str()
            .ok_or_else(|| format!("stored draft color {token:?} is not a string"))?;
        color_from_string(color)
            .map_err(|error| format!("stored draft color {token:?} is invalid: {error}"))?;
    }
    Ok(())
}

/// Deterministic contrast warnings: for every foreground/background pair of
/// RGB colors, warn when the WCAG ratio drops below [`MIN_CONTRAST_RATIO`].
/// Results are capped so the preview card stays bounded.
fn contrast_warnings_for(theme: &TuiTheme) -> Vec<String> {
    let foreground = token_colors(FOREGROUND_TOKENS, theme);
    let background = token_colors(BACKGROUND_TOKENS, theme);
    let mut warnings = Vec::new();
    for (fg_token, fg) in &foreground {
        for (bg_token, bg) in &background {
            let Some(ratio) = contrast_ratio(*fg, *bg) else {
                continue;
            };
            if ratio < MIN_CONTRAST_RATIO {
                warnings.push(format!(
                    "{fg_token} vs {bg_token}: contrast {ratio:.1} is below {MIN_CONTRAST_RATIO}"
                ));
                if warnings.len() >= MAX_CONTRAST_WARNINGS {
                    return warnings;
                }
            }
        }
    }
    warnings
}

fn token_colors(tokens: &'static [&'static str], theme: &TuiTheme) -> Vec<(&'static str, Color)> {
    tokens
        .iter()
        .filter_map(|token| token_color(token, theme).map(|color| (*token, color)))
        .collect()
}

fn token_color(token: &str, theme: &TuiTheme) -> Option<Color> {
    let color = match token {
        "text_primary" => theme.text_primary,
        "text_muted" => theme.text_muted,
        "prompt" => theme.prompt,
        "user_message" => theme.user_message,
        "brand" => theme.brand,
        "status_ok" => theme.status_ok,
        "status_error" => theme.status_error,
        "status_warn" => theme.status_warn,
        "status_pending" => theme.status_pending,
        "status_cancelled" => theme.status_cancelled,
        "shell_mode" => theme.shell_mode,
        "footer_permission_allow" => theme.footer_permission_allow,
        "footer_permission_ask" => theme.footer_permission_ask,
        "footer_permission_deny" => theme.footer_permission_deny,
        "footer_working" => theme.footer_working,
        "footer_context_ok" => theme.footer_context_ok,
        "footer_context_warn" => theme.footer_context_warn,
        "footer_context_critical" => theme.footer_context_critical,
        "selected_fg" => theme.selected_fg,
        "selection_bg" => theme.selection_bg,
        "selected_bg" => theme.selected_bg,
        _ => return None,
    };
    matches!(color, Color::Rgb(..)).then_some(color)
}

/// WCAG contrast ratio between two RGB colors. `None` when either color is not
/// an RGB value (ANSI palette colors have no defined luminance).
fn contrast_ratio(a: Color, b: Color) -> Option<f64> {
    let (ar, ag, ab) = rgb(a)?;
    let (br, bg, bb) = rgb(b)?;
    let a_luminance = relative_luminance(ar, ag, ab);
    let b_luminance = relative_luminance(br, bg, bb);
    let (higher, lower) = if a_luminance > b_luminance {
        (a_luminance, b_luminance)
    } else {
        (b_luminance, a_luminance)
    };
    Some((higher + 0.05) / (lower + 0.05))
}

fn rgb(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(red, green, blue) => Some((red, green, blue)),
        _ => None,
    }
}

fn relative_luminance(red: u8, green: u8, blue: u8) -> f64 {
    0.2126 * linearize(red) + 0.7152 * linearize(green) + 0.0722 * linearize(blue)
}

fn linearize(channel: u8) -> f64 {
    let scaled = f64::from(channel) / 255.0;
    if scaled <= 0.040_45 {
        scaled / 12.92
    } else {
        ((scaled + 0.055) / 1.055).powf(2.4)
    }
}

/// Typed error result with a stable category for the skill: `invalid_input`,
/// `missing_base`, `conflict`, `expired_draft`, `permission`, `plan_blocked`,
/// or `atomic_write`.
fn theme_draft_error(category: &'static str, message: String) -> ToolResult {
    let text = format!("{category}: {message}");
    let mut details = serde_json::Map::new();
    details.insert("kind".to_owned(), serde_json::json!("theme_draft_error"));
    details.insert("error".to_owned(), serde_json::json!(category));
    details.insert("message".to_owned(), serde_json::Value::String(message));
    ToolResult::error(text).with_details(serde_json::Value::Object(details))
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo_agent_core::ToolAccess;
    use neo_agent_core::tools::ToolContext;
    use serde_json::json;
    use tempfile::TempDir;

    fn repository_in(temp: &TempDir) -> ThemeRepository {
        ThemeRepository::from_home(Some(temp.path().to_path_buf()))
    }

    fn tool_in(temp: &TempDir) -> ThemeDraftTool {
        ThemeDraftTool::new(
            repository_in(temp),
            Arc::new(Mutex::new(ThemeDraftStore::new())),
        )
    }

    fn context(workspace: &TempDir) -> ToolContext {
        ToolContext::new(workspace.path())
            .expect("context")
            .with_access(ToolAccess {
                file_read: false,
                file_write: false,
                shell: false,
                tool: true,
                user_question: false,
            })
    }

    fn preview_input(name: &str) -> serde_json::Value {
        json!({
            "action": "preview",
            "name": name,
            "colors": {"brand": "#58a6ff", "text_primary": "#E6EDF3"},
        })
    }

    async fn run_preview(
        tool: &ThemeDraftTool,
        ctx: &ToolContext,
        input: serde_json::Value,
    ) -> ToolResult {
        tool.execute(ctx, input).await.expect("preview should run")
    }

    fn result_details(result: &ToolResult) -> serde_json::Value {
        result.details.clone().expect("result should carry details")
    }

    #[tokio::test]
    async fn preview_materializes_complete_independent_theme() {
        let temp = TempDir::new().expect("tempdir");
        let tool = tool_in(&temp);
        let ctx = context(&temp);

        let result = run_preview(&tool, &ctx, preview_input("Aurora Night")).await;
        assert!(!result.is_error, "preview failed: {}", result.content);

        let details = result_details(&result);
        assert_eq!(details["kind"], "theme_draft_preview");
        assert_eq!(details["display_name"], "Aurora Night");
        assert_eq!(details["candidate_theme_id"], "aurora-night.json");
        assert_eq!(details["applied"], false);
        assert!(details["draft_id"].as_str().unwrap().starts_with("draft-"));

        let colors = details["normalized_colors"].as_object().unwrap();
        assert_eq!(colors.len(), CANONICAL_TOKENS.len());
        assert_eq!(colors["brand"], "#58a6ff");
        // Uppercase input hex is normalized to lowercase canonical form.
        assert_eq!(colors["text_primary"], "#e6edf3");
        // Non-overridden tokens come from the built-in default.
        assert_eq!(colors["status_ok"], "#4ec87e");
        assert!(colors.contains_key("shell_mode"));
    }

    #[tokio::test]
    async fn preview_is_non_mutating_and_store_is_shared_with_runtime() {
        let temp = TempDir::new().expect("tempdir");
        let store = Arc::new(Mutex::new(ThemeDraftStore::new()));
        let tool = ThemeDraftTool::new(repository_in(&temp), Arc::clone(&store));
        let ctx = context(&temp);

        let result = run_preview(&tool, &ctx, preview_input("Aurora Night")).await;
        assert!(!result.is_error);
        let draft_id = result_details(&result)["draft_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(store.lock().unwrap().get(&draft_id).is_some());
        // No theme files were written by a preview.
        assert_eq!(repository_in(&temp).catalog().unwrap().entries.len(), 0);
        assert_eq!(store.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn preview_then_save_across_tool_instances_shares_the_session_store() {
        // The interactive session owns one bounded store threaded through every
        // turn's runtime. A preview issued by instance A (turn N) must be
        // savable by instance B (turn N+1) built from the same Arc; a fresh
        // store (a different session) must reject the draft as expired.
        let temp = TempDir::new().expect("tempdir");
        let store = Arc::new(Mutex::new(ThemeDraftStore::new()));
        let repo = repository_in(&temp);
        let turn_a = ThemeDraftTool::new(repo.clone(), Arc::clone(&store));
        let turn_b = ThemeDraftTool::new(repo.clone(), Arc::clone(&store));
        let ctx = context(&temp);

        let preview = turn_a
            .execute(&ctx, preview_input("Aurora Night"))
            .await
            .expect("preview runs");
        assert!(!preview.is_error, "{}", preview.content);
        let draft_id = result_details(&preview)["draft_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let saved = turn_b
            .execute(&ctx, json!({"action": "save", "draft_id": draft_id}))
            .await
            .expect("save runs");
        assert!(
            !saved.is_error,
            "save across turns must succeed: {}",
            saved.content
        );
        assert_eq!(result_details(&saved)["applied"], false);

        // A different Arc (fresh session) cannot see the draft.
        let other_session = ThemeDraftTool::new(repo, Arc::new(Mutex::new(ThemeDraftStore::new())));
        let expired = other_session
            .execute(&ctx, json!({"action": "save", "draft_id": draft_id}))
            .await
            .expect("save runs");
        assert!(expired.is_error);
        assert_eq!(result_details(&expired)["error"], "expired_draft");
    }

    #[tokio::test]
    async fn preview_rejects_unknown_tokens_and_invalid_colors() {
        let temp = TempDir::new().expect("tempdir");
        let tool = tool_in(&temp);
        let ctx = context(&temp);

        let unknown = run_preview(
            &tool,
            &ctx,
            json!({
                "action": "preview",
                "name": "Bad",
                "colors": {"accent": "#ff0000"},
            }),
        )
        .await;
        assert!(unknown.is_error);
        assert_eq!(result_details(&unknown)["error"], "invalid_input");

        let bad_color = run_preview(
            &tool,
            &ctx,
            json!({
                "action": "preview",
                "name": "Bad",
                "colors": {"brand": "not-a-color"},
            }),
        )
        .await;
        assert!(bad_color.is_error);
        assert_eq!(result_details(&bad_color)["error"], "invalid_input");
    }

    #[tokio::test]
    async fn preview_rejects_unknown_json_fields() {
        let temp = TempDir::new().expect("tempdir");
        let tool = tool_in(&temp);
        let ctx = context(&temp);

        let result = tool
            .execute(
                &ctx,
                json!({
                    "action": "preview",
                    "name": "Aurora",
                    "bogus_field": true,
                }),
            )
            .await
            .expect("tool runs");
        assert!(result.is_error);
        assert_eq!(result_details(&result)["error"], "invalid_input");
    }

    #[tokio::test]
    async fn preview_validates_display_name_bounds() {
        let temp = TempDir::new().expect("tempdir");
        let tool = tool_in(&temp);
        let ctx = context(&temp);

        for (name, needle) in [
            ("", "empty"),
            ("Aurora/ Night", "separator"),
            ("bad\u{1}name", "control"),
            (&"x".repeat(MAX_DISPLAY_NAME_CHARS + 1), "at most"),
            ("CON", "cannot be used"),
        ] {
            let result = run_preview(&tool, &ctx, preview_input(name)).await;
            assert!(result.is_error, "accepted name {name:?}");
            assert_eq!(
                result_details(&result)["error"],
                "invalid_input",
                "name {name:?}"
            );
            assert!(
                result.content.contains(needle),
                "name {name:?} message {:?} missing {needle:?}",
                result.content
            );
        }
    }

    #[tokio::test]
    async fn preview_base_theme_resolution_and_missing_base() {
        let temp = TempDir::new().expect("tempdir");
        let repo = repository_in(&temp);
        let tool = ThemeDraftTool::new(repo.clone(), Arc::new(Mutex::new(ThemeDraftStore::new())));
        let ctx = context(&temp);

        let base_id = crate::themes::ThemeId::new("base.json").unwrap();
        let path = base_id.path_under(repo.root());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r##"{"name": "Base", "colors": {"brand": "#123456"}}"##,
        )
        .unwrap();

        let result = run_preview(
            &tool,
            &ctx,
            json!({
                "action": "preview",
                "name": "Derived",
                "base_theme": "base.json",
                "colors": {"brand": "#ff0000"},
            }),
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        let details = result_details(&result);
        assert_eq!(details["base_theme_id"], "base.json");
        assert_eq!(details["normalized_colors"]["brand"], "#ff0000");

        let missing = run_preview(
            &tool,
            &ctx,
            json!({
                "action": "preview",
                "name": "Derived",
                "base_theme": "nope.json",
            }),
        )
        .await;
        assert!(missing.is_error);
        assert_eq!(result_details(&missing)["error"], "missing_base");
    }

    #[tokio::test]
    async fn preview_fingerprint_is_stable_and_content_driven() {
        let temp = TempDir::new().expect("tempdir");
        let tool = tool_in(&temp);
        let ctx = context(&temp);

        let first = run_preview(&tool, &ctx, preview_input("Aurora Night")).await;
        let second = run_preview(&tool, &ctx, preview_input("Aurora Night")).await;
        let third = run_preview(&tool, &ctx, preview_input("Different Name")).await;

        let first_fp = result_details(&first)["fingerprint"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(first_fp, result_details(&second)["fingerprint"]);
        assert_eq!(&first_fp[..7], "sha256:");
        assert_ne!(first_fp, result_details(&third)["fingerprint"]);
    }

    #[tokio::test]
    async fn preview_store_is_bounded_and_evicts_oldest_first() {
        let temp = TempDir::new().expect("tempdir");
        let tool = tool_in(&temp);
        let ctx = context(&temp);

        let mut draft_ids = Vec::new();
        for index in 0..(DRAFT_STORE_CAPACITY + 3) {
            let result = run_preview(&tool, &ctx, preview_input(&format!("Theme {index}"))).await;
            assert!(!result.is_error, "{}", result.content);
            draft_ids.push(
                result_details(&result)["draft_id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
        }

        let store = tool.store();
        let store = store.lock().unwrap();
        assert_eq!(store.len(), DRAFT_STORE_CAPACITY);
        // The three oldest drafts were evicted deterministically.
        for id in &draft_ids[..3] {
            assert!(store.get(id).is_none(), "oldest draft {id} must be evicted");
        }
        for id in &draft_ids[3..] {
            assert!(store.get(id).is_some(), "recent draft {id} must be kept");
        }
    }

    #[tokio::test]
    async fn save_persists_previewed_draft_inside_theme_home() {
        let temp = TempDir::new().expect("tempdir");
        let repo = repository_in(&temp);
        let tool = ThemeDraftTool::new(repo.clone(), Arc::new(Mutex::new(ThemeDraftStore::new())));
        let ctx = context(&temp);

        let preview = run_preview(&tool, &ctx, preview_input("Aurora Night")).await;
        assert!(!preview.is_error);
        let draft_id = result_details(&preview)["draft_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let expected_fingerprint = result_details(&preview)["fingerprint"]
            .as_str()
            .unwrap()
            .to_owned();

        let saved = tool
            .execute(&ctx, json!({"action": "save", "draft_id": draft_id}))
            .await
            .expect("save runs");
        assert!(!saved.is_error, "save failed: {}", saved.content);

        let details = result_details(&saved);
        assert_eq!(details["kind"], "theme_draft_saved");
        assert_eq!(details["theme_id"], "aurora-night.json");
        assert_eq!(details["fingerprint"], expected_fingerprint);
        assert_eq!(details["applied"], false);

        let entry = repo
            .resolve(&crate::themes::ThemeId::new("aurora-night.json").unwrap())
            .unwrap();
        assert!(entry.is_valid());
        assert_eq!(entry.name, "Aurora Night");
        let on_disk = std::fs::read_to_string(&entry.path).unwrap();
        assert_eq!(super::fingerprint_of(&on_disk), expected_fingerprint);
    }

    #[tokio::test]
    async fn save_rejects_unknown_draft_and_extra_fields() {
        let temp = TempDir::new().expect("tempdir");
        let tool = tool_in(&temp);
        let ctx = context(&temp);

        let expired = tool
            .execute(&ctx, json!({"action": "save", "draft_id": "draft-missing"}))
            .await
            .expect("save runs");
        assert!(expired.is_error);
        assert_eq!(result_details(&expired)["error"], "expired_draft");

        let extra = tool
            .execute(
                &ctx,
                json!({"action": "save", "draft_id": "x", "colors": {"brand": "#fff"}}),
            )
            .await
            .expect("save runs");
        assert!(extra.is_error);
        assert_eq!(result_details(&extra)["error"], "invalid_input");
    }

    #[tokio::test]
    async fn save_conflict_requires_explicit_overwrite() {
        let temp = TempDir::new().expect("tempdir");
        let repo = repository_in(&temp);
        let tool = ThemeDraftTool::new(repo.clone(), Arc::new(Mutex::new(ThemeDraftStore::new())));
        let ctx = context(&temp);

        let preview = run_preview(&tool, &ctx, preview_input("Aurora Night")).await;
        let draft_id = result_details(&preview)["draft_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let conflict = tool
            .execute(&ctx, json!({"action": "save", "draft_id": draft_id}))
            .await
            .expect("first save");
        assert!(!conflict.is_error, "{}", conflict.content);

        let conflict = tool
            .execute(&ctx, json!({"action": "save", "draft_id": draft_id}))
            .await
            .expect("conflicting save");
        assert!(conflict.is_error);
        assert_eq!(result_details(&conflict)["error"], "conflict");

        let overwritten = tool
            .execute(
                &ctx,
                json!({"action": "save", "draft_id": draft_id, "overwrite": true}),
            )
            .await
            .expect("overwrite save");
        assert!(!overwritten.is_error, "{}", overwritten.content);
        assert_eq!(result_details(&overwritten)["applied"], false);
    }

    #[tokio::test]
    async fn save_is_denied_without_tool_access() {
        let temp = TempDir::new().expect("tempdir");
        let tool = tool_in(&temp);
        let workspace = TempDir::new().expect("tempdir");
        let denied_ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_access(ToolAccess::none());

        let error = tool
            .execute(&denied_ctx, preview_input("Aurora Night"))
            .await
            .expect_err("tool access required");
        assert!(matches!(
            error,
            ToolError::PermissionDenied { operation: "tool" }
        ));
    }

    #[test]
    fn candidate_id_slugs_display_names_deterministically() {
        assert_eq!(
            candidate_theme_id("Aurora Night").unwrap().as_str(),
            "aurora-night.json"
        );
        assert_eq!(
            candidate_theme_id("  Aurora   Night  ").unwrap().as_str(),
            "aurora-night.json"
        );
        assert_eq!(
            candidate_theme_id("B.R.A.N.D.").unwrap().as_str(),
            "b-r-a-n-d.json"
        );
        assert_eq!(candidate_theme_id("主题").unwrap().as_str(), "主题.json");
        assert!(candidate_theme_id("").is_err());
    }

    #[test]
    fn contrast_warnings_flag_low_contrast_pairs() {
        let theme = TuiTheme {
            text_primary: Color::Rgb(20, 20, 20), // near-black on default surface
            selection_bg: Color::Rgb(31, 35, 43),
            ..Default::default()
        };
        let warnings = contrast_warnings_for(&theme);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("text_primary vs selection_bg")),
            "expected a low-contrast warning: {warnings:?}"
        );
    }
}
