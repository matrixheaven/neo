use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::AgentRole;

/// Tool policy for a subagent role. Determines how Bash and tool access are
/// enforced at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicy {
    /// Full coder access: bash, write, edit, read-only tools.
    FullCoder,
    /// Read-only shell: bash allowed but only read-only commands.
    ReadOnlyShell,
    /// No shell: no bash, no write/edit.
    NoShell,
    /// Orchestrator: only coordination tools, no direct bash/write/edit.
    Orchestrator,
}

/// A built-in profile that maps a role to its display label, prompt addendum,
/// allowed tools, and tool policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentProfile {
    pub role: AgentRole,
    pub display_label: &'static str,
    pub prompt_addendum: &'static str,
    pub allowed_tools: BTreeSet<&'static str>,
    pub tool_policy: ToolPolicy,
}

impl AgentProfile {
    #[must_use]
    pub fn for_role(role: AgentRole) -> Self {
        match role {
            AgentRole::Coder => coder_profile(),
            AgentRole::Explorer => explorer_profile(),
            AgentRole::Planner => planner_profile(),
            AgentRole::Reviewer => reviewer_profile(),
            AgentRole::Orchestrator => orchestrator_profile(),
        }
    }
}

fn set(items: &[&'static str]) -> BTreeSet<&'static str> {
    items.iter().copied().collect()
}

fn coder_profile() -> AgentProfile {
    AgentProfile {
        role: AgentRole::Coder,
        display_label: "Coder",
        prompt_addendum: "You are a Coder subagent. Implement bounded code changes requested by the parent agent. Return a compact technical summary. Do not ask the end user questions. Never mutate git state.",
        allowed_tools: set(&[
            "Read", "List", "Grep", "Find", "Glob", "Bash", "Write", "Edit", "TodoList",
        ]),
        tool_policy: ToolPolicy::FullCoder,
    }
}

fn explorer_profile() -> AgentProfile {
    AgentProfile {
        role: AgentRole::Explorer,
        display_label: "Explorer",
        prompt_addendum: "You are an Explorer subagent. Search, read, and analyze only. Prefer parallel read/search calls when independent. Report findings with file references and confidence. Do not repeat this setup text in your final summary.",
        allowed_tools: set(&["Read", "List", "Grep", "Find", "Glob", "Bash"]),
        tool_policy: ToolPolicy::ReadOnlyShell,
    }
}

fn planner_profile() -> AgentProfile {
    AgentProfile {
        role: AgentRole::Planner,
        display_label: "Planner",
        prompt_addendum: "You are a Planner subagent. Identify unknowns and produce step-by-step implementation plans. Recommend explorer subagents if more investigation is required. Do not run shell commands. Do not edit files. Do not repeat this setup text in your final summary.",
        allowed_tools: set(&["Read", "List", "Grep", "Find", "Glob"]),
        tool_policy: ToolPolicy::NoShell,
    }
}

fn reviewer_profile() -> AgentProfile {
    AgentProfile {
        role: AgentRole::Reviewer,
        display_label: "Reviewer",
        prompt_addendum: "You are a Reviewer subagent. Findings first, ordered by severity, with file and line references. Focus on bugs, regressions, missing tests, and risk. Do not edit files. Do not repeat this setup text in your final summary.",
        allowed_tools: set(&["Read", "List", "Grep", "Find", "Glob", "Bash"]),
        tool_policy: ToolPolicy::ReadOnlyShell,
    }
}

fn orchestrator_profile() -> AgentProfile {
    AgentProfile {
        role: AgentRole::Orchestrator,
        display_label: "Orchestrator",
        prompt_addendum: "You are an Orchestrator subagent. Break work into bounded subagent tasks, prefer foreground blocking unless background collaboration is useful, wait for foreground subagents, summarize results, and use resume for continuing old agents. Do not edit files directly. Do not run shell commands.",
        allowed_tools: set(&[
            "Delegate",
            "DelegateSwarm",
            "WaitDelegate",
            "ListDelegates",
            "MessageDelegate",
            "InterruptDelegate",
            "TaskOutput",
            "TaskStop",
            "RunWorkflow",
            "TodoList",
        ]),
        tool_policy: ToolPolicy::Orchestrator,
    }
}

/// Classify whether a shell command is read-only (safe for explorer/reviewer).
///
/// Conservative: if a command cannot be positively classified as read-only,
/// it is rejected.
#[must_use]
pub fn is_read_only_shell_command(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    if contains_unsupported_shell_syntax(trimmed) {
        return false;
    }
    let Some(tokens) = shlex::split(trimmed) else {
        return false;
    };
    let segments = split_shell_segments(&tokens);
    if segments.is_empty() {
        return false;
    }
    segments
        .iter()
        .all(|segment| is_read_only_command_segment(segment))
}

fn contains_unsupported_shell_syntax(command: &str) -> bool {
    command.contains('\n')
        || command.contains('\r')
        || command.contains('>')
        || command.contains('<')
        || command.contains('`')
        || command.contains("$(")
}

fn split_shell_segments(tokens: &[String]) -> Vec<&[String]> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        if matches!(token.as_str(), "|" | "&&" | "||" | ";") {
            if start < index {
                segments.push(&tokens[start..index]);
            }
            start = index + 1;
        }
    }
    if start < tokens.len() {
        segments.push(&tokens[start..]);
    }
    segments
}

