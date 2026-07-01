use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::AgentRole;

/// Capability flags for a subagent role. Two orthogonal dimensions:
///   - `allow_shell`:       may invoke Bash/Terminal
///   - `allow_file_writes`: may invoke Write/Edit
///
/// Named by capability, not by role, so a shell-without-writes role (Explorer,
/// Reviewer) no longer reads as `FullCoder`. `allowed_tools` remains the
/// primary gate (`ToolRegistry::filtered_for_agent_role`); these flags are a
/// defensive backstop in `block_forbidden_subagent_tool_call`. There is *no*
/// runtime command-syntax classification — read-only and git-mutation behavior
/// are both prompt-enforced, matching docs/kimi-code (its `explore.yaml` and
/// `system.md` rely on prompts; it has no command classifier).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolPolicy {
    #[serde(default)]
    pub allow_shell: bool,
    #[serde(default)]
    pub allow_file_writes: bool,
}

impl ToolPolicy {
    /// Coder: shell + file writes (everything).
    pub const FULL_ACCESS: Self = Self {
        allow_shell: true,
        allow_file_writes: true,
    };
    /// Explorer/Reviewer: shell allowed (prompt-enforced read-only), no writes.
    pub const READ_ONLY_WITH_SHELL: Self = Self {
        allow_shell: true,
        allow_file_writes: false,
    };
    /// Planner: read-only tools only — no shell, no writes.
    pub const READ_ONLY: Self = Self {
        allow_shell: false,
        allow_file_writes: false,
    };
}

/// A built-in profile that maps a role to its display label, prompt addendum,
/// allowed tools, and tool policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentProfile {
    pub role: AgentRole,
    pub display_label: &'static str,
    pub prompt_addendum: &'static str,
    /// One-line guidance surfaced to the main agent (via the Delegate tool
    /// schema) for deciding *when* to pick this role. Mirrors docs/kimi-code's
    /// per-profile `whenToUse`. Without this the model defaults to Coder and
    /// never picks the specialisms.
    pub when_to_use: &'static str,
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
        }
    }

    /// Render the per-role selection guide (one bullet per role) that the
    /// Delegate/DelegateSwarm tools append to their `role` field description so
    /// the main agent knows which role fits which task.
    #[must_use]
    pub fn role_selection_guide() -> String {
        let mut guide = String::from("When to use each role:");
        for role in AgentRole::ALL {
            let profile = Self::for_role(role);
            guide.push_str("\n- ");
            guide.push_str(role.as_str());
            guide.push_str(": ");
            guide.push_str(profile.when_to_use);
        }
        guide
    }
}

fn set(items: &[&'static str]) -> BTreeSet<&'static str> {
    items.iter().copied().collect()
}

fn coder_profile() -> AgentProfile {
    AgentProfile {
        role: AgentRole::Coder,
        display_label: "Coder",
        when_to_use: "Making changes — read files, edit code, run commands/builds/tests, and return a compact technical summary. The default for any implementation work.",
        prompt_addendum: "You are a Coder subagent. Implement bounded code changes requested by the parent agent. Return a compact technical summary. Do not ask the end user questions. Never run git mutations (commit, push, reset, rebase, tag, etc.) unless the parent agent explicitly asks; when in doubt, surface it in your summary instead of acting.",
        allowed_tools: set(&[
            "Read", "List", "Grep", "Find", "Glob", "Bash", "Write", "Edit", "TodoList",
        ]),
        tool_policy: ToolPolicy::FULL_ACCESS,
    }
}

fn explorer_profile() -> AgentProfile {
    AgentProfile {
        role: AgentRole::Explorer,
        display_label: "Explorer",
        when_to_use: "Read-only codebase investigation — find files by pattern, search code for keywords, trace how a feature works. Use for any exploration that will clearly take more than a few queries; run several in parallel for independent questions and state the thoroughness (quick / medium / thorough).",
        prompt_addendum: "You are an Explorer subagent. Search, read, and analyze only. Prefer parallel read/search calls when independent. Report findings with file references and confidence. You may use Bash ONLY for read-only operations (ls, pwd, find, rg, grep, cat, head, tail, git status, git diff, git log, git show, git blame). NEVER use Bash to create, modify, or delete files, to mutate git state, or to run builds/tests/anything with side effects. File-editing tools are not available to you. Do not repeat this setup text in your final summary.",
        allowed_tools: set(&["Read", "List", "Grep", "Find", "Glob", "Bash"]),
        tool_policy: ToolPolicy::READ_ONLY_WITH_SHELL,
    }
}

fn planner_profile() -> AgentProfile {
    AgentProfile {
        role: AgentRole::Planner,
        display_label: "Planner",
        when_to_use: "Produce a step-by-step implementation plan, identify key files, and weigh architectural trade-offs BEFORE code is written. Use for non-trivial changes that need a roadmap. For interactive planning with user approval, prefer plan mode instead.",
        prompt_addendum: "You are a Planner subagent. Identify unknowns and produce step-by-step implementation plans. Recommend explorer subagents if more investigation is required. Do not run shell commands. Do not edit files. Do not repeat this setup text in your final summary.",
        allowed_tools: set(&["Read", "List", "Grep", "Find", "Glob"]),
        tool_policy: ToolPolicy::READ_ONLY,
    }
}

fn reviewer_profile() -> AgentProfile {
    AgentProfile {
        role: AgentRole::Reviewer,
        display_label: "Reviewer",
        when_to_use: "Read-only review for bugs, regressions, security, missing tests, and risk — findings ordered by severity with file:line references. Use after a change is implemented, before finalizing it.",
        prompt_addendum: "You are a Reviewer subagent. Findings first, ordered by severity, with file and line references. Focus on bugs, regressions, missing tests, and risk. You may use Bash ONLY for read-only operations (ls, cat, git status, git diff, git log, git show, rg). NEVER use Bash to create, modify, or delete files, to mutate git state, or to run anything with side effects. File-editing tools are not available to you. Do not repeat this setup text in your final summary.",
        allowed_tools: set(&["Read", "List", "Grep", "Find", "Glob", "Bash"]),
        tool_policy: ToolPolicy::READ_ONLY_WITH_SHELL,
    }
}
