use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use regex::Regex;

#[derive(Debug, Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<XtaskCommand>,
}

#[derive(Debug, Subcommand)]
enum XtaskCommand {
    Check(CheckOptions),
    /// Run the docs/examples parity gate without fmt, clippy, or tests.
    Parity,
}

#[derive(Debug, Clone, Default, clap::Args)]
struct CheckOptions {
    /// Validate local links in docs and examples Markdown files.
    #[arg(long)]
    docs: bool,
    /// Run full workspace checks instead of the stable xtask gate.
    #[arg(long)]
    workspace: bool,
    /// Run only the xtask package checks.
    #[arg(long)]
    quick: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandStep {
    program: String,
    args: Vec<String>,
}

impl CommandStep {
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_owned(),
            args: args.iter().map(ToString::to_string).collect(),
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli
        .command
        .unwrap_or_else(|| XtaskCommand::Check(CheckOptions::default()))
    {
        XtaskCommand::Check(options) => check(&options),
        XtaskCommand::Parity => run_parity_gate(Path::new(".")),
    }
}

fn check(options: &CheckOptions) -> Result<()> {
    for step in check_steps(options) {
        run(&step)?;
    }
    if options.docs {
        run_parity_gate(Path::new("."))?;
    }
    Ok(())
}

fn run_parity_gate(root: &Path) -> Result<()> {
    let errors = validate_parity_gate(root)?;
    if !errors.is_empty() {
        bail!(
            "production/docs/examples parity validation failed:\n{}",
            errors.join("\n")
        );
    }
    run_rust_examples_compile_gate(root)?;
    println!("production/docs/examples parity validation passed");
    Ok(())
}

fn validate_parity_gate(root: &Path) -> Result<Vec<String>> {
    let mut errors = validate_docs_links(root)?;
    errors.extend(validate_docs_parity(root)?);
    errors.extend(validate_examples(root)?);
    errors.sort();
    Ok(errors)
}

fn check_steps(options: &CheckOptions) -> Vec<CommandStep> {
    if !options.workspace || options.quick {
        vec![
            CommandStep::new("cargo", &["fmt", "-p", "xtask", "--check"]),
            CommandStep::new(
                "cargo",
                &[
                    "clippy",
                    "-p",
                    "xtask",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
            ),
            CommandStep::new("cargo", &["test", "-p", "xtask"]),
        ]
    } else {
        vec![
            CommandStep::new("cargo", &["fmt", "--all", "--check"]),
            CommandStep::new(
                "cargo",
                &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--",
                    "-D",
                    "warnings",
                ],
            ),
            CommandStep::new("cargo", &["test", "--workspace", "--all-features"]),
        ]
    }
}

fn run(step: &CommandStep) -> Result<()> {
    println!("running: {} {}", step.program, step.args.join(" "));
    let status = Command::new(&step.program).args(&step.args).status()?;
    if !status.success() {
        bail!("{} {} failed", step.program, step.args.join(" "));
    }
    Ok(())
}

fn validate_docs_links(root: &Path) -> Result<Vec<String>> {
    let link_pattern = Regex::new(r"\[[^\]]+\]\(([^)]+)\)")?;
    let markdown_files = markdown_files(root)?;
    let mut errors = Vec::new();

    for file in markdown_files {
        let source = fs::read_to_string(&file)?;
        let relative_file = relative_path(root, &file);
        let Some(parent) = file.parent() else {
            continue;
        };

        for captures in link_pattern.captures_iter(&source) {
            let Some(raw_target) = captures.get(1).map(|target| target.as_str().trim()) else {
                continue;
            };
            if let Some(target) = local_link_target(raw_target) {
                let target_path = parent.join(target);
                if !target_path.exists() {
                    errors.push(format!(
                        "{} links to missing local file {}",
                        relative_file.display(),
                        relative_path(root, &target_path).display()
                    ));
                }
            }
        }
    }

    errors.sort();
    Ok(errors)
}

fn validate_docs_parity(root: &Path) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let code_truth = ParityCodeTruth::load(root)?;
    for file in parity_scan_files(root)? {
        let source = fs::read_to_string(&file)?;
        let relative_file = relative_path(root, &file);
        let explicit_fixture_path = is_explicit_fixture_path(&relative_file);
        let mut allow_next_line = false;

        for (index, line) in source.lines().enumerate() {
            let line_number = index + 1;
            if explicit_fixture_path && parity_allowlist_reason(line).is_some() {
                allow_next_line = true;
                continue;
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                allow_next_line = false;
                continue;
            }

            if allow_next_line {
                allow_next_line = false;
                continue;
            }

            if explicit_fixture_path && parity_line_is_fixture_safe(trimmed) {
                continue;
            }

            if let Some(reason) = parity_line_violation(trimmed, explicit_fixture_path, &code_truth)
            {
                errors.push(format!(
                    "{}:{line_number} contains {reason}: {trimmed}",
                    relative_file.display()
                ));
            }
        }
    }
    errors.sort();
    Ok(errors)
}

