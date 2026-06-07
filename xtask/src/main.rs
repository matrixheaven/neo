use std::{
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

            if let Some(reason) = parity_line_violation(trimmed, explicit_fixture_path) {
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

fn parity_line_violation(line: &str, explicit_fixture_path: bool) -> Option<&'static str> {
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

    errors.sort();
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
}
