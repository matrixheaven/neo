use globset::GlobSetBuilder;
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, parse_input, schema};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GlobInput {
    #[schemars(
        description = "Glob pattern to match files and/or directories. Supports `*`, `**`, and brace expansion such as `*.{rs,toml}` or `{src,tests}/**/*.rs`."
    )]
    pattern: String,
    #[serde(default = "default_path")]
    #[schemars(
        description = "Directory to search in. Relative paths resolve against the working directory; paths outside the working directory must be absolute. Defaults to the current working directory."
    )]
    path: std::path::PathBuf,
    #[serde(default = "default_include_dirs")]
    #[schemars(
        description = "Whether to include directories in results. Defaults to true. Set false to return only files."
    )]
    include_dirs: bool,
    #[serde(default = "default_max_matches")]
    #[schemars(
        description = "Maximum number of matching paths to return. Defaults to 100. Lower this only when you need a quick peek; refine the pattern when the cap is hit."
    )]
    max_matches: usize,
}

fn default_path() -> std::path::PathBuf {
    ".".into()
}

const fn default_include_dirs() -> bool {
    true
}

const fn default_max_matches() -> usize {
    100
}

const MAX_STRUCTURED_MATCHES: usize = 100;

fn display_path(path: &Path, workspace: &Path) -> String {
    let relative = path.strip_prefix(workspace).unwrap_or(path);
    let display = relative.to_string_lossy();
    if display.is_empty() {
        ".".to_owned()
    } else {
        display.into_owned()
    }
}

pub struct GlobTool;

impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "Glob"
    }

    fn description(&self) -> &'static str {
        "Find files and optionally directories by glob pattern, sorted by modification time \
        (most recent first).\
        \
        Good patterns:\
        - `*.ts` — files in the current directory matching an extension\
        - `src/**/*.ts` — recursive walk with a subdirectory anchor and extension\
        - `**/*.py` — recursive walk from the search root for an extension\
        - `*.{ts,tsx}` — brace expansion is supported; expanded into `*.ts` and `*.tsx` before walking\
        - `{src,test}/**/*.ts` — cartesian brace expansion is supported too\
        \
        Results are capped at the first `max_matches` matching paths (walk order, not global \
        modification-time order). If a search returns more, a truncation marker is appended with \
        the count of matches seen so far. Refine the pattern (extension, subdirectory) when the cap \
        is hit, or call again with a narrower anchor.\
        \
        Large-directory caveat — avoid recursing into dependency / build output even with an anchor:\
        - `node_modules/**/*.js`, `.venv/**/*.py`, `__pycache__/**`, `target/**` all match \
          technically but typically produce thousands of results that truncate at the match cap and \
          waste the caller context. Prefer specific subpaths like `node_modules/react/src/**/*.js`.\
        \
        Parameters:\
        - pattern: Glob pattern to match files/directories.\
        - path: Directory to search in. Defaults to the current working directory.\
        - include_dirs: Whether to include directories in results. Defaults to true.\
        - max_matches: Maximum number of matching paths to return. Defaults to 100."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<GlobInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_file_read_allowed()?;
            let input: GlobInput = parse_input(self.name(), input)?;
            let walk_root = ctx.resolve_workspace_path(&input.path)?;
            let workspace = ctx.workspace_root().to_path_buf();
            let display_root = display_path(&walk_root, &workspace);

            // Brace-expand the pattern into individual sub-patterns.
            let sub_patterns = expand_braces(&input.pattern);
            let mut builder = GlobSetBuilder::new();
            for sub_pattern in &sub_patterns {
                let glob = globset::GlobBuilder::new(sub_pattern)
                    .literal_separator(true)
                    .build()
                    .map_err(|err| ToolError::InvalidInput {
                        tool: self.name().to_owned(),
                        message: format!("invalid glob pattern '{sub_pattern}': {err}"),
                    })?;
                builder.add(glob);
            }
            let glob_set = builder.build().map_err(|err| ToolError::InvalidInput {
                tool: self.name().to_owned(),
                message: format!("invalid glob pattern: {err}"),
            })?;

            let max_matches = input.max_matches;
            let include_dirs = input.include_dirs;
            let result = tokio::task::spawn_blocking(move || {
                let mut matches: Vec<(String, std::time::SystemTime)> = Vec::new();
                let mut total_matched: usize = 0;
                for entry in WalkBuilder::new(&walk_root).standard_filters(true).build() {
                    let Ok(entry) = entry else {
                        continue;
                    };
                    let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
                    if is_dir && !include_dirs {
                        continue;
                    }
                    // Match the path relative to the walk root so that the
                    // `path` parameter scopes the search naturally.
                    let relative = entry
                        .path()
                        .strip_prefix(&walk_root)
                        .unwrap_or(entry.path());
                    if !glob_set.is_match(relative) {
                        continue;
                    }
                    total_matched += 1;
                    // Display the path relative to the workspace root for
                    // consistency with grep / find.
                    let display = entry
                        .path()
                        .strip_prefix(&workspace)
                        .unwrap_or(entry.path());
                    let mtime = entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    let suffix = if is_dir { "/" } else { "" };
                    matches.push((format!("{}{suffix}", display.display()), mtime));
                }
                // Sort by modification time, most recent first.
                matches.sort_by_key(|b| std::cmp::Reverse(b.1));
                let truncated = matches.len() > max_matches;
                let paths: Vec<_> = matches
                    .into_iter()
                    .take(max_matches)
                    .map(|(p, _)| p)
                    .collect();
                Ok::<_, std::io::Error>((paths, total_matched, truncated))
            })
            .await
            .map_err(std::io::Error::other)??;

            let (paths, total_matched, truncated) = result;
            let returned = paths.len();
            let structured_matches: Vec<_> =
                paths.iter().take(MAX_STRUCTURED_MATCHES).cloned().collect();
            let details_truncated = structured_matches.len() < returned;
            let mut lines = paths;
            if truncated {
                lines.push(format!(
                    "[Truncated at {max_matches} matches — {total_matched} matched so far, use a more specific pattern]"
                ));
                lines.push(format!(
                    "Only the first {max_matches} matches are returned."
                ));
            } else if !lines.is_empty() {
                lines.push(format!("Found {} matches", lines.len()));
            }

            Ok(ToolResult::ok(lines.join("\n")).with_details(json!({
                "kind": "glob",
                "pattern": input.pattern,
                "path": display_root,
                "matches": structured_matches,
                "total_matched": total_matched,
                "returned": returned,
                "truncated": truncated,
                "details_truncated": details_truncated,
            })))
        })
    }
}

/// Expand brace alternatives in a glob pattern.
///
/// `*.{ts,tsx}` → `["*.ts", "*.tsx"]`
/// `{src,tests}/*.rs` → `["src/*.rs", "tests/*.rs"]`
fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    let Some(close_rel) = pattern[open..].find('}') else {
        // No closing brace — treat the `{` as a literal.
        return vec![pattern.to_string()];
    };
    let close = open + close_rel;
    let prefix = &pattern[..open];
    let group = &pattern[open + 1..close];
    let suffix = &pattern[close + 1..];

    let mut results = Vec::new();
    for option in group.split(',') {
        let expanded = format!("{prefix}{option}{suffix}");
        // Recurse to handle additional brace groups in prefix/suffix.
        results.extend(expand_braces(&expanded));
    }
    results
}

#[cfg(test)]
#[path = "test_cases/matching.rs"]
mod matching;

#[cfg(test)]
#[path = "test_cases/braces.rs"]
mod braces;