fn parity_scan_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = production_scan_files(root)?;
    out.extend(markdown_files(root)?);
    for dir in ["examples/config", "examples/tools"] {
        let path = root.join(dir);
        if path.is_dir() {
            collect_files_with_extensions(&path, &["toml", "json"], &mut out)?;
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn production_scan_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let crates = root.join("crates");
    if !crates.is_dir() {
        return Ok(out);
    }

    for entry in fs::read_dir(crates)? {
        let crate_dir = entry?.path();
        if crate_dir.is_dir() {
            let src = crate_dir.join("src");
            if src.is_dir() {
                collect_files_with_extensions(&src, &["rs", "toml", "json"], &mut out)?;
            }
        }
    }

    out.sort();
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ImplementedSurface {
    McpToolAdapterBoundary,
    StdioMcpProcessAdapter,
    HttpMcpJsonSubscribeEventReader,
    ExtensionLifecycleCommands,
    SessionMetadataBranching,
    InteractiveSessionPicker,
    InteractiveModelPicker,
    InteractiveSessionFork,
    RuntimeHooksAndQueues,
    TuiUnifiedDiffRenderer,
    TuiPasteBuffering,
    TuiTranscriptSelectionCopy,
    TerminalImageProtocol,
    AiAnthropicGoogleThinkingPayloads,
}

#[derive(Debug, Clone)]
struct ParityCodeTruth {
    implemented: BTreeSet<ImplementedSurface>,
}

#[derive(Debug, Clone)]
struct ParitySources {
    mcp: String,
    cli: String,
    session: String,
    runtime: String,
    interactive: String,
    input: String,
    tui_app: String,
    tui_components: String,
    tui_image: String,
    anthropic: String,
    google: String,
}

impl ParitySources {
    fn load(root: &Path) -> Result<Self> {
        Ok(Self {
            mcp: read_agent_core_source(root, &["tools", "mcp.rs"])?,
            cli: read_neo_agent_source(root, &["cli.rs"])?,
            session: read_agent_core_source(root, &["session", "mod.rs"])?,
            runtime: read_agent_core_source(root, &["runtime.rs"])?,
            interactive: read_neo_agent_source(root, &["modes", "interactive.rs"])?,
            input: read_tui_source(root, &["input.rs"])?,
            tui_app: read_tui_source(root, &["app.rs"])?,
            tui_components: read_tui_source(root, &["components.rs"])?,
            tui_image: read_tui_source(root, &["image.rs"])?,
            anthropic: read_ai_provider_source(root, "anthropic.rs")?,
            google: read_ai_provider_source(root, "google.rs")?,
        })
    }
}

impl ParityCodeTruth {
    fn load(root: &Path) -> Result<Self> {
        let sources = ParitySources::load(root)?;
        let mut implemented = BTreeSet::new();

        if sources.mcp.contains("trait McpToolAdapter") && sources.mcp.contains("McpToolProvider") {
            implemented.insert(ImplementedSurface::McpToolAdapterBoundary);
        }
        if sources.mcp.contains("McpStdioToolAdapter")
            && sources.mcp.contains("tools/list")
            && sources.mcp.contains("tools/call")
        {
            implemented.insert(ImplementedSurface::StdioMcpProcessAdapter);
        }
        if sources.mcp.contains("start_resource_event_reader") {
            implemented.insert(ImplementedSurface::HttpMcpJsonSubscribeEventReader);
        }
        if sources.cli.contains("Status")
            && sources.cli.contains("Enable")
            && sources.cli.contains("Disable")
            && sources.cli.contains("ExtensionCommand")
        {
            implemented.insert(ImplementedSurface::ExtensionLifecycleCommands);
        }
        if sources.session.contains("SessionMetadataStore")
            && sources.session.contains("pub fn fork")
            && sources.session.contains("pub fn rename")
        {
            implemented.insert(ImplementedSurface::SessionMetadataBranching);
        }
        if sources.interactive.contains("open_session_picker")
            && sources.interactive.contains("load_selected_session")
            && sources.interactive.contains("session_catalog_for_config")
            && sources.input.contains("SessionPickerOpen")
        {
            implemented.insert(ImplementedSurface::InteractiveSessionPicker);
        }
        if sources.interactive.contains("open_model_picker")
            && sources.interactive.contains("apply_selected_model")
            && sources.interactive.contains("model_catalog_for_config")
            && sources.input.contains("ModelPickerOpen")
        {
            implemented.insert(ImplementedSurface::InteractiveModelPicker);
        }
        if sources.interactive.contains("fork_selected_session")
            && sources.interactive.contains("fork_session_transcript")
            && sources.input.contains("SessionFork")
            && sources.input.contains("tui.session.fork")
        {
            implemented.insert(ImplementedSurface::InteractiveSessionFork);
        }
        if sources.runtime.contains("with_before_tool_call")
            && sources.runtime.contains("with_after_tool_call")
            && sources.runtime.contains("with_queue_modes")
            && sources.runtime.contains("queue_steering_message")
        {
            implemented.insert(ImplementedSurface::RuntimeHooksAndQueues);
        }
        if sources.tui_app.contains("DiffAdded")
            && sources.tui_app.contains("DiffRemoved")
            && sources.tui_components.contains("transcript_line_style")
        {
            implemented.insert(ImplementedSurface::TuiUnifiedDiffRenderer);
        }
        if sources.input.contains("InputParser")
            && sources.input.contains("BRACKETED_PASTE_START")
            && sources.input.contains("BRACKETED_PASTE_END")
        {
            implemented.insert(ImplementedSurface::TuiPasteBuffering);
        }
        if sources.tui_app.contains("TranscriptSelection")
            && sources.tui_app.contains("copy_selection")
            && sources.tui_app.contains("copy_selected_transcript_text")
            && sources.tui_components.contains("with_selection")
            && sources.input.contains("TranscriptSelectionStart")
            && sources.input.contains("TranscriptCopySelection")
            && sources.input.contains("tui.transcript.copySelection")
            && sources
                .interactive
                .contains("copy_transcript_selection_to_clipboard")
        {
            implemented.insert(ImplementedSurface::TuiTranscriptSelectionCopy);
        }
        if terminal_image_protocol_symbols_exist(&[
            &sources.input,
            &sources.tui_app,
            &sources.tui_components,
            &sources.tui_image,
        ]) {
            implemented.insert(ImplementedSurface::TerminalImageProtocol);
        }
        if sources.anthropic.contains("thinking_budget_tokens")
            && sources.anthropic.contains("\"budget_tokens\"")
            && sources.google.contains("thinking_budget_tokens")
            && sources.google.contains("\"thinkingConfig\"")
        {
            implemented.insert(ImplementedSurface::AiAnthropicGoogleThinkingPayloads);
        }

        Ok(Self { implemented })
    }

    fn has(&self, surface: ImplementedSurface) -> bool {
        self.implemented.contains(&surface)
    }
}

fn terminal_image_protocol_symbols_exist(sources: &[&str]) -> bool {
    sources.iter().any(|source| {
        source.contains("ImageProtocolError")
            || source.contains("KittyGraphicsOptions")
            || source.contains("Iterm2InlineImageOptions")
            || source.contains("encode_kitty_graphics")
            || source.contains("encode_iterm2_inline_image")
            || source.contains("TerminalImageProtocol")
            || source.contains("InlineImageProtocol")
            || source.contains("KittyGraphicsProtocol")
            || source.contains("Sixel")
            || source.contains("Iterm2InlineImage")
            || source.contains("render_inline_image_protocol")
            || source.contains("terminal_image_protocol")
            || source.contains("kitty_graphics_protocol")
            || source.contains("sixel_protocol")
    })
}

fn read_agent_core_source(root: &Path, parts: &[&str]) -> Result<String> {
    read_crate_source(root, "agent-core", parts)
}

fn read_neo_agent_source(root: &Path, parts: &[&str]) -> Result<String> {
    read_crate_source(root, "neo-agent", parts)
}

fn read_tui_source(root: &Path, parts: &[&str]) -> Result<String> {
    read_crate_source(root, "tui", parts)
}

fn read_ai_provider_source(root: &Path, file: &str) -> Result<String> {
    read_crate_source(root, "ai", &["providers", file])
}

fn read_crate_source(root: &Path, crate_name: &str, parts: &[&str]) -> Result<String> {
    let path = parts.iter().fold(
        root.join("crates").join(crate_name).join("src"),
        |path, part| path.join(part),
    );
    read_optional_source(&path)
}

fn read_optional_source(path: &Path) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }
    Ok(fs::read_to_string(path)?)
}

fn collect_files_with_extensions(
    dir: &Path,
    extensions: &[&str],
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files_with_extensions(&path, extensions, out)?;
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extensions.contains(&extension))
        {
            out.push(path);
        }
    }
    Ok(())
}

fn parity_allowlist_reason(line: &str) -> Option<&str> {
    let (_prefix, reason) = line.split_once("xtask-parity: allow ")?;
    Some(reason.trim())
}

