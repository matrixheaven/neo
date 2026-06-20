use globset::GlobSetBuilder;
use ignore::WalkBuilder;
use regex::RegexBuilder;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, parse_input, schema};

const DEFAULT_HEAD_LIMIT: usize = 250;

#[derive(Debug, Clone, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum OutputMode {
    #[default]
    FilesWithMatches,
    Content,
    CountMatches,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
struct GrepInput {
    pattern: String,
    #[serde(default = "default_path")]
    #[schemars(
        description = "File or directory to search. Accepts an absolute path, or a path relative to the current working directory. Omit to search the current working directory."
    )]
    path: std::path::PathBuf,
    #[serde(default)]
    #[schemars(
        description = "Optional glob filter to restrict searched files, e.g. '*.rs' or 'src/**/*.ts'. Supports * and **."
    )]
    glob: Option<String>,
    #[serde(default, rename = "type")]
    #[schemars(
        description = "Optional file extension filter, e.g. 'rs' matches '*.rs'. Prefer this over glob when filtering by language or file kind."
    )]
    type_filter: Option<String>,
    #[serde(default)]
    #[schemars(
        description = "Shape of the result. 'content' shows matching lines; 'files_with_matches' shows only the paths of files that contain a match; 'count_matches' shows the count of matches per file. Defaults to 'files_with_matches'."
    )]
    output_mode: OutputMode,
    #[serde(default, rename = "-i")]
    #[schemars(description = "Perform a case-insensitive search. Defaults to false.")]
    case_insensitive: bool,
    #[serde(default = "default_line_numbers", rename = "-n")]
    #[schemars(
        description = "Prefix each matching line with its line number. Applies only when output_mode is 'content'. Defaults to true."
    )]
    line_numbers: bool,
    #[serde(default, rename = "-A")]
    #[schemars(
        description = "Number of lines to show after each match. Applies only when output_mode is 'content'."
    )]
    context_after: usize,
    #[serde(default, rename = "-B")]
    #[schemars(
        description = "Number of lines to show before each match. Applies only when output_mode is 'content'."
    )]
    context_before: usize,
    #[serde(default, rename = "-C")]
    #[schemars(
        description = "Number of lines to show before and after each match. Applies only when output_mode is 'content'; takes precedence over -A and -B."
    )]
    context: usize,
    #[serde(default = "default_head_limit")]
    #[schemars(
        description = "Maximum number of result lines or entries to return. 0 means unlimited. Defaults to 250."
    )]
    head_limit: usize,
    #[serde(default)]
    #[schemars(
        description = "Number of leading results to skip before applying head_limit. Use together with head_limit to page through large result sets. Defaults to 0."
    )]
    offset: usize,
    #[serde(default)]
    #[schemars(
        description = "Enable multiline matching, where the pattern can span line boundaries and '.' also matches newlines. Defaults to false."
    )]
    multiline: bool,
    #[serde(default)]
    #[schemars(
        description = "Also search files excluded by ignore files such as .gitignore, .ignore, and .rgignore (for example node_modules or build outputs). Defaults to false."
    )]
    include_ignored: bool,
}

fn default_path() -> std::path::PathBuf {
    ".".into()
}

const fn default_line_numbers() -> bool {
    true
}

const fn default_head_limit() -> usize {
    DEFAULT_HEAD_LIMIT
}

pub struct GrepTool;

impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "Grep"
    }

    fn description(&self) -> &'static str {
        "Search workspace text files using regular expressions.\n\
        \n\
        Use Grep when the task is to find unknown content or unknown file locations. Do not use \
        shell `grep` or `rg` directly; this tool applies workspace path policy, output limits, and \
        ignore-file handling. If you already know a concrete file path and need to inspect its \
        contents, use Read directly instead.\n\
        \n\
        Write patterns in Rust/regex crate syntax, which differs from POSIX `grep`. For example, \
        braces are special, so escape them as `\\{` to match a literal `{`.\n\
        \n\
        Hidden files (dotfiles such as `.gitlab-ci.yml` or `.eslintrc.json`) are searched by default. \
        To also search files excluded by `.gitignore` (such as `node_modules` or build outputs), set \
        `include_ignored` to `true`.\n\
        \n\
        Parameters:\n\
        - pattern: Regular expression to search for.\n\
        - path: File or directory to search. Relative paths resolve against the working directory.\n\
        - glob: Optional glob filter to restrict searched files (e.g. '*.rs').\n\
        - type: Optional file extension filter (e.g. 'rs' matches '*.rs').\n\
        - output_mode: 'content' (matching lines), 'files_with_matches' (paths only), or \
          'count_matches' (count per file). Defaults to 'files_with_matches'.\n\
        - -i: Case-insensitive search.\n\
        - -n: Prefix matching lines with line numbers in 'content' mode. Defaults to true.\n\
        - -A, -B, -C: Context lines around matches in 'content' mode; -C takes precedence.\n\
        - head_limit: Maximum number of result lines/entries. 0 means unlimited. Defaults to 250.\n\
        - offset: Skip this many leading results. Use with head_limit for pagination.\n\
        - multiline: Allow '.' to match newlines and matches to span lines.\n\
        - include_ignored: Search files ignored by .gitignore and friends.\n\
        \n\
        Output format:\n\
        - 'content' returns matching lines prefixed with `file:line:text` when -n is true, or \
          `file:text` when -n is false. Context lines use the same prefix. Non-contiguous context \
          groups are separated by `--`.\n\
        - 'files_with_matches' returns one relative path per line.\n\
        - 'count_matches' returns `path:count` lines.\n\
        - A `<system>...</system>` status block is appended with the number of matches/files found, \
          the number returned, and any truncation/pagination info."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<GrepInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_file_read_allowed()?;
            let input: GrepInput = parse_input(self.name(), input)?;
            let root = ctx.resolve_workspace_path(&input.path)?;
            let workspace = ctx.workspace_root().to_path_buf();

            let mut regex_builder = RegexBuilder::new(&input.pattern);
            regex_builder.case_insensitive(input.case_insensitive);
            if input.multiline {
                regex_builder.dot_matches_new_line(true);
            }
            let regex = regex_builder.build()?;

            let glob_matcher = build_glob_matcher(&input)?;

            let max_collect = if input.head_limit > 0 {
                input
                    .head_limit
                    .saturating_add(input.offset)
                    .saturating_add(1)
            } else {
                usize::MAX
            };

            let records = tokio::task::spawn_blocking({
                let input = input.clone();
                move || {
                    let mut collected = Vec::new();
                    let mut walk_builder = WalkBuilder::new(&root);
                    walk_builder.hidden(false);
                    walk_builder.ignore(!input.include_ignored);
                    walk_builder.git_ignore(!input.include_ignored);
                    walk_builder.git_global(!input.include_ignored);
                    walk_builder.git_exclude(!input.include_ignored);

                    for entry in walk_builder.build() {
                        let entry = entry.map_err(std::io::Error::other)?;
                        if !entry
                            .file_type()
                            .is_some_and(|file_type| file_type.is_file())
                        {
                            continue;
                        }

                        let relative_to_root =
                            entry.path().strip_prefix(&root).unwrap_or(entry.path());
                        if !matches_filters(
                            relative_to_root,
                            glob_matcher.as_ref(),
                            input.type_filter.as_deref(),
                        ) {
                            continue;
                        }

                        let Ok(content) = std::fs::read_to_string(entry.path()) else {
                            continue;
                        };
                        let display = entry
                            .path()
                            .strip_prefix(&workspace)
                            .unwrap_or(entry.path());
                        let display_str = display.display().to_string();

                        collect_matches(
                            &input,
                            &regex,
                            &content,
                            display_str,
                            &mut collected,
                            max_collect,
                        );

                        if collected.len() >= max_collect {
                            break;
                        }
                    }
                    Ok::<_, std::io::Error>(collected)
                }
            })
            .await
            .map_err(std::io::Error::other)??;

            let output = format_output(&input, &records);
            Ok(ToolResult::ok(output))
        })
    }
}

fn build_glob_matcher(input: &GrepInput) -> Result<Option<globset::GlobSet>, ToolError> {
    let mut builder = GlobSetBuilder::new();
    let mut has_pattern = false;

    if let Some(glob) = input.glob.as_deref()
        && !glob.is_empty()
    {
        let glob = globset::GlobBuilder::new(glob)
            .literal_separator(false)
            .build()
            .map_err(|err| ToolError::InvalidInput {
                tool: "Grep".to_owned(),
                message: format!("invalid glob pattern '{glob}': {err}"),
            })?;
        builder.add(glob);
        has_pattern = true;
    }

    if let Some(ext) = input.type_filter.as_deref()
        && !ext.is_empty()
    {
        let pattern = format!("*.{ext}");
        let glob = globset::GlobBuilder::new(&pattern)
            .literal_separator(false)
            .build()
            .map_err(|err| ToolError::InvalidInput {
                tool: "Grep".to_owned(),
                message: format!("invalid type filter '{ext}': {err}"),
            })?;
        builder.add(glob);
        has_pattern = true;
    }

    if !has_pattern {
        return Ok(None);
    }

    builder
        .build()
        .map(Some)
        .map_err(|err| ToolError::InvalidInput {
            tool: "Grep".to_owned(),
            message: format!("invalid glob matcher: {err}"),
        })
}

