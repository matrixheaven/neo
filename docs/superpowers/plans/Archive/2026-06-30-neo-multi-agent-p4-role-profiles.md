# Neo Multi-Agent P4 Role Profiles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make multi-agent roles enforceable runtime profiles with real tool whitelists, read-only Bash policy, and role-specific prompt addenda.

**Architecture:** Add a `profile` module under `multi_agent` that maps `AgentRole` to an `AgentProfile` containing display label, prompt addendum, and `ToolPolicy`. Child runtimes receive a role-filtered `ToolRegistry`; Bash additionally checks structured read-only command classification for explorer and reviewer roles. Prompt text reinforces policy but does not replace enforcement.

**Tech Stack:** Rust 2024, `serde`, `schemars`, `ToolRegistry`, `AgentRuntime`, shell command parsing by conservative token classifier, `cargo nextest run`.

---

## Source Spec

Use `/Users/chenyuanhao/Workspace/neo/docs/superpowers/specs/2026-06-30-neo-multi-agent-hardening-design.md`.

This plan covers:

- Section 16 Role Profiles.
- Section 17 Tool Policy Enforcement.
- Section 18 Prompt and Summary Hygiene.
- Acceptance criteria under Roles.

P1 must be complete first. P2 and P3 can run before or after this plan if compile conflicts are resolved by applying the newest type definitions from those plans.

## Constraints

- Start implementation with `icm recall-context "Neo multi-agent P4 role profiles tool policy" --limit 5`.
- Use CodeGraph before grep/read for symbol discovery in this repo.
- Do not run bare `cargo test`; use `cargo nextest run ...`.
- Do not mutate git unless the user explicitly authorizes that exact command.
- Do not keep any `harness` role alias or parser fallback.
- Do not rely on prompt-only safety for tool restrictions.
- Do not allow subagents to mutate git through Bash.

## Kimi Reference To Check Before Coding

Read these reference files directly from the vendored docs before implementation:

- `docs/kimi-code/docs/en/customization/agents.md`
- `docs/kimi-code/packages/agent-core/src/profile/default/explorer.yaml`
- `docs/kimi-code/packages/agent-core/src/profile/default/coder.yaml`
- `docs/kimi-code/packages/agent-core/src/profile/default/reviewer.yaml`

Purpose: copy the product shape, not the implementation. Neo roles remain Rust-native and local-only.

## Current Code Touchpoints

- `crates/neo-agent-core/src/multi_agent/identity.rs`
  - `AgentRole` currently has `Coder`, `Explorer`, `Planner`, `Reviewer`, `Orchestrator`.
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
  - `run_agent_snapshot` creates child `AgentRuntime` using shared tools.
  - `child_config` injects child system prompt text.
  - Existing git mutation guard exists for subagent Bash.
- `crates/neo-agent-core/src/tools/mod.rs`
  - `ToolRegistry` owns specs and tool execution.
- `crates/neo-agent-core/src/tools/bash.rs` or shell runner modules
  - Bash command execution and permissions.
- `crates/neo-agent-core/tests/multi_agent_runtime.rs`
  - Existing child-tool tests.

## File Structure

Create:

- `crates/neo-agent-core/src/multi_agent/profile.rs`

Modify:

- `crates/neo-agent-core/src/multi_agent/mod.rs`
- `crates/neo-agent-core/src/multi_agent/identity.rs`
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
- `crates/neo-agent-core/src/tools/mod.rs`
- `crates/neo-agent-core/src/tools/bash.rs`
- `crates/neo-agent-core/tests/multi_agent_runtime.rs`
- `crates/neo-agent-core/tests/multi_agent_roles.rs`

Create `crates/neo-agent-core/tests/multi_agent_roles.rs` in Task 1.

## Desired End State

- Built-in `AgentProfile` exists for coder, explorer, planner, reviewer, orchestrator.
- Profile names and display labels are deterministic and not LLM-generated.
- Child tool specs match role policy.
- Explorer and reviewer can run read-only Bash only.
- Planner has no Bash and no write/edit.
- Orchestrator can coordinate through multi-agent tools but cannot directly edit or run Bash.
- Coder can write/edit but cannot mutate git.
- Summary context does not leak profile setup boilerplate.