fn is_explicit_fixture_path(path: &Path) -> bool {
    let normalized = normalize_path(path);
    let text = normalized.to_string_lossy();
    text.contains("/tests/")
        || text.starts_with("examples/")
        || text.ends_with("/src/harness.rs")
        || text.ends_with("/src/providers/fake.rs")
}

fn is_explicit_fixture_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    let normalized = lower.replace(['-', '_'], " ");
    normalized.contains("for tests")
        || normalized.contains("test fake provider")
        || normalized.contains("fake test provider")
        || normalized.contains("fake provider")
        || normalized.contains("fake harness")
        || normalized.contains("fake model")
        || normalized.contains("fake_model")
        || normalized.contains("fakemodelclient")
        || normalized.contains("fakeharness")
        || normalized.contains("pub mod fake")
        || normalized.contains("deterministic fixture")
        || normalized.contains("not production")
        || normalized.contains("no production")
        || normalized.contains("not resolve")
}

fn is_gap_or_checker_description(line: &str) -> bool {
    let lower = line.to_lowercase();
    let normalized = lower.replace(['-', '_'], " ");
    normalized.contains("no longer placeholder level")
        || (normalized.contains("scans for production docs")
            && normalized.contains("fake or placeholder"))
        || normalized.contains("production fake/local/placeholder guidance")
        || normalized.contains("fake/local/placeholder production guidance")
        || normalized.contains("that point at fake or placeholder provider paths")
        || normalized.contains("compile time rust stub should only be added")
        || normalized.contains("rejects stale text")
        || normalized.contains("once symbols exist")
        || normalized.contains("once `mcptooladapter`")
        || normalized.contains("once status/enable/disable commands")
        || normalized.contains("session branching and naming are future work\" once")
        || normalized.contains("once `sessionmetadatastore::fork`")
        || normalized.contains("once lifecycle commands exist")
        || normalized.contains("once fork exists")
        || normalized.contains("parity gate")
}

fn parity_line_is_fixture_safe(line: &str) -> bool {
    let lower = line.to_lowercase();
    let normalized = lower.replace(['-', '_'], " ");
    if (normalized.contains("production") || normalized.contains("prod "))
        && !is_explicit_fixture_line(line)
    {
        return false;
    }

    contains_any_word(
        &normalized,
        &[
            "fake",
            "placeholder",
            "stub",
            "dummy",
            "mock",
            "todo",
            "tbd",
        ],
    ) || normalized.contains("127.0.0.1")
        || normalized.contains("localhost")
}

fn parity_line_violation(
    line: &str,
    explicit_fixture_path: bool,
    code_truth: &ParityCodeTruth,
) -> Option<&'static str> {
    let lower = line.to_lowercase();
    let normalized = lower.replace(['-', '_'], " ");
    if is_gap_or_checker_description(line) {
        return None;
    }
    if normalized.contains("parity")
        && (normalized.contains("scan")
            || normalized.contains("gate")
            || normalized.contains("guidance")
            || normalized.contains("allowlist"))
    {
        return None;
    }

    let production_context = contains_any_word(
        &normalized,
        &["production", "prod", "default", "defaults", "deploy"],
    );
    let fake_context = contains_any_word(
        &normalized,
        &[
            "fake",
            "placeholder",
            "stub",
            "dummy",
            "mock",
            "fixture",
            "deterministic",
        ],
    ) || normalized.contains("localhost")
        || normalized.contains("127.0.0.1");
    let local_provider_context = normalized.contains("local provider")
        || normalized.contains("local providers")
        || normalized.contains("api kind::local")
        || normalized.contains("apikind::local");

    if is_explicit_fixture_line(line) {
        return None;
    }

    if let Some(reason) = stale_gap_claim_violation(&normalized, code_truth) {
        return Some(reason);
    }

    if production_context && fake_context {
        return Some("production fake/default guidance");
    }

    if production_context && local_provider_context && !is_rejection_or_gap_statement(&normalized) {
        return Some("production local-provider guidance");
    }

    if explicit_fixture_path && parity_line_is_fixture_safe(line) {
        return None;
    }

    if contains_any_word(&normalized, &["todo", "tbd"]) {
        return Some("unresolved placeholder marker");
    }

    if contains_any_word(
        &normalized,
        &["fake", "placeholder", "stub", "dummy", "mock"],
    ) {
        return Some("production fake/placeholder marker");
    }

    if normalized.contains("local deterministic") || normalized.contains("deterministic guidance") {
        return Some("local deterministic guidance");
    }

    None
}

fn stale_gap_claim_violation(
    normalized: &str,
    code_truth: &ParityCodeTruth,
) -> Option<&'static str> {
    stale_backend_gap_claim_violation(normalized, code_truth)
        .or_else(|| stale_interactive_gap_claim_violation(normalized, code_truth))
        .or_else(|| stale_tui_gap_claim_violation(normalized, code_truth))
        .or_else(|| stale_ai_gap_claim_violation(normalized, code_truth))
}

fn stale_backend_gap_claim_violation(
    normalized: &str,
    code_truth: &ParityCodeTruth,
) -> Option<&'static str> {
    if code_truth.has(ImplementedSurface::McpToolAdapterBoundary)
        && normalized.contains("mcp")
        && normalized.contains("adapter")
        && (normalized.contains("not wired")
            || normalized.contains("no mcp")
            || normalized.contains("future work"))
    {
        return Some("stale MCP adapter gap claim");
    }

    if code_truth.has(ImplementedSurface::StdioMcpProcessAdapter)
        && normalized.contains("mcp")
        && normalized.contains("process")
        && (normalized.contains("does not yet spawn")
            || normalized.contains("not yet spawn")
            || normalized.contains("not implemented")
            || normalized.contains("future work"))
    {
        return Some("stale MCP process adapter gap claim");
    }

    if code_truth.has(ImplementedSurface::HttpMcpJsonSubscribeEventReader)
        && normalized.contains("mcp")
        && normalized.contains("json")
        && normalized.contains("subscribe")
        && (normalized.contains("ack") || normalized.contains("resource update"))
        && (normalized.contains("cannot receive")
            || normalized.contains("can't receive")
            || normalized.contains("does not receive")
            || normalized.contains("not receive")
            || normalized.contains("no update")
            || normalized.contains("missing")
            || normalized.contains("not implemented")
            || normalized.contains("future work"))
    {
        return Some("stale HTTP MCP JSON subscribe event gap claim");
    }

    if code_truth.has(ImplementedSurface::ExtensionLifecycleCommands)
        && normalized.contains("extension lifecycle")
        && (normalized.contains("do not document")
            || normalized.contains("not available")
            || normalized.contains("unavailable")
            || normalized.contains("not implemented")
            || normalized.contains("future work"))
    {
        return Some("stale extension lifecycle gap claim");
    }

    if code_truth.has(ImplementedSurface::SessionMetadataBranching)
        && (normalized.contains("branching") || normalized.contains("fork"))
        && normalized.contains("naming")
        && (normalized.contains("future work")
            || normalized.contains("remain")
            || normalized.contains("missing")
            || normalized.contains("not implemented"))
    {
        return Some("stale session branching gap claim");
    }

    if code_truth.has(ImplementedSurface::RuntimeHooksAndQueues)
        && normalized.contains("hook")
        && normalized.contains("steering")
        && normalized.contains("docs")
        && (normalized.contains("only when")
            || normalized.contains("not exposed")
            || normalized.contains("future work")
            || normalized.contains("missing"))
    {
        return Some("stale runtime hook/queue gap claim");
    }

    None
}