fn matches_filters(
    relative_to_root: &std::path::Path,
    glob_matcher: Option<&globset::GlobSet>,
    type_filter: Option<&str>,
) -> bool {
    if let Some(matcher) = glob_matcher {
        let candidate = relative_to_root.to_string_lossy();
        if !matcher.is_match(candidate.as_ref()) {
            return false;
        }
    }

    if let Some(ext) = type_filter {
        if ext.is_empty() {
            return true;
        }
        return relative_to_root
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(ext));
    }

    true
}

#[derive(Debug, Clone)]
enum MatchRecord {
    File(String),
    Count(String, usize),
    ContentLine {
        file: String,
        line_no: usize,
        text: String,
    },
    Separator,
}

fn collect_matches(
    input: &GrepInput,
    regex: &regex::Regex,
    content: &str,
    display: String,
    records: &mut Vec<MatchRecord>,
    max_collect: usize,
) {
    match input.output_mode {
        OutputMode::FilesWithMatches => {
            if regex.is_match(content) {
                records.push(MatchRecord::File(display));
            }
        }
        OutputMode::CountMatches => {
            let count = regex.find_iter(content).count();
            if count > 0 {
                records.push(MatchRecord::Count(display, count));
            }
        }
        OutputMode::Content => {
            if input.multiline {
                for m in regex.find_iter(content) {
                    let line_no = content[..m.start()].matches('\n').count() + 1;
                    let text = collapse_match_text(&content[m.start()..m.end()]);
                    records.push(MatchRecord::ContentLine {
                        file: display.clone(),
                        line_no,
                        text,
                    });
                    if records.len() >= max_collect {
                        break;
                    }
                }
            } else {
                let lines: Vec<&str> = content.lines().collect();
                let mut matched_indices = Vec::new();
                for (idx, line) in lines.iter().enumerate() {
                    if regex.is_match(line) {
                        matched_indices.push(idx);
                    }
                }

                let after = if input.context > 0 {
                    input.context
                } else {
                    input.context_after
                };
                let before = if input.context > 0 {
                    input.context
                } else {
                    input.context_before
                };
                let groups = build_context_groups(&matched_indices, before, after, lines.len());

                let has_context =
                    input.context > 0 || input.context_before > 0 || input.context_after > 0;
                for (group_idx, (start, end)) in groups.iter().enumerate() {
                    if group_idx > 0 && has_context {
                        records.push(MatchRecord::Separator);
                    }
                    for (line_idx, line) in lines.iter().enumerate().take(*end + 1).skip(*start) {
                        records.push(MatchRecord::ContentLine {
                            file: display.clone(),
                            line_no: line_idx + 1,
                            text: (*line).to_owned(),
                        });
                        if records.len() >= max_collect {
                            return;
                        }
                    }
                }
            }
        }
    }
}

fn collapse_match_text(text: &str) -> String {
    text.lines().map(str::trim).collect::<Vec<_>>().join(" ")
}

fn build_context_groups(
    matched: &[usize],
    before: usize,
    after: usize,
    total: usize,
) -> Vec<(usize, usize)> {
    if matched.is_empty() || total == 0 {
        return Vec::new();
    }
    let mut groups = Vec::new();
    let mut start = matched[0].saturating_sub(before);
    let mut end = matched[0].saturating_add(after).min(total - 1);

    for &idx in &matched[1..] {
        let new_start = idx.saturating_sub(before);
        let new_end = idx.saturating_add(after).min(total - 1);
        if new_start <= end.saturating_add(1) {
            end = new_end;
        } else {
            groups.push((start, end));
            start = new_start;
            end = new_end;
        }
    }
    groups.push((start, end));
    groups
}

#[derive(Debug)]
struct GrepSummary {
    records: usize,
    files: usize,
    matches: usize,
}

fn summarize_records(records: &[MatchRecord], mode: &OutputMode) -> GrepSummary {
    let mut files = 0;
    let mut matches = 0;

    for record in records {
        match record {
            MatchRecord::File(_) => {
                files += 1;
                matches += 1;
            }
            MatchRecord::Count(_, count) => {
                files += 1;
                matches += count;
            }
            MatchRecord::ContentLine { .. } => {
                matches += 1;
            }
            MatchRecord::Separator => {}
        }
    }

    let record_count = match mode {
        OutputMode::FilesWithMatches | OutputMode::CountMatches => files,
        OutputMode::Content => matches,
    };

    GrepSummary {
        records: record_count,
        files,
        matches,
    }
}