## Task 1: Add Built-In AgentProfile Model

**Files:**

- Create: `crates/neo-agent-core/src/multi_agent/profile.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/mod.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_roles.rs`

- [ ] **Step 1: Add failing profile test**

Create `crates/neo-agent-core/tests/multi_agent_roles.rs`:

```rust
use neo_agent_core::multi_agent::{AgentProfile, AgentRole, ToolPolicy};

#[test]
fn built_in_profiles_have_expected_labels_and_tool_policies() {
    let explorer = AgentProfile::for_role(AgentRole::Explorer);
    assert_eq!(explorer.display_label, "Explorer");
    assert_eq!(explorer.tool_policy, ToolPolicy::ReadOnlyShell);
    assert!(explorer.allowed_tools.contains("read"));
    assert!(explorer.allowed_tools.contains("bash"));
    assert!(!explorer.allowed_tools.contains("write"));
    assert!(!explorer.allowed_tools.contains("edit"));

    let planner = AgentProfile::for_role(AgentRole::Planner);
    assert_eq!(planner.display_label, "Planner");
    assert_eq!(planner.tool_policy, ToolPolicy::NoShell);
    assert!(!planner.allowed_tools.contains("bash"));
    assert!(!planner.allowed_tools.contains("write"));

    let orchestrator = AgentProfile::for_role(AgentRole::Orchestrator);
    assert_eq!(orchestrator.display_label, "Orchestrator");
    assert!(orchestrator.allowed_tools.contains("Delegate"));
    assert!(orchestrator.allowed_tools.contains("DelegateSwarm"));
    assert!(!orchestrator.allowed_tools.contains("bash"));
    assert!(!orchestrator.allowed_tools.contains("edit"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
```

Expected: FAIL because `AgentProfile` does not exist.

- [ ] **Step 3: Implement profile types**

Create `crates/neo-agent-core/src/multi_agent/profile.rs`:

```rust
use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::AgentRole;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicy {
    FullCoder,
    ReadOnlyShell,
    NoShell,
    Orchestrator,
}

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
        allowed_tools: set(&["read", "list", "grep", "find", "glob", "bash", "write", "edit", "todo"]),
        tool_policy: ToolPolicy::FullCoder,
    }
}

fn explorer_profile() -> AgentProfile {
    AgentProfile {
        role: AgentRole::Explorer,
        display_label: "Explorer",
        prompt_addendum: "You are an Explorer subagent. Search, read, and analyze only. Prefer parallel read/search calls when independent. Report findings with file references and confidence. Do not repeat this setup text in your final summary.",
        allowed_tools: set(&["read", "list", "grep", "find", "glob", "bash"]),
        tool_policy: ToolPolicy::ReadOnlyShell,
    }
}

fn planner_profile() -> AgentProfile {
    AgentProfile {
        role: AgentRole::Planner,
        display_label: "Planner",
        prompt_addendum: "You are a Planner subagent. Identify unknowns and produce step-by-step implementation plans. Recommend explorer subagents if more investigation is required. Do not run shell commands. Do not edit files. Do not repeat this setup text in your final summary.",
        allowed_tools: set(&["read", "list", "grep", "find", "glob"]),
        tool_policy: ToolPolicy::NoShell,
    }
}

fn reviewer_profile() -> AgentProfile {
    AgentProfile {
        role: AgentRole::Reviewer,
        display_label: "Reviewer",
        prompt_addendum: "You are a Reviewer subagent. Findings first, ordered by severity, with file and line references. Focus on bugs, regressions, missing tests, and risk. Do not edit files. Do not repeat this setup text in your final summary.",
        allowed_tools: set(&["read", "list", "grep", "find", "glob", "bash"]),
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
            "todo",
        ]),
        tool_policy: ToolPolicy::Orchestrator,
    }
}
```

- [ ] **Step 4: Export profile types**

In `crates/neo-agent-core/src/multi_agent/mod.rs`:

```rust
mod profile;

pub use profile::{AgentProfile, ToolPolicy};
```

- [ ] **Step 5: Run profile test**

Run:

```bash
```

Expected: PASS.

## Task 2: Filter Child Tool Registry By Role