fn stale_interactive_gap_claim_violation(
    normalized: &str,
    code_truth: &ParityCodeTruth,
) -> Option<&'static str> {
    if code_truth.has(ImplementedSurface::InteractiveSessionPicker)
        && normalized.contains("session picker")
        && (normalized.contains("missing")
            || normalized.contains("not implemented")
            || normalized.contains("future work"))
    {
        return Some("stale live session picker gap claim");
    }

    if code_truth.has(ImplementedSurface::InteractiveModelPicker)
        && normalized.contains("model picker")
        && (normalized.contains("missing")
            || normalized.contains("not implemented")
            || normalized.contains("future work"))
    {
        return Some("stale live model picker gap claim");
    }

    if code_truth.has(ImplementedSurface::InteractiveSessionFork)
        && (normalized.contains("fork-before-continue")
            || normalized.contains("fork before continue"))
        && (normalized.contains("missing")
            || normalized.contains("not implemented")
            || normalized.contains("future work")
            || normalized.contains("gap")
            || normalized.contains("only when")
            || normalized.contains("beyond local jsonl append"))
    {
        return Some("stale interactive session fork gap claim");
    }

    None
}

fn stale_tui_gap_claim_violation(
    normalized: &str,
    code_truth: &ParityCodeTruth,
) -> Option<&'static str> {
    if code_truth.has(ImplementedSurface::TuiUnifiedDiffRenderer)
        && stale_tui_diff_renderer_claim(normalized)
    {
        return Some("stale TUI unified diff renderer gap claim");
    }

    if code_truth.has(ImplementedSurface::TuiPasteBuffering)
        && (normalized.contains("stdin buffering") || normalized.contains("paste buffering"))
        && (normalized.contains("not implement")
            || normalized.contains("not implemented")
            || normalized.contains("missing")
            || normalized.contains("future work")
            || normalized.contains("until")
            || normalized.contains("land"))
    {
        return Some("stale TUI paste buffering gap claim");
    }

    if code_truth.has(ImplementedSurface::TuiTranscriptSelectionCopy)
        && stale_tui_transcript_selection_copy_claim(normalized)
    {
        return Some("stale TUI transcript selection copy gap claim");
    }

    if code_truth.has(ImplementedSurface::TerminalImageProtocol)
        && stale_terminal_image_protocol_claim(normalized)
    {
        return Some("stale terminal image protocol gap claim");
    }

    None
}

fn stale_tui_diff_renderer_claim(normalized: &str) -> bool {
    (normalized.contains("diff renderer")
        || normalized.contains("diff rendering")
        || normalized.contains("unified diff"))
        && (normalized.contains("not implement")
            || normalized.contains("not implemented")
            || normalized.contains("missing")
            || normalized.contains("future work")
            || normalized.contains("until diff rendering"))
}

fn stale_tui_transcript_selection_copy_claim(normalized: &str) -> bool {
    let Some(copy_index) = normalized.find("copy") else {
        return false;
    };
    let context = {
        let prefix = normalized[..copy_index]
            .chars()
            .rev()
            .take(80)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>();
        let suffix = normalized[copy_index..]
            .chars()
            .take(120)
            .collect::<String>();
        format!("{prefix}{suffix}")
    };

    (context.contains("selected transcript")
        || normalized.contains("transcript region")
        || normalized.contains("transcript-region")
        || context.contains("transcript selection"))
        && (context.contains("not implement")
            || context.contains("not implemented")
            || context.contains("missing")
            || context.contains("future work")
            || context.contains("remain")
            || context.contains("gap"))
}

fn stale_terminal_image_protocol_claim(normalized: &str) -> bool {
    (normalized.contains("terminal image protocol")
        || normalized.contains("terminal image protocols")
        || (normalized.contains("image protocol")
            && (normalized.contains("no ")
                || normalized.contains("without ")
                || normalized.contains("not implement")
                || normalized.contains("not implemented")))
        || (normalized.contains("image protocols")
            && (normalized.contains("no ")
                || normalized.contains("without ")
                || normalized.contains("not implement")
                || normalized.contains("not implemented"))))
        && (normalized.contains("not implement")
            || normalized.contains("not implemented")
            || normalized.contains("missing")
            || normalized.contains("future work")
            || normalized.contains("gap")
            || normalized.contains("unsupported"))
}

fn stale_ai_gap_claim_violation(
    normalized: &str,
    code_truth: &ParityCodeTruth,
) -> Option<&'static str> {
    if code_truth.has(ImplementedSurface::AiAnthropicGoogleThinkingPayloads)
        && normalized.contains("anthropic")
        && normalized.contains("google")
        && normalized.contains("thinking")
        && (normalized.contains("only after")
            || normalized.contains("future work")
            || normalized.contains("missing")
            || normalized.contains("not implemented")
            || normalized.contains("does not translate")
            || normalized.contains("not translate"))
    {
        return Some("stale Anthropic/Google thinking payload gap claim");
    }

    None
}

fn is_rejection_or_gap_statement(normalized: &str) -> bool {
    contains_any_word(
        normalized,
        &[
            "reject",
            "rejects",
            "rejected",
            "unsupported",
            "never",
            "missing",
            "gap",
            "gaps",
        ],
    )
}

fn contains_any_word(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| contains_word(haystack, needle))
}

fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle
        .chars()
        .all(|character| character.is_ascii_alphanumeric())
    {
        haystack
            .split(|character: char| {
                !character.is_ascii_alphanumeric() && character != '.' && character != ':'
            })
            .any(|word| word == needle)
    } else {
        haystack.contains(needle)
    }
}

fn validate_examples(root: &Path) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let minimal_config = root.join("examples/config/minimal.toml");
    if minimal_config.exists() {
        errors.extend(validate_minimal_config(root, &minimal_config)?);
    }

    let mcp_config = root.join("examples/config/mcp-server.toml");
    if mcp_config.exists() {
        errors.extend(validate_mcp_config(root, &mcp_config)?);
    }

    let read_schema = root.join("examples/tools/read-file-schema.json");
    if read_schema.exists() {
        errors.extend(validate_read_file_schema(root, &read_schema)?);
    }

    errors.extend(validate_rust_examples(root)?);
    errors.sort();
    Ok(errors)
}

fn run_rust_examples_compile_gate(root: &Path) -> Result<()> {
    let manifest = root.join("examples/rust/Cargo.toml");
    if !manifest.exists() {
        return Ok(());
    }

    run(&CommandStep {
        program: "cargo".to_owned(),
        args: vec![
            "check".to_owned(),
            "--manifest-path".to_owned(),
            manifest.display().to_string(),
            "--examples".to_owned(),
        ],
    })
}