fn format_output(input: &GrepInput, records: &[MatchRecord]) -> String {
    let summary = summarize_records(records, &input.output_mode);
    let offset = input.offset;
    let head_limit = input.head_limit;
    let limit_active = head_limit > 0;

    let skipped = offset.min(records.len());
    let after_offset: Vec<&MatchRecord> = records.iter().skip(skipped).collect();

    let (limited, truncated) = if limit_active {
        let end = head_limit.min(after_offset.len());
        let truncated = summary.records.saturating_sub(skipped) > head_limit;
        (
            after_offset.iter().take(end).copied().collect::<Vec<_>>(),
            truncated,
        )
    } else {
        (after_offset.clone(), false)
    };

    let mut lines = Vec::new();
    for record in &limited {
        match record {
            MatchRecord::File(path) => lines.push(path.clone()),
            MatchRecord::Count(path, count) => lines.push(format!("{path}:{count}")),
            MatchRecord::ContentLine {
                file,
                line_no,
                text,
            } => {
                if input.line_numbers {
                    lines.push(format!("{file}:{line_no}:{text}"));
                } else {
                    lines.push(format!("{file}:{text}"));
                }
            }
            MatchRecord::Separator => lines.push("--".to_owned()),
        }
    }

    let rendered = lines.join("\n");
    let message = format_system_message(input, &summary, skipped, limited.len(), truncated);

    if rendered.is_empty() {
        format!("<system>{message}</system>")
    } else {
        format!("{rendered}\n<system>{message}</system>")
    }
}