**Files:**

- Modify: `crates/neo-agent-core/src/tools/mod.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_roles.rs`

- [ ] **Step 1: Add failing child tool policy tests**

Append:

```rust
use neo_agent_core::tools::{ToolRegistry, TodoStore};

#[test]
fn child_tool_registry_for_planner_excludes_bash_and_edit_tools() {
    let registry = ToolRegistry::with_builtin_tools_and_todos(TodoStore::default());
    let filtered = registry.filtered_for_agent_role(AgentRole::Planner);
    let names = filtered.specs().into_iter().map(|spec| spec.name).collect::<Vec<_>>();

    assert!(names.contains(&"read".to_owned()));
    assert!(!names.contains(&"bash".to_owned()));
    assert!(!names.contains(&"write".to_owned()));
    assert!(!names.contains(&"edit".to_owned()));
}

#[test]
fn child_tool_registry_for_orchestrator_contains_coordination_tools_only() {
    let registry = ToolRegistry::with_builtin_tools_and_todos(TodoStore::default());
    let filtered = registry.filtered_for_agent_role(AgentRole::Orchestrator);
    let names = filtered.specs().into_iter().map(|spec| spec.name).collect::<Vec<_>>();

    assert!(names.contains(&"Delegate".to_owned()));
    assert!(names.contains(&"DelegateSwarm".to_owned()));
    assert!(names.contains(&"WaitDelegate".to_owned()));
    assert!(!names.contains(&"bash".to_owned()));
    assert!(!names.contains(&"write".to_owned()));
    assert!(!names.contains(&"edit".to_owned()));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
```

Expected: FAIL because filtered registry does not exist.

- [ ] **Step 3: Add registry filter method**

In `crates/neo-agent-core/src/tools/mod.rs`, add:

```rust
impl ToolRegistry {
    #[must_use]
    pub fn filtered_for_agent_role(&self, role: crate::multi_agent::AgentRole) -> Self {
        let profile = crate::multi_agent::AgentProfile::for_role(role);
        let mut filtered = Self::default();
        for (name, tool) in &self.tools {
            if profile.allowed_tools.contains(name.as_str()) {
                filtered.tools.insert(name.clone(), Arc::clone(tool));
            }
        }
        filtered
    }
}
```

`ToolRegistry` stores tools in a private `BTreeMap<String, Box<dyn Tool>>`, so implement this method inside `crates/neo-agent-core/src/tools/mod.rs`. Change the registry storage to `BTreeMap<String, Arc<dyn Tool>>`, update `register` to store `Arc::new(tool)`, and keep `specs()` and `run()` behavior unchanged.

- [ ] **Step 4: Use filtered tools for child runtime**

In `crates/neo-agent-core/src/multi_agent/runtime.rs`, where child `ChildRuntimeDeps` are built for a snapshot, replace the shared parent tools with role-filtered tools:

```rust
let child_tools = Arc::new(deps.tools.filtered_for_agent_role(snapshot.role));
let child_runtime = AgentRuntime::with_shared_tools_and_configured_specs(
    child_config(deps.config, snapshot.role),
    deps.model,
    child_tools,
)
.with_steer_input(steer_input);
```

`ChildRuntimeDeps.tools` is currently `Arc<ToolRegistry>`, so call:

```rust
let child_tools = Arc::new(deps.tools.as_ref().filtered_for_agent_role(snapshot.role));
```

Update `child_config` signature to accept `role: AgentRole`.

- [ ] **Step 5: Run child registry tests**

Run:

```bash
```

Expected: PASS.

## Task 3: Enforce Read-Only Bash For Explorer And Reviewer

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/profile.rs`
- Modify: `crates/neo-agent-core/src/tools/bash.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_roles.rs`

- [ ] **Step 1: Add read-only classifier tests**

Append:

```rust
use neo_agent_core::multi_agent::is_read_only_shell_command;