fn validate_rust_examples(root: &Path) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let example_dir = root.join("examples/rust");
    if !example_dir.is_dir() {
        return Ok(errors);
    }

    let mut example_files = Vec::new();
    collect_files_with_extensions(&example_dir, &["rs"], &mut example_files)?;
    example_files.sort();
    if example_files.is_empty() {
        return Ok(errors);
    }

    let manifest = example_dir.join("Cargo.toml");
    let relative_manifest = relative_path(root, &manifest);
    if !manifest.exists() {
        errors.push("examples/rust contains Rust examples but is missing Cargo.toml".to_owned());
        return Ok(errors);
    }

    let source = fs::read_to_string(&manifest)?;
    let value = match toml::from_str::<toml::Value>(&source) {
        Ok(value) => value,
        Err(error) => {
            errors.push(format!(
                "{} is invalid TOML: {error}",
                relative_manifest.display()
            ));
            return Ok(errors);
        }
    };

    let declared_paths = value
        .get("example")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|target| target.get("path"))
        .filter_map(toml::Value::as_str)
        .map(|path| normalize_path(Path::new(path)))
        .collect::<BTreeSet<_>>();

    if declared_paths.is_empty() {
        errors.push(format!(
            "{} must declare at least one [[example]] target",
            relative_manifest.display()
        ));
        return Ok(errors);
    }

    for example_file in example_files {
        let relative_to_manifest = example_file
            .strip_prefix(&example_dir)
            .map_or_else(|_| normalize_path(&example_file), normalize_path);
        if !declared_paths.contains(&relative_to_manifest) {
            errors.push(format!(
                "{} does not declare example target for {}",
                relative_manifest.display(),
                relative_path(root, &example_file).display()
            ));
        }
    }

    Ok(errors)
}

fn validate_minimal_config(root: &Path, path: &Path) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let relative = relative_path(root, path);
    let source = fs::read_to_string(path)?;
    let keys = top_level_toml_keys(&source);

    for key in [
        "default_provider",
        "default_model",
        "sessions_dir",
        "permissions",
        "defaults",
    ] {
        if !keys.contains(&key) {
            errors.push(format!("{} is missing `{key}`", relative.display()));
        }
    }

    if keys.contains(&"api_base") {
        errors.push(format!(
            "{} must not set api_base in the minimal development fixture",
            relative.display()
        ));
    }

    for key in keys {
        let allowed = [
            "default_provider",
            "default_model",
            "sessions_dir",
            "permissions",
            "defaults",
        ];
        if !allowed.contains(&key) {
            errors.push(format!(
                "{} contains unsupported minimal config key `{key}`",
                relative.display()
            ));
        }
    }

    Ok(errors)
}

fn validate_mcp_config(root: &Path, path: &Path) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let relative = relative_path(root, path);
    let source = fs::read_to_string(path)?;
    let Some(server_body) = first_toml_array_table(&source, "mcp.servers") else {
        errors.push(format!(
            "{} must define [[mcp.servers]]",
            relative.display()
        ));
        return Ok(errors);
    };

    for key in ["id", "enabled", "transport", "command"] {
        if !toml_body_has_key(server_body, key) {
            errors.push(format!(
                "{} mcp server #0 is missing `{key}`",
                relative.display()
            ));
        }
    }

    Ok(errors)
}

fn validate_read_file_schema(root: &Path, path: &Path) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let relative = relative_path(root, path);
    let source = fs::read_to_string(path)?;

    if !source.contains("\"properties\"") {
        errors.push(format!(
            "{} must define object properties",
            relative.display()
        ));
        return Ok(errors);
    }
    if !source.contains("\"path\"") {
        errors.push(format!(
            "{} must include a `path` property",
            relative.display()
        ));
    }
    if !source.contains("\"required\"") || !source.contains("\"path\"") {
        errors.push(format!("{} must require `path`", relative.display()));
    }

    Ok(errors)
}

fn top_level_toml_keys(source: &str) -> Vec<&str> {
    let mut keys = Vec::new();
    let mut in_top_level = true;

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(section) = trimmed
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .filter(|section| !section.starts_with('['))
        {
            keys.push(section.trim());
            in_top_level = false;
            continue;
        }
        if in_top_level && let Some((key, _value)) = trimmed.split_once('=') {
            let key = key.trim();
            if !key.is_empty() {
                keys.push(key);
            }
        }
    }

    keys
}

fn first_toml_array_table<'a>(source: &'a str, table: &str) -> Option<&'a str> {
    let header = format!("[[{table}]]");
    let start = source.find(&header)? + header.len();
    let rest = &source[start..];
    let end = rest.find("\n[[").unwrap_or(rest.len());
    Some(&rest[..end])
}

fn toml_body_has_key(body: &str, expected: &str) -> bool {
    body.lines().any(|line| {
        let trimmed = line.trim();
        trimmed
            .split_once('=')
            .is_some_and(|(key, _value)| key.trim() == expected)
    })
}

fn markdown_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in ["README.md", "docs", "examples"] {
        let path = root.join(entry);
        if path.is_file() && path.extension().is_some_and(|extension| extension == "md") {
            out.push(path);
        } else if path.is_dir() {
            collect_markdown_files(&path, &mut out)?;
        }
    }
    out.sort();
    Ok(out)
}

fn collect_markdown_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_markdown_files(&path, out)?;
        } else if path.extension().is_some_and(|extension| extension == "md") {
            out.push(path);
        }
    }
    Ok(())
}

fn local_link_target(target: &str) -> Option<&str> {
    if target.is_empty()
        || target.starts_with('#')
        || target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
    {
        return None;
    }

    let without_anchor = target
        .split_once('#')
        .map_or(target, |(path, _anchor)| path);
    let without_query = without_anchor
        .split_once('?')
        .map_or(without_anchor, |(path, _query)| path);
    (!without_query.is_empty()).then_some(without_query)
}