fn format_system_message(
    input: &GrepInput,
    summary: &GrepSummary,
    skipped: usize,
    returned: usize,
    truncated: bool,
) -> String {
    let mut parts = Vec::new();

    match input.output_mode {
        OutputMode::FilesWithMatches => {
            let file_word = if summary.files == 1 { "file" } else { "files" };
            parts.push(format!(
                "Found {total_files} {file_word} with matches.",
                total_files = summary.files
            ));
        }
        OutputMode::CountMatches => {
            let occurrence_word = if summary.matches == 1 {
                "occurrence"
            } else {
                "occurrences"
            };
            let file_word = if summary.files == 1 { "file" } else { "files" };
            parts.push(format!(
                "Found {total_matches} {occurrence_word} across {total_files} {file_word}.",
                total_matches = summary.matches,
                total_files = summary.files
            ));
        }
        OutputMode::Content => {
            let line_word = if summary.matches == 1 {
                "line"
            } else {
                "lines"
            };
            parts.push(format!(
                "Found {total_matches} matching {line_word}.",
                total_matches = summary.matches
            ));
        }
    }

    if skipped > 0 {
        parts.push(format!(
            "Skipped first {skipped} results (offset={offset}).",
            offset = input.offset
        ));
    }

    if returned > 0 {
        parts.push(format!("Returned {returned} results."));
    }

    if truncated {
        let next_offset = input.offset + input.head_limit;
        parts.push(format!(
            "Results truncated. Use offset={next_offset} with the same head_limit to see more."
        ));
    }

    if summary.records == 0 {
        parts.push("No matches found.".to_owned());
    }

    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PermissionPolicy, ToolContext};
    use serde_json::json;

    fn setup_workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("foo.rs"), "fn main() {}\nlet x = 1;\n")
            .expect("write foo.rs");
        std::fs::write(dir.path().join("bar.txt"), "hello world\nHello World\n")
            .expect("write bar.txt");
        std::fs::write(dir.path().join("baz.rs"), "// baz\nfn foo() {}\n").expect("write baz.rs");
        std::fs::create_dir_all(dir.path().join("sub")).expect("mkdir sub");
        std::fs::write(
            dir.path().join("sub").join("qux.rs"),
            "// sub\nfn qux() {}\n",
        )
        .expect("write qux.rs");
        dir
    }

    async fn run_grep(ctx: &ToolContext, pattern: &str, extra: serde_json::Value) -> ToolResult {
        let mut input = json!({ "pattern": pattern });
        if let Some(obj) = input.as_object_mut()
            && let serde_json::Value::Object(extra_obj) = extra
        {
            for (k, v) in extra_obj {
                obj.insert(k, v);
            }
        }
        GrepTool.execute(ctx, input).await.expect("execute")
    }

    #[tokio::test]
    async fn content_mode_returns_matching_lines() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_grep(&ctx, "fn main", json!({ "output_mode": "content" })).await;
        assert!(result.content.contains("foo.rs:1:fn main() {}"));
        assert!(result.content.contains("Found 1 matching line"));
        assert!(result.content.contains("<system>"));
    }

    #[tokio::test]
    async fn files_with_matches_is_default() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_grep(&ctx, "fn", json!({})).await;
        assert!(result.content.contains("foo.rs"));
        assert!(result.content.contains("baz.rs"));
        assert!(result.content.contains("sub/qux.rs"));
        assert!(!result.content.contains(':'));
        assert!(result.content.contains("Found 3 files with matches"));
    }

    #[tokio::test]
    async fn count_matches_mode() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_grep(&ctx, "fn", json!({ "output_mode": "count_matches" })).await;
        assert!(result.content.contains("foo.rs:1"));
        assert!(result.content.contains("baz.rs:1"));
        assert!(result.content.contains("sub/qux.rs:1"));
    }

    #[tokio::test]
    async fn case_insensitive_search() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_grep(
            &ctx,
            "hello",
            json!({ "output_mode": "content", "-i": true }),
        )
        .await;
        assert!(result.content.contains("bar.txt:1:hello world"));
        assert!(result.content.contains("bar.txt:2:Hello World"));
    }

    #[tokio::test]
    async fn glob_filter_restricts_files() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_grep(
            &ctx,
            "fn",
            json!({ "output_mode": "files_with_matches", "glob": "*.txt" }),
        )
        .await;
        assert!(!result.content.contains("foo.rs"));
        assert!(!result.content.contains("baz.rs"));
        assert!(!result.content.contains("sub/qux.rs"));
        assert!(result.content.contains("No matches found"));
    }

    #[tokio::test]
    async fn type_filter_restricts_by_extension() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_grep(
            &ctx,
            "fn",
            json!({ "output_mode": "files_with_matches", "type": "rs" }),
        )
        .await;
        assert!(result.content.contains("foo.rs"));
        assert!(result.content.contains("baz.rs"));
        assert!(!result.content.contains("bar.txt"));
    }

    #[tokio::test]
    async fn context_lines_group_matches() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_grep(
            &ctx,
            "fn main",
            json!({ "output_mode": "content", "-C": 1 }),
        )
        .await;
        assert!(result.content.contains("foo.rs:1:fn main() {}"));
        assert!(result.content.contains("foo.rs:2:let x = 1;"));
    }

    #[tokio::test]
    async fn head_limit_and_offset_paginate() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let first = run_grep(
            &ctx,
            "fn",
            json!({ "output_mode": "files_with_matches", "head_limit": 1, "offset": 0 }),
        )
        .await;
        assert_eq!(
            first
                .content
                .lines()
                .filter(|l| !l.is_empty() && !l.starts_with("<system>"))
                .count(),
            1
        );
        assert!(first.content.contains("Results truncated"));

        let second = run_grep(
            &ctx,
            "fn",
            json!({ "output_mode": "files_with_matches", "head_limit": 1, "offset": 1 }),
        )
        .await;
        assert_eq!(
            second
                .content
                .lines()
                .filter(|l| !l.is_empty() && !l.starts_with("<system>"))
                .count(),
            1
        );
        assert_ne!(first.content, second.content);
    }

    #[tokio::test]
    async fn line_numbers_false_omits_numbers() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_grep(
            &ctx,
            "fn main",
            json!({ "output_mode": "content", "-n": false }),
        )
        .await;
        assert!(result.content.contains("foo.rs:fn main() {}"));
        assert!(!result.content.contains("foo.rs:1:fn main() {}"));
    }

    #[tokio::test]
    async fn invalid_regex_returns_error() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = GrepTool
            .execute(&ctx, json!({ "pattern": "[invalid" }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn count_summary_is_accurate() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_grep(&ctx, "fn", json!({ "output_mode": "count_matches" })).await;
        // foo.rs has 1, baz.rs has 1, sub/qux.rs has 1 => 3 total
        assert!(
            result
                .content
                .contains("Found 3 occurrences across 3 files")
        );
    }

    #[test]
    fn context_groups_merge_adjacent_matches() {
        let groups = build_context_groups(&[1, 3], 1, 1, 10);
        assert_eq!(groups, vec![(0, 4)]);
    }

    #[test]
    fn context_groups_separate_distant_matches() {
        let groups = build_context_groups(&[1, 5], 0, 0, 10);
        assert_eq!(groups, vec![(1, 1), (5, 5)]);
    }
}