#[test]
fn read_only_shell_classifier_allows_known_read_commands() {
    assert!(is_read_only_shell_command("ls crates"));
    assert!(is_read_only_shell_command("find crates -name '*.rs'"));
    assert!(is_read_only_shell_command("rg Delegate crates/neo-agent-core/src"));
    assert!(is_read_only_shell_command("git status --short"));
    assert!(is_read_only_shell_command("git diff -- crates/neo-agent-core/src/tools/delegate.rs"));
    assert!(is_read_only_shell_command("git log -1 --oneline"));
    assert!(is_read_only_shell_command("git blame crates/neo-agent-core/src/lib.rs"));
}

#[test]
fn read_only_shell_classifier_rejects_mutating_commands() {
    assert!(!is_read_only_shell_command("git add ."));
    assert!(!is_read_only_shell_command("git commit -m change"));
    assert!(!is_read_only_shell_command("git checkout -- crates/neo-agent-core/src/lib.rs"));
    assert!(!is_read_only_shell_command("rm -rf target/tmp"));
    assert!(!is_read_only_shell_command("python - <<'PY'\nopen('x','w').write('x')\nPY"));
    assert!(!is_read_only_shell_command("cargo fmt"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
```

Expected: FAIL because classifier does not exist.

- [ ] **Step 3: Implement conservative classifier**

In `profile.rs`, add:

```rust
#[must_use]
pub fn is_read_only_shell_command(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    if contains_shell_mutation_operator(trimmed) {
        return false;
    }
    let first = trimmed.split_whitespace().next().unwrap_or_default();
    match first {
        "ls" | "pwd" | "find" | "rg" | "grep" | "wc" | "sed" | "awk" | "head" | "tail" | "cat" => true,
        "git" => is_read_only_git_command(trimmed),
        _ => false,
    }
}

fn contains_shell_mutation_operator(command: &str) -> bool {
    [" > ", ">>", "| tee", "&& rm", "; rm", "&& mv", "; mv", "&& cp", "; cp"]
        .iter()
        .any(|needle| command.contains(needle))
}

fn is_read_only_git_command(command: &str) -> bool {
    let mut parts = command.split_whitespace();
    if parts.next() != Some("git") {
        return false;
    }
    matches!(
        parts.next(),
        Some("status" | "diff" | "log" | "show" | "blame" | "branch" | "ls-files" | "rev-parse")
    )
}
```

Export `is_read_only_shell_command` from `multi_agent/mod.rs`.

- [ ] **Step 4: Attach role policy to Bash context**

Add a field to `ToolContext`:

```rust
pub child_agent_role: Option<crate::multi_agent::AgentRole>,
```

Default it to `None` in every `ToolContext` constructor. When building child runtime tool context, set it to `Some(snapshot.role)`.

In Bash tool execution, before permission checks:

```rust
if let Some(role) = ctx.child_agent_role {
    let profile = crate::multi_agent::AgentProfile::for_role(role);
    if profile.tool_policy == crate::multi_agent::ToolPolicy::ReadOnlyShell
        && !crate::multi_agent::is_read_only_shell_command(&input.command)
    {
        return Ok(ToolResult::error(format!(
            "{} agents may only run read-only shell commands",
            profile.display_label
        )));
    }
    if profile.tool_policy == crate::multi_agent::ToolPolicy::NoShell
        || profile.tool_policy == crate::multi_agent::ToolPolicy::Orchestrator
    {
        return Ok(ToolResult::error(format!(
            "{} agents may not run shell commands",
            profile.display_label
        )));
    }
}
```

- [ ] **Step 5: Run classifier tests**

Run:

```bash
```

Expected: PASS.

## Task 4: Deny Git Mutation For All Subagent Bash

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/profile.rs`
- Modify: `crates/neo-agent-core/src/tools/bash.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Add coder git mutation denial test**

Append to `crates/neo-agent-core/tests/multi_agent_runtime.rs`:

```rust
#[tokio::test]
async fn coder_subagent_bash_still_denies_git_mutation() {
    let (registry, ctx) = registry_with_multi_agent();
    let result = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "Run `git add .` and report the result",
                "role": "coder",
                "mode": "foreground"
            }),
        )
        .await
        .expect("delegate should return result");

    assert!(
        result.content.contains("git mutation") || result.content.contains("Never mutate git"),
        "{}",
        result.content
    );
}
```

- [ ] **Step 2: Run test**

Run:

```bash
```

Expected: PASS if existing guard already works; FAIL if role filtering bypassed it.

- [ ] **Step 3: Centralize git mutation classifier**

In `profile.rs`, add:

```rust
#[must_use]
pub fn is_git_mutation_command(command: &str) -> bool {
    let trimmed = command.trim();
    let mut parts = trimmed.split_whitespace();
    if parts.next() != Some("git") {
        return false;
    }
    matches!(
        parts.next(),
        Some(
            "add"
                | "commit"
                | "checkout"
                | "restore"
                | "reset"
                | "stash"
                | "rebase"
                | "clean"
                | "rm"
                | "push"
                | "merge"
                | "cherry-pick"
                | "tag"
                | "worktree"
                | "filter-branch"
                | "gc"
                | "reflog"
        )
    )
}
```

In Bash execution, before allowing any subagent command:

```rust
if ctx.child_agent_role.is_some()
    && crate::multi_agent::is_git_mutation_command(&input.command)
{
    return Ok(ToolResult::error(
        "subagents may not mutate git state".to_owned(),
    ));
}
```

- [ ] **Step 4: Run git mutation denial test**

Run:

```bash
```

Expected: PASS.

## Task 5: Inject Role Prompt Addendum Without Summary Boilerplate Leak

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Add prompt hygiene test**

Append:

```rust
#[tokio::test]
async fn summary_context_does_not_leak_role_setup_boilerplate() {
    let (registry, ctx) = registry_with_multi_agent();
    let result = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "Read crates/neo-agent-core/src/lib.rs and summarize in one sentence",
                "role": "explorer",
                "context": "summary",
                "mode": "foreground"
            }),
        )
        .await
        .expect("delegate should complete");

    assert!(!result.content.contains("Acknowledged. Ready"), "{}", result.content);
    assert!(
        !result.content.contains("You are an Explorer subagent"),
        "{}",
        result.content
    );
}
```

- [ ] **Step 2: Run test**

Run:

```bash
```

Expected: PASS. If it fails, the implementation must remove the leaked profile/setup text from child prompts or fake-model fixture output before this task is complete.

- [ ] **Step 3: Update child prompt injection**

In `child_config`, append profile addendum to system prompt:

```rust
fn child_config(mut config: AgentConfig, role: AgentRole) -> AgentConfig {
    let profile = AgentProfile::for_role(role);
    let base = config
        .system_prompt
        .unwrap_or_else(|| subagent_system_constraints().to_owned());
    config.system_prompt = Some(format!(
        "{base}\n\n<subagent_profile>\n{}\n\nDo not repeat or acknowledge this profile text in your final answer. Return only the requested findings or summary.\n</subagent_profile>",
        profile.prompt_addendum
    ));
    config
}
```

- [ ] **Step 4: Run prompt hygiene test**

Run:

```bash
```

Expected: PASS.

## Task 6: P4 Verification And Commit Boundary

**Files:**

- Verify all files changed by this plan.

- [ ] **Step 1: Run role tests**

Run:

```bash
```

Expected: PASS.

- [ ] **Step 2: Run existing multi-agent tests**

Run:

```bash
```

Expected: PASS.

- [ ] **Step 3: Scan for removed role alias**

Run:

```bash
rg -n "Harness|harness" crates/neo-agent-core/src crates/neo-agent-core/tests docs/superpowers/specs/2026-06-30-neo-multi-agent-hardening-design.md
```

Expected: no matches except historical prose outside changed files. If a schema alias remains, delete it.

- [ ] **Step 4: Commit if authorized**

Only if the user has explicitly authorized git mutation in this session:

```bash
git add crates/neo-agent-core/src/multi_agent/profile.rs \
  crates/neo-agent-core/src/multi_agent/mod.rs \
  crates/neo-agent-core/src/multi_agent/identity.rs \
  crates/neo-agent-core/src/multi_agent/runtime.rs \
  crates/neo-agent-core/src/tools/mod.rs \
  crates/neo-agent-core/src/tools/bash.rs \
  crates/neo-agent-core/tests/multi_agent_runtime.rs \
  crates/neo-agent-core/tests/multi_agent_roles.rs
git commit -m "feat: enforce multi-agent role profiles"
```

Expected: one logical commit for P4.