fn relative_path(root: &Path, path: &Path) -> PathBuf {
    let relative = path.strip_prefix(root).map_or(path, |stripped| stripped);
    normalize_path(relative)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(component.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_check_runs_xtask_gate() {
        let steps = check_steps(&CheckOptions::default());

        assert_eq!(
            steps,
            vec![
                CommandStep::new("cargo", &["fmt", "-p", "xtask", "--check"]),
                CommandStep::new(
                    "cargo",
                    &[
                        "clippy",
                        "-p",
                        "xtask",
                        "--all-targets",
                        "--",
                        "-D",
                        "warnings"
                    ]
                ),
                CommandStep::new("cargo", &["test", "-p", "xtask"]),
            ]
        );
    }

    #[test]
    fn workspace_check_opts_into_workspace_gate() {
        let steps = check_steps(&CheckOptions {
            workspace: true,
            ..CheckOptions::default()
        });

        assert_eq!(
            steps,
            vec![
                CommandStep::new("cargo", &["fmt", "--all", "--check"]),
                CommandStep::new(
                    "cargo",
                    &[
                        "clippy",
                        "--workspace",
                        "--all-targets",
                        "--all-features",
                        "--",
                        "-D",
                        "warnings"
                    ]
                ),
                CommandStep::new("cargo", &["test", "--workspace", "--all-features"]),
            ]
        );
    }

    #[test]
    fn quick_check_limits_execution_to_xtask_package() {
        let steps = check_steps(&CheckOptions {
            quick: true,
            ..CheckOptions::default()
        });

        assert_eq!(
            steps,
            vec![
                CommandStep::new("cargo", &["fmt", "-p", "xtask", "--check"]),
                CommandStep::new(
                    "cargo",
                    &[
                        "clippy",
                        "-p",
                        "xtask",
                        "--all-targets",
                        "--",
                        "-D",
                        "warnings"
                    ]
                ),
                CommandStep::new("cargo", &["test", "-p", "xtask"]),
            ]
        );
    }

    #[test]
    fn docs_validation_fails_missing_local_markdown_links() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("index.md"),
            "[Missing](./missing.md)\n[Anchor](./index.md#heading)\n[Web](https://example.com)\n",
        )
        .expect("write docs");

        let errors = validate_docs_links(dir.path()).expect("docs validation should run");

        assert_eq!(
            errors,
            vec!["docs/index.md links to missing local file docs/missing.md".to_string()]
        );
    }

    #[test]
    fn parity_validation_rejects_fake_production_defaults_without_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("config.md"),
            "Production defaults may use provider = \"fake\" and http://127.0.0.1:11434/v1.\n",
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/config.md:1 contains production fake/default guidance: Production defaults may use provider = \"fake\" and http://127.0.0.1:11434/v1.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_accepts_explicit_fake_fixture_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("examples").join("config")).expect("examples dir");
        std::fs::write(
            dir.path().join("examples").join("config").join("minimal.toml"),
            "# xtask-parity: allow fake-provider-example - deterministic development fixture.\ndefault_provider = \"fake\"\n",
        )
        .expect("write example");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parity_validation_rejects_allowlisted_fake_production_source() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("crates").join("neo-agent").join("src");
        std::fs::create_dir_all(&src).expect("crate src dir");
        std::fs::write(
            src.join("config.rs"),
            concat!(
                "// xtask-parity: allow fake-provider-example - fixture only.\n",
                "const DEFAULT_PROVIDER: &str = \"fake\";\n",
            ),
        )
        .expect("write production source");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "crates/neo-agent/src/config.rs:2 contains production fake/default guidance: const DEFAULT_PROVIDER: &str = \"fake\";".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_local_production_guidance() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("providers.md"),
            "Production deployments can use the local provider by default.\n",
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/providers.md:1 contains production local-provider guidance: Production deployments can use the local provider by default.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_softened_local_production_guidance() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("providers.md"),
            concat!(
                "Production local provider is not risky.\n",
                "Future production deployments can use the local provider.\n",
            ),
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/providers.md:1 contains production local-provider guidance: Production local provider is not risky.".to_string(),
                "docs/providers.md:2 contains production local-provider guidance: Future production deployments can use the local provider.".to_string(),
            ]
        );
    }

    #[test]
    fn parity_validation_allows_local_provider_rejection_docs() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("providers.md"),
            "ProviderResolver rejects test-only/local providers in production resolution.\n",
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parity_validation_scans_production_crate_sources() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("crates").join("neo-agent").join("src");
        std::fs::create_dir_all(&src).expect("crate src dir");
        std::fs::write(
            src.join("config.rs"),
            "const DEFAULT_PROVIDER: &str = \"fake\";\n",
        )
        .expect("write production source");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "crates/neo-agent/src/config.rs:1 contains production fake/default guidance: const DEFAULT_PROVIDER: &str = \"fake\";".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_allows_explicit_source_fixtures() {
        let dir = tempfile::tempdir().expect("tempdir");
        let provider_src = dir
            .path()
            .join("crates")
            .join("ai")
            .join("src")
            .join("providers");
        let core_src = dir.path().join("crates").join("agent-core").join("src");
        std::fs::create_dir_all(&provider_src).expect("provider src dir");
        std::fs::create_dir_all(&core_src).expect("core src dir");
        std::fs::write(
            provider_src.join("fake.rs"),
            "pub struct FakeModelClient;\n",
        )
        .expect("write fake provider");
        std::fs::write(core_src.join("harness.rs"), "pub fn fake_model() {}\n")
            .expect("write harness");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parity_validation_allows_gap_and_checker_descriptions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mcp_source = dir
            .path()
            .join("crates")
            .join("agent-core")
            .join("src")
            .join("tools");
        let session_source = dir
            .path()
            .join("crates")
            .join("agent-core")
            .join("src")
            .join("session");
        let agent_source = dir.path().join("crates").join("neo-agent").join("src");
        std::fs::create_dir_all(&mcp_source).expect("mcp source dir");
        std::fs::create_dir_all(&session_source).expect("session source dir");
        std::fs::create_dir_all(&agent_source).expect("agent source dir");
        std::fs::write(
            mcp_source.join("mcp.rs"),
            "pub trait McpToolAdapter {}\npub struct McpToolProvider;\n",
        )
        .expect("write mcp source");
        std::fs::write(
            session_source.join("mod.rs"),
            "impl SessionMetadataStore { pub fn fork(&self) {} pub fn rename(&self) {} }\n",
        )
        .expect("write session source");
        std::fs::write(
            agent_source.join("cli.rs"),
            "pub enum ExtensionCommand { Status, Enable, Disable }\n",
        )
        .expect("write cli source");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            dir.path().join("docs").join("gap").join("neo-agent.md"),
            "Keep quickstart scoped until interactive mode is no longer placeholder-level.\n",
        )
        .expect("write gap doc");
        std::fs::write(
            dir.path().join("docs").join("quickstart.md"),
            concat!(
                "`--docs` scans for production docs\n",
                "that point at fake or placeholder provider paths.\n",
                "It also scans for fake/local/placeholder production guidance.\n",
                "It rejects stale text such as \"no MCP adapter is wired\" once symbols exist.\n",
                "It rejects \"extension lifecycle unavailable\" once lifecycle commands exist.\n",
                "It rejects \"session branching and naming are future work\" once fork exists.\n",
            ),
        )
        .expect("write quickstart");
        std::fs::write(
            dir.path().join("docs").join("mcp.md"),
            "A compile-time Rust stub should only be added after adjacent modules are stable.\n",
        )
        .expect("write mcp");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parity_validation_rejects_stale_mcp_adapter_gap_after_adapter_symbol_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_dir = dir
            .path()
            .join("crates")
            .join("agent-core")
            .join("src")
            .join("tools");
        std::fs::create_dir_all(&source_dir).expect("mcp source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            source_dir.join("mcp.rs"),
            "pub trait McpToolAdapter {}\npub struct McpToolProvider;\n",
        )
        .expect("write mcp source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("INDEX.md"),
            "No MCP client adapter is wired into neo-agent-core yet.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/INDEX.md:1 contains stale MCP adapter gap claim: No MCP client adapter is wired into neo-agent-core yet.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_mcp_process_gap_after_stdio_adapter_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_dir = dir
            .path()
            .join("crates")
            .join("agent-core")
            .join("src")
            .join("tools");
        std::fs::create_dir_all(&source_dir).expect("mcp source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            source_dir.join("mcp.rs"),
            concat!(
                "pub trait McpToolAdapter {}\n",
                "pub struct McpToolProvider;\n",
                "pub struct McpStdioToolAdapter;\n",
                "const METHODS: &[&str] = &[\"tools/list\", \"tools/call\"];\n",
            ),
        )
        .expect("write mcp source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("INDEX.md"),
            "Neo still does not yet spawn external MCP server processes.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/INDEX.md:1 contains stale MCP process adapter gap claim: Neo still does not yet spawn external MCP server processes.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_http_mcp_json_subscribe_gap_after_event_reader_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_dir = dir
            .path()
            .join("crates")
            .join("agent-core")
            .join("src")
            .join("tools");
        std::fs::create_dir_all(&source_dir).expect("mcp source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            source_dir.join("mcp.rs"),
            concat!(
                "impl McpHttpToolAdapter {\n",
                "  async fn start_resource_event_reader(&self) {}\n",
                "}\n",
            ),
        )
        .expect("write mcp source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("INDEX.md"),
            "HTTP MCP JSON subscribe ACK cannot receive resource updates yet.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/INDEX.md:1 contains stale HTTP MCP JSON subscribe event gap claim: HTTP MCP JSON subscribe ACK cannot receive resource updates yet.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_extension_lifecycle_gap_after_commands_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_dir = dir.path().join("crates").join("neo-agent").join("src");
        std::fs::create_dir_all(&source_dir).expect("agent source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            source_dir.join("cli.rs"),
            "pub enum ExtensionCommand { Status, Enable, Disable }\n",
        )
        .expect("write cli source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("neo-agent.md"),
            "Do not document extension lifecycle management as available Neo features yet.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/neo-agent.md:1 contains stale extension lifecycle gap claim: Do not document extension lifecycle management as available Neo features yet.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_session_branching_gap_after_fork_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_dir = dir
            .path()
            .join("crates")
            .join("agent-core")
            .join("src")
            .join("session");
        std::fs::create_dir_all(&source_dir).expect("session source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            source_dir.join("mod.rs"),
            "impl SessionMetadataStore { pub fn fork(&self) {} pub fn rename(&self) {} }\n",
        )
        .expect("write session source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("INDEX.md"),
            "Session tree branching and naming remain pi-inspired future work.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/INDEX.md:1 contains stale session branching gap claim: Session tree branching and naming remain pi-inspired future work.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_live_session_picker_gap_after_interactive_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let interactive_dir = dir
            .path()
            .join("crates")
            .join("neo-agent")
            .join("src")
            .join("modes");
        let input_dir = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&interactive_dir).expect("interactive source dir");
        std::fs::create_dir_all(&input_dir).expect("input source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            interactive_dir.join("interactive.rs"),
            "fn open_session_picker() {} fn load_selected_session() {} fn session_catalog_for_config() {}\n",
        )
        .expect("write interactive source");
        std::fs::write(
            input_dir.join("input.rs"),
            "enum Key { SessionPickerOpen }\n",
        )
        .expect("write input source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("neo-agent.md"),
            "The live session picker remains future work.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/neo-agent.md:1 contains stale live session picker gap claim: The live session picker remains future work.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_live_model_picker_gap_after_interactive_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let interactive_dir = dir
            .path()
            .join("crates")
            .join("neo-agent")
            .join("src")
            .join("modes");
        let input_dir = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&interactive_dir).expect("interactive source dir");
        std::fs::create_dir_all(&input_dir).expect("input source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            interactive_dir.join("interactive.rs"),
            "fn open_model_picker() {} fn apply_selected_model() {} fn model_catalog_for_config() {}\n",
        )
        .expect("write interactive source");
        std::fs::write(input_dir.join("input.rs"), "enum Key { ModelPickerOpen }\n")
            .expect("write input source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("neo-agent.md"),
            "The live model picker is still missing.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/neo-agent.md:1 contains stale live model picker gap claim: The live model picker is still missing.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_fork_before_continue_gap_after_session_fork_ui_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let interactive_dir = dir
            .path()
            .join("crates")
            .join("neo-agent")
            .join("src")
            .join("modes");
        let input_dir = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&interactive_dir).expect("interactive source dir");
        std::fs::create_dir_all(&input_dir).expect("input source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            interactive_dir.join("interactive.rs"),
            "fn fork_selected_session() {} fn fork_session_transcript() {}\n",
        )
        .expect("write interactive source");
        std::fs::write(
            input_dir.join("input.rs"),
            "enum Action { SessionFork } const ID: &str = \"tui.session.fork\";\n",
        )
        .expect("write input source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("neo-agent.md"),
            "Explicit fork-before-continue controls are still a gap beyond local JSONL append.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/neo-agent.md:1 contains stale interactive session fork gap claim: Explicit fork-before-continue controls are still a gap beyond local JSONL append.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_runtime_hook_queue_gap_after_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_dir = dir.path().join("crates").join("agent-core").join("src");
        std::fs::create_dir_all(&source_dir).expect("agent-core source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            source_dir.join("runtime.rs"),
            concat!(
                "impl AgentConfig {\n",
                "  pub fn with_before_tool_call(&self) {}\n",
                "  pub fn with_after_tool_call(&self) {}\n",
                "  pub fn with_queue_modes(&self) {}\n",
                "}\n",
                "impl AgentContext { pub fn queue_steering_message(&self) {} }\n",
            ),
        )
        .expect("write runtime source");
        std::fs::write(
            dir.path()
                .join("docs")
                .join("gap")
                .join("neo-agent-core.md"),
            "Add hook/steering docs only when the runtime exposes those APIs.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/neo-agent-core.md:1 contains stale runtime hook/queue gap claim: Add hook/steering docs only when the runtime exposes those APIs.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_tui_diff_gap_after_renderer_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tui_src = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&tui_src).expect("tui source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            tui_src.join("app.rs"),
            "enum TranscriptLine { DiffAdded, DiffRemoved }\n",
        )
        .expect("write tui app source");
        std::fs::write(
            tui_src.join("components.rs"),
            "fn transcript_line_style() {}\n",
        )
        .expect("write tui components source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("tui.md"),
            "Keep TUI docs scoped until diff rendering lands.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/tui.md:1 contains stale TUI unified diff renderer gap claim: Keep TUI docs scoped until diff rendering lands.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_allows_advanced_diff_affordance_gaps_after_basic_renderer_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tui_src = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&tui_src).expect("tui source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            tui_src.join("app.rs"),
            "enum TranscriptLine { DiffAdded, DiffRemoved }\n",
        )
        .expect("write tui app source");
        std::fs::write(
            tui_src.join("components.rs"),
            "fn transcript_line_style() {}\n",
        )
        .expect("write tui components source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("tui.md"),
            "Advanced diff affordances remain not implemented.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(errors, Vec::<String>::new());
    }

    #[test]
    fn parity_validation_rejects_stale_tui_paste_buffering_gap_after_input_parser_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tui_src = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&tui_src).expect("tui source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            tui_src.join("input.rs"),
            concat!(
                "struct InputParser;\n",
                "const BRACKETED_PASTE_START: &[u8] = b\"\\x1b[200~\";\n",
                "const BRACKETED_PASTE_END: &[u8] = b\"\\x1b[201~\";\n",
            ),
        )
        .expect("write tui input source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("tui.md"),
            "Keep TUI docs scoped until stdin buffering lands.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/tui.md:1 contains stale TUI paste buffering gap claim: Keep TUI docs scoped until stdin buffering lands.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_tui_transcript_selection_copy_gap_after_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tui_src = dir.path().join("crates").join("tui").join("src");
        let interactive_dir = dir
            .path()
            .join("crates")
            .join("neo-agent")
            .join("src")
            .join("modes");
        std::fs::create_dir_all(&tui_src).expect("tui source dir");
        std::fs::create_dir_all(&interactive_dir).expect("interactive source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            tui_src.join("app.rs"),
            concat!(
                "struct TranscriptSelection;\n",
                "impl ChatTranscript { fn copy_selection(&self) {} }\n",
                "impl NeoTuiApp { fn copy_selected_transcript_text(&self) {} }\n",
            ),
        )
        .expect("write tui app source");
        std::fs::write(
            tui_src.join("components.rs"),
            "impl TranscriptWidget<'_> { fn with_selection(&self) {} }\n",
        )
        .expect("write tui components source");
        std::fs::write(
            tui_src.join("input.rs"),
            concat!(
                "enum Action { TranscriptSelectionStart, TranscriptCopySelection }\n",
                "const ID: &str = \"tui.transcript.copySelection\";\n",
            ),
        )
        .expect("write tui input source");
        std::fs::write(
            interactive_dir.join("interactive.rs"),
            "fn copy_transcript_selection_to_clipboard() {}\n",
        )
        .expect("write interactive source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("tui.md"),
            "Selected transcript-region copy remains future work.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/tui.md:1 contains stale TUI transcript selection copy gap claim: Selected transcript-region copy remains future work.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_terminal_image_protocol_gap_after_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tui_src = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&tui_src).expect("tui source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            tui_src.join("image.rs"),
            concat!(
                "pub enum ImageProtocolError {}\n",
                "pub struct KittyGraphicsOptions;\n",
                "pub fn encode_kitty_graphics() {}\n",
                "pub fn encode_iterm2_inline_image() {}\n",
            ),
        )
        .expect("write tui image source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("tui.md"),
            "Terminal image protocols remain not implemented.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/tui.md:1 contains stale terminal image protocol gap claim: Terminal image protocols remain not implemented.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_allows_specific_unimplemented_image_protocol_gaps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tui_src = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&tui_src).expect("tui source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            tui_src.join("image.rs"),
            concat!(
                "pub enum ImageProtocolError {}\n",
                "pub struct KittyGraphicsOptions;\n",
                "pub fn encode_kitty_graphics() {}\n",
            ),
        )
        .expect("write tui image source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("tui.md"),
            "Sixel and full renderer integration remain explicit image protocol gaps.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(errors, Vec::<String>::new());
    }

    #[test]
    fn parity_validation_rejects_stale_ai_thinking_gap_after_payload_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let providers_dir = dir
            .path()
            .join("crates")
            .join("ai")
            .join("src")
            .join("providers");
        std::fs::create_dir_all(&providers_dir).expect("ai provider source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            providers_dir.join("anthropic.rs"),
            "fn thinking_budget_tokens() { let _ = \"budget_tokens\"; }\n",
        )
        .expect("write anthropic source");
        std::fs::write(
            providers_dir.join("google.rs"),
            "fn thinking_budget_tokens() { let _ = \"thinkingConfig\"; }\n",
        )
        .expect("write google source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("neo-ai.md"),
            "Add Anthropic and Google thinking controls only after Neo has explicit budget contracts.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/neo-ai.md:1 contains stale Anthropic/Google thinking payload gap claim: Add Anthropic and Google thinking controls only after Neo has explicit budget contracts.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_ai_thinking_translation_gap_after_payload_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let providers_dir = dir
            .path()
            .join("crates")
            .join("ai")
            .join("src")
            .join("providers");
        std::fs::create_dir_all(&providers_dir).expect("ai provider source dir");
        std::fs::create_dir_all(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            providers_dir.join("anthropic.rs"),
            "fn thinking_budget_tokens() { let _ = \"budget_tokens\"; }\n",
        )
        .expect("write anthropic source");
        std::fs::write(
            providers_dir.join("google.rs"),
            "fn thinking_budget_tokens() { let _ = \"thinkingConfig\"; }\n",
        )
        .expect("write google source");
        std::fs::write(
            dir.path().join("docs").join("providers.md"),
            "Neo intentionally does not translate reasoning effort into Anthropic or Google thinking payloads yet.\n",
        )
        .expect("write provider doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/providers.md:1 contains stale Anthropic/Google thinking payload gap claim: Neo intentionally does not translate reasoning effort into Anthropic or Google thinking payloads yet.".to_string()
            ]
        );
    }

    #[test]
    fn examples_validation_requires_rust_example_harness_for_rust_sources() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rust_examples = dir.path().join("examples").join("rust");
        std::fs::create_dir_all(&rust_examples).expect("rust examples dir");
        std::fs::write(rust_examples.join("tool_schema.rs"), "fn main() {}\n")
            .expect("write rust example");

        let errors = validate_examples(dir.path()).expect("examples validation should run");

        assert_eq!(
            errors,
            vec!["examples/rust contains Rust examples but is missing Cargo.toml".to_string()]
        );
    }

    #[test]
    fn examples_validation_requires_rust_harness_to_cover_each_example_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rust_examples = dir.path().join("examples").join("rust");
        std::fs::create_dir_all(&rust_examples).expect("rust examples dir");
        std::fs::write(rust_examples.join("provider_registry.rs"), "fn main() {}\n")
            .expect("write provider example");
        std::fs::write(rust_examples.join("tool_schema.rs"), "fn main() {}\n")
            .expect("write tool example");
        std::fs::write(
            rust_examples.join("Cargo.toml"),
            concat!(
                "[package]\n",
                "name = \"neo-rust-examples\"\n",
                "version = \"0.0.0\"\n",
                "edition = \"2024\"\n",
                "\n",
                "[[example]]\n",
                "name = \"provider_registry\"\n",
                "path = \"provider_registry.rs\"\n",
            ),
        )
        .expect("write harness manifest");

        let errors = validate_examples(dir.path()).expect("examples validation should run");

        assert_eq!(
            errors,
            vec![
                "examples/rust/Cargo.toml does not declare example target for examples/rust/tool_schema.rs".to_string()
            ]
        );
    }
}