fn is_read_only_command_segment(tokens: &[String]) -> bool {
    let Some(first) = tokens.first().map(String::as_str) else {
        return false;
    };
    match first {
        "ls" | "pwd" | "find" | "rg" | "grep" | "wc" | "head" | "tail" | "cat" | "tree"
        | "stat" | "file" | "du" | "df" | "sort" | "uniq" | "sed" => true,
        "git" => is_read_only_git_tokens(tokens),
        _ => false,
    }
}

fn is_read_only_git_tokens(tokens: &[String]) -> bool {
    let Some(subcommand) = tokens.get(1).map(String::as_str) else {
        return false;
    };
    match subcommand {
        "status" | "diff" | "log" | "show" | "blame" | "ls-files" | "rev-parse" => true,
        "branch" => branch_args_are_read_only(&tokens[2..]),
        _ => false,
    }
}

fn branch_args_are_read_only(args: &[String]) -> bool {
    args.is_empty()
        || args.iter().all(|arg| {
            matches!(
                arg.as_str(),
                "--show-current" | "--list" | "-a" | "-r" | "-v" | "-vv"
            )
        })
}

/// Classify whether a command is a git mutation command.
#[must_use]
pub fn is_git_mutation_command(command: &str) -> bool {
    shlex::split(command.trim()).is_some_and(|tokens| tokens_contain_git_mutation(&tokens))
}

fn tokens_contain_git_mutation(tokens: &[String]) -> bool {
    if tokens.is_empty() {
        return false;
    }
    if shell_wrapper_script(tokens).is_some_and(is_git_mutation_command) {
        return true;
    }
    tokens.iter().enumerate().any(|(index, token)| {
        token == "git"
            && tokens
                .get(index + 1)
                .is_some_and(|subcommand| git_subcommand_mutates(subcommand))
    })
}

fn shell_wrapper_script(tokens: &[String]) -> Option<&str> {
    let shell = tokens.first()?.as_str();
    if !matches!(shell, "bash" | "sh" | "zsh") {
        return None;
    }
    let command_index = tokens
        .iter()
        .position(|token| matches!(token.as_str(), "-c" | "-lc" | "-ic"))?;
    tokens.get(command_index + 1).map(String::as_str)
}

fn git_subcommand_mutates(subcommand: &str) -> bool {
    matches!(
        subcommand,
        "add"
            | "am"
            | "apply"
            | "branch"
            | "checkout"
            | "cherry-pick"
            | "clean"
            | "commit"
            | "filter-branch"
            | "gc"
            | "merge"
            | "mv"
            | "push"
            | "rebase"
            | "reflog"
            | "reset"
            | "restore"
            | "rm"
            | "stash"
            | "switch"
            | "tag"
            | "worktree"
    )
}
