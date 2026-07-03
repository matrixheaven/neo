# Multi-Agent Persistence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move Neo sessions to a durable agent layout with `state.json`, `agents/main/wire.jsonl`, per-subagent `wire.jsonl`, agent-scoped tasks, transcript replay, and a one-shot migration script.

**Architecture:** Add a session layout/state layer in `neo-agent-core`, then cut all main-session readers/writers from `transcript.jsonl` to `agents/main/wire.jsonl`. Restore parent-level Delegate/DelegateSwarm snapshots into both TUI transcript state and `MultiAgentRuntime`, then persist child runtime events into each subagent wire and replay those wires when resuming a subagent.

**Tech Stack:** Rust 2024, Tokio async file IO, serde JSON/JSONL, existing Neo `AgentEvent` wire format, Python 3 standard library for the migration script, cargo-nextest for narrow verification.

---

## Scope Check

The spec touches session storage, migration, runtime replay, multi-agent execution, TUI transcript replay, and background task persistence. These are not independent products: the new path API and `state.json` registry must land before child wires, migration, and replay can be correct. Keep this as one plan, implemented in sequential tasks with narrow tests after each boundary.

## File Structure

- Create `crates/neo-agent-core/src/session/layout.rs`
  - Owns constants and path helpers for `state.json`, `agents/`, `agents/<id>/wire.jsonl`, and `agents/<id>/tasks/`.
- Create `crates/neo-agent-core/src/session/agent_state.rs`
  - Owns `SessionState`, `SessionAgentRecord`, `SessionAgentKind`, read/write helpers, and main/subagent registration.
- Modify `crates/neo-agent-core/src/session/mod.rs`
  - Exports layout/state modules and updates `SessionMetadataStore` existence/list/fork behavior to use the main agent wire.
- Modify `crates/neo-agent/src/modes/run/session_mgmt.rs`
  - Creates new session directories with `state.json` and `agents/main/wire.jsonl`; resolves latest sessions from the main wire.
- Modify `crates/neo-agent/src/modes/run/mod.rs`
  - Opens/appends main wire and passes session directory into runtime config.
- Modify `crates/neo-agent/src/modes/sessions.rs`
  - Resolves session refs to main wire, exports/compacts from main wire, and stops accepting old `transcript.jsonl` paths as active runtime paths.
- Modify `crates/neo-agent/src/modes/interactive/controller_factory.rs`
  - Creates interactive sessions using the new main wire helper.
- Modify `crates/neo-agent/src/modes/interactive/shell_command.rs`
  - Persists shell events to the main wire.
- Modify `crates/neo-agent/src/modes/interactive/mod.rs`
  - Loads replay events, restores transcript cards, and restores `MultiAgentRuntime` state on resume.
- Modify `crates/neo-agent/src/rpc/server.rs`
  - Replaces hardcoded `transcript.jsonl` checks with main wire helpers.
- Modify `crates/neo-agent-core/src/tools/sessions.rs`
  - Summarizes sessions from `agents/main/wire.jsonl`.
- Modify `crates/neo-agent-core/src/runtime/config.rs`
  - Carries optional session directory and session agent state handles needed by child runtimes and task persistence.
- Modify `crates/neo-agent-core/src/tools/mod.rs`
  - Adds optional session directory / agent id context to `ToolContext`.
- Modify `crates/neo-agent-core/src/multi_agent/runtime.rs`
  - Restores delegate/swarm snapshots from replay, writes child events to child wires, and replays child wires for `Delegate(resume=...)`.
- Modify `crates/neo-agent-core/src/tools/delegate.rs`
  - Registers new subagents in `state.json`, passes child wire paths into the multi-agent runtime, and marks stale background work lost after resume.
- Modify `crates/neo-agent-core/src/tools/background_tasks.rs`
  - Adds an optional persistent task store rooted at an agent record directory.
- Create `scripts/migrate_sessions_to_agent_layout.py`
  - Migrates old sessions to the new layout with dry-run, backup, and apply modes.
- Modify focused tests:
  - `crates/neo-agent-core/tests/session_jsonl.rs`
  - `crates/neo-agent-core/tests/session_tree.rs`
  - `crates/neo-agent-core/tests/multi_agent_runtime.rs`
  - `crates/neo-agent/tests/cli_commands.rs`
  - `crates/neo-agent/tests/rpc_mode.rs`
  - `crates/neo-agent/src/modes/interactive/tests.rs`
  - `crates/neo-tui/tests/multi_agent_transcript.rs`

## Git Policy

Neo's current project policy forbids git mutations without explicit per-instance user authorization. Each task includes commit commands for the execution worker, but the worker must ask for and receive explicit authorization before running `git add` or `git commit`.

### Task 1: Add Session Layout And State Store

**Files:**
- Create: `crates/neo-agent-core/src/session/layout.rs`
- Create: `crates/neo-agent-core/src/session/agent_state.rs`
- Modify: `crates/neo-agent-core/src/session/mod.rs`
- Test: `crates/neo-agent-core/tests/session_jsonl.rs`

- [ ] **Step 1: Write failing layout/state tests**

Append these tests to `crates/neo-agent-core/tests/session_jsonl.rs`:

```rust
use neo_agent_core::session::{
    SessionAgentKind, SessionAgentRecord, SessionState, SessionStateStore,
    agent_tasks_dir, agent_wire_path, agents_dir, main_agent_wire_path,
    session_state_path,
};

#[test]
fn session_layout_paths_are_agent_scoped() {
    let session_dir = std::path::Path::new("/tmp/neo-session");

    assert_eq!(session_state_path(session_dir), session_dir.join("state.json"));
    assert_eq!(agents_dir(session_dir), session_dir.join("agents"));
    assert_eq!(
        main_agent_wire_path(session_dir),
        session_dir.join("agents").join("main").join("wire.jsonl")
    );
    assert_eq!(
        agent_wire_path(session_dir, "agent_abc"),
        session_dir.join("agents").join("agent_abc").join("wire.jsonl")
    );
    assert_eq!(
        agent_tasks_dir(session_dir, "agent_abc"),
        session_dir.join("agents").join("agent_abc").join("tasks")
    );
}

#[tokio::test]
async fn session_state_store_round_trips_main_and_subagent_records() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SessionStateStore::new(temp.path());
    let mut state = SessionState::new();
    state.ensure_main_agent();
    state.upsert_agent(SessionAgentRecord {
        kind: SessionAgentKind::Sub,
        record_dir: std::path::PathBuf::from("agents/agent_abc"),
        parent_agent_id: Some("main".to_owned()),
        role: Some("coder".to_owned()),
        swarm_id: Some("swarm_1".to_owned()),
        swarm_item: Some("crate-a".to_owned()),
    });

    store.write(&state).await.expect("write state");
    let loaded = store.read().await.expect("read state");

    assert_eq!(loaded.schema_version, 1);
    assert_eq!(
        loaded.agents.get("main").expect("main").record_dir,
        std::path::PathBuf::from("agents/main")
    );
    assert_eq!(
        loaded.agents.get("agent_abc").expect("child").parent_agent_id.as_deref(),
        Some("main")
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo nextest run -p neo-agent-core --test session_jsonl session_layout_paths_are_agent_scoped
```

Expected: FAIL because `SessionAgentKind`, `SessionAgentRecord`, `SessionState`, `SessionStateStore`, and the layout helpers are not exported yet.

- [ ] **Step 3: Implement layout helpers**

Create `crates/neo-agent-core/src/session/layout.rs`:

```rust
use std::path::{Path, PathBuf};

pub const MAIN_AGENT_ID: &str = "main";
pub const SESSION_STATE_FILE: &str = "state.json";
pub const AGENTS_DIR: &str = "agents";
pub const WIRE_FILE: &str = "wire.jsonl";
pub const TASKS_DIR: &str = "tasks";

#[must_use]
pub fn session_state_path(session_dir: &Path) -> PathBuf {
    session_dir.join(SESSION_STATE_FILE)
}

#[must_use]
pub fn agents_dir(session_dir: &Path) -> PathBuf {
    session_dir.join(AGENTS_DIR)
}

#[must_use]
pub fn agent_record_dir(session_dir: &Path, agent_id: &str) -> PathBuf {
    agents_dir(session_dir).join(agent_id)
}

#[must_use]
pub fn agent_wire_path(session_dir: &Path, agent_id: &str) -> PathBuf {
    agent_record_dir(session_dir, agent_id).join(WIRE_FILE)
}

#[must_use]
pub fn main_agent_wire_path(session_dir: &Path) -> PathBuf {
    agent_wire_path(session_dir, MAIN_AGENT_ID)
}

#[must_use]
pub fn agent_tasks_dir(session_dir: &Path, agent_id: &str) -> PathBuf {
    agent_record_dir(session_dir, agent_id).join(TASKS_DIR)
}

#[must_use]
pub fn relative_agent_record_dir(agent_id: &str) -> PathBuf {
    PathBuf::from(AGENTS_DIR).join(agent_id)
}
```

- [ ] **Step 4: Implement session state store**

Create `crates/neo-agent-core/src/session/agent_state.rs`:

```rust
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tokio::fs;

use super::{
    SessionError,
    layout::{MAIN_AGENT_ID, relative_agent_record_dir, session_state_path},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionAgentKind {
    Main,
    Sub,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAgentRecord {
    pub kind: SessionAgentKind,
    pub record_dir: PathBuf,
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm_item: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    pub schema_version: u32,
    pub agents: BTreeMap<String, SessionAgentRecord>,
}

impl SessionState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema_version: 1,
            agents: BTreeMap::new(),
        }
    }

    pub fn ensure_main_agent(&mut self) {
        self.agents
            .entry(MAIN_AGENT_ID.to_owned())
            .or_insert_with(|| SessionAgentRecord {
                kind: SessionAgentKind::Main,
                record_dir: relative_agent_record_dir(MAIN_AGENT_ID),
                parent_agent_id: None,
                role: None,
                swarm_id: None,
                swarm_item: None,
            });
    }

    pub fn upsert_agent(&mut self, record: SessionAgentRecord) {
        let id = record
            .record_dir
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .expect("agent record dir has final component")
            .to_owned();
        self.agents.insert(id, record);
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct SessionStateStore {
    session_dir: PathBuf,
}

impl SessionStateStore {
    #[must_use]
    pub fn new(session_dir: impl Into<PathBuf>) -> Self {
        Self {
            session_dir: session_dir.into(),
        }
    }

    pub async fn read(&self) -> Result<SessionState, SessionError> {
        let path = session_state_path(&self.session_dir);
        let content = fs::read_to_string(path).await?;
        serde_json::from_str(&content).map_err(|source| SessionError::Json { line: 0, source })
    }

    pub async fn write(&self, state: &SessionState) -> Result<(), SessionError> {
        fs::create_dir_all(&self.session_dir).await?;
        let path = session_state_path(&self.session_dir);
        let content = serde_json::to_string_pretty(state)
            .map_err(|source| SessionError::Json { line: 0, source })?;
        fs::write(path, format!("{content}\n")).await?;
        Ok(())
    }
}
```

- [ ] **Step 5: Export the new modules**

Modify `crates/neo-agent-core/src/session/mod.rs` near the existing module declarations:

```rust
pub mod agent_state;
pub mod index;
pub mod layout;
pub mod workspace;

pub use agent_state::{SessionAgentKind, SessionAgentRecord, SessionState, SessionStateStore};
pub use index::{SessionIndex, SessionIndexEntry, SessionIndexError};
pub use layout::{
    MAIN_AGENT_ID, agent_record_dir, agent_tasks_dir, agent_wire_path, agents_dir,
    main_agent_wire_path, relative_agent_record_dir, session_state_path,
};
pub use workspace::{
    encode_workdir_key, normalize_workdir, slugify_basename, workspace_sessions_dir,
};
```

- [ ] **Step 6: Run test to verify it passes**

Run:

```bash
cargo nextest run -p neo-agent-core --test session_jsonl session_layout_paths_are_agent_scoped
cargo nextest run -p neo-agent-core --test session_jsonl session_state_store_round_trips_main_and_subagent_records
```

Expected: PASS.

- [ ] **Step 7: Commit after explicit authorization**

Ask the user: `Authorize git add/git commit for Task 1?`

Only after authorization:

```bash
git add crates/neo-agent-core/src/session/layout.rs crates/neo-agent-core/src/session/agent_state.rs crates/neo-agent-core/src/session/mod.rs crates/neo-agent-core/tests/session_jsonl.rs
git commit -m "feat(session): add agent layout state"
```

### Task 2: Cut Core Session Creation And Metadata To Main Wire

**Files:**
- Modify: `crates/neo-agent-core/src/session/mod.rs`
- Modify: `crates/neo-agent/src/modes/run/session_mgmt.rs`
- Modify: `crates/neo-agent/src/modes/run/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/controller_factory.rs`
- Modify: `crates/neo-agent/src/modes/interactive/shell_command.rs`
- Test: `crates/neo-agent-core/tests/session_tree.rs`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Write failing tests for new main wire location**

Update or add assertions in `crates/neo-agent-core/tests/session_tree.rs`:

```rust
#[test]
fn session_metadata_lists_sessions_with_main_agent_wire() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path();
    let session_id = "session_00000000-0000-0000-0000-000000000001";
    let session_dir = sessions_dir.join(session_id);
    std::fs::create_dir_all(session_dir.join("agents/main")).expect("mkdir");
    std::fs::write(session_dir.join("agents/main/wire.jsonl"), "{\"kind\":\"neo.session.metadata\",\"format\":\"neo.session.jsonl\",\"schema_version\":1,\"created_at\":\"0\"}\n").expect("write wire");

    let store = neo_agent_core::session::SessionMetadataStore::new(sessions_dir);
    let sessions = store.list().expect("list");

    assert!(sessions.iter().any(|session| session.id == session_id));
}
```

Add an interactive path assertion in `crates/neo-agent/src/modes/interactive/tests.rs` near existing session path tests:

```rust
#[tokio::test]
async fn interactive_session_path_uses_main_agent_wire() {
    let harness = TestHarness::new().await;
    let path = super::controller_factory::create_interactive_session_path(&harness.config)
        .await
        .expect("session path");

    assert!(path.ends_with(std::path::Path::new("agents/main/wire.jsonl")));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo nextest run -p neo-agent-core --test session_tree session_metadata_lists_sessions_with_main_agent_wire
cargo test --package neo-agent --bin neo -- modes::interactive::tests::interactive_session_path_uses_main_agent_wire --exact --nocapture --include-ignored
```

Expected: FAIL because `SessionMetadataStore` and interactive session creation still look for `transcript.jsonl`.

- [ ] **Step 3: Update `SessionMetadataStore` to recognize main wire**

In `crates/neo-agent-core/src/session/mod.rs`, change the internal path helpers:

```rust
fn session_path(&self, session_id: &str) -> PathBuf {
    main_agent_wire_path(&self.session_dir(session_id))
}

fn ensure_session_exists(&self, session_id: &str) -> Result<(), SessionError> {
    if self.session_path(session_id).is_file() {
        Ok(())
    } else {
        Err(SessionError::MissingSession(session_id.to_owned()))
    }
}
```

In `session_ids`, replace the file check with:

```rust
if !main_agent_wire_path(&path).is_file() {
    continue;
}
```

- [ ] **Step 4: Create sessions with `state.json` and main wire**

In `crates/neo-agent/src/modes/run/session_mgmt.rs`, replace both `session_dir.join("transcript.jsonl")` returns with:

```rust
let mut state = neo_agent_core::session::SessionState::new();
state.ensure_main_agent();
neo_agent_core::session::SessionStateStore::new(&session_dir)
    .write(&state)
    .await
    .with_context(|| format!("failed to write session state {}", session_dir.display()))?;
return Ok(neo_agent_core::session::main_agent_wire_path(&session_dir));
```

Change `session_id_from_path` so it requires `wire.jsonl` under `agents/main`:

```rust
if file_name != "wire.jsonl" {
    anyhow::bail!("invalid session wire path {}", path.display());
}
let main_dir = path.parent().with_context(|| {
    format!("session wire has no parent directory {}", path.display())
})?;
if main_dir.file_name().and_then(std::ffi::OsStr::to_str) != Some("main") {
    anyhow::bail!("invalid main agent wire path {}", path.display());
}
let agents_dir = main_dir.parent().with_context(|| {
    format!("main agent directory has no parent {}", main_dir.display())
})?;
if agents_dir.file_name().and_then(std::ffi::OsStr::to_str) != Some("agents") {
    anyhow::bail!("invalid agents directory {}", agents_dir.display());
}
let session_dir = agents_dir.parent().with_context(|| {
    format!("agents directory has no session parent {}", agents_dir.display())
})?;
```

In `latest_session_id`, replace `path.join("transcript.jsonl")` with:

```rust
let transcript = neo_agent_core::session::main_agent_wire_path(&path);
```

- [ ] **Step 5: Update interactive session creation**

In `crates/neo-agent/src/modes/interactive/controller_factory.rs`, replace the old return with the same state initialization from Step 4 and return `main_agent_wire_path(&session_dir)`.

In `session_id_from_transcript_path`, rename locally to `session_id_from_wire_path` if practical and derive the session id by walking up from `agents/main/wire.jsonl`:

```rust
let main_dir = path.parent().with_context(|| format!("invalid wire path {}", path.display()))?;
let agents_dir = main_dir.parent().with_context(|| format!("invalid wire path {}", path.display()))?;
let session_dir = agents_dir.parent().with_context(|| format!("invalid wire path {}", path.display()))?;
let id = session_dir
    .file_name()
    .and_then(std::ffi::OsStr::to_str)
    .with_context(|| format!("invalid session directory name {}", session_dir.display()))?;
Ok(id.to_owned())
```

- [ ] **Step 6: Update shell event persistence**

In `crates/neo-agent/src/modes/interactive/shell_command.rs`, `ensure_shell_session_path` should keep calling `crate::modes::sessions::session_path(session_id, config)` after Task 3 changes that helper. When creating a new shell session, use the updated `create_interactive_session_path` and `session_id_from_wire_path` names.

- [ ] **Step 7: Run tests to verify they pass**

Run:

```bash
cargo nextest run -p neo-agent-core --test session_tree session_metadata_lists_sessions_with_main_agent_wire
cargo test --package neo-agent --bin neo -- modes::interactive::tests::interactive_session_path_uses_main_agent_wire --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 8: Commit after explicit authorization**

Ask the user: `Authorize git add/git commit for Task 2?`

Only after authorization:

```bash
git add crates/neo-agent-core/src/session/mod.rs crates/neo-agent-core/tests/session_tree.rs crates/neo-agent/src/modes/run/session_mgmt.rs crates/neo-agent/src/modes/run/mod.rs crates/neo-agent/src/modes/interactive/controller_factory.rs crates/neo-agent/src/modes/interactive/shell_command.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "feat(session): store main wire under agents"
```

### Task 3: Cut CLI, RPC, Export, Compact, And Summary Readers To Main Wire

**Files:**
- Modify: `crates/neo-agent/src/modes/sessions.rs`
- Modify: `crates/neo-agent/src/rpc/server.rs`
- Modify: `crates/neo-agent-core/src/tools/sessions.rs`
- Test: `crates/neo-agent/tests/cli_commands.rs`
- Test: `crates/neo-agent/tests/rpc_mode.rs`
- Test: `crates/neo-agent-core/tests/session_jsonl.rs`

- [ ] **Step 1: Write failing CLI and RPC tests**

In `crates/neo-agent/tests/cli_commands.rs`, change the fixture helper that writes a session from:

```rust
fs::write(session_dir.join("transcript.jsonl"), content).expect("write transcript");
```

to:

```rust
let wire = session_dir.join("agents").join("main").join("wire.jsonl");
fs::create_dir_all(wire.parent().expect("wire parent")).expect("mkdir wire parent");
fs::write(&wire, content).expect("write main wire");
fs::write(
    session_dir.join("state.json"),
    "{\"schema_version\":1,\"agents\":{\"main\":{\"kind\":\"main\",\"record_dir\":\"agents/main\",\"parent_agent_id\":null}}}\n",
)
.expect("write state");
```

In `crates/neo-agent/tests/rpc_mode.rs`, update the session fixture writes the same way and assert returned paths end with `agents/main/wire.jsonl` where path assertions exist.

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo nextest run -p neo-agent --test cli_commands sessions_show_and_resume_read_jsonl_transcripts
cargo nextest run -p neo-agent --test rpc_mode rpc_get_messages_replays_session_jsonl_messages
```

Expected: FAIL because CLI/RPC handlers still hardcode `transcript.jsonl`.

- [ ] **Step 3: Update session path resolution**

In `crates/neo-agent/src/modes/sessions.rs`, change `session_path`:

```rust
pub fn session_path(session_ref: &str, config: &AppConfig) -> anyhow::Result<PathBuf> {
    let session_id = resolve_session_id(session_ref, config)?;
    let bucket_dir = workspace_sessions_dir(config);
    Ok(neo_agent_core::session::main_agent_wire_path(
        &bucket_dir.join(&session_id),
    ))
}
```

Replace `session_id_from_jsonl_path` with logic that accepts only paths ending in `agents/main/wire.jsonl` inside the current bucket:

```rust
if path.file_name().and_then(|n| n.to_str()) == Some("wire.jsonl")
    && path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) == Some("main")
    && path.parent().and_then(|p| p.parent()).and_then(|p| p.file_name()).and_then(|n| n.to_str()) == Some("agents")
    && let Some(session_id) = path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
{
    validate_session_id(session_id)
        .map_err(|_| anyhow::anyhow!("invalid session id {session_id:?}"))?;
    return Ok(Some(session_id.to_owned()));
}
```

- [ ] **Step 4: Update RPC hardcoded paths**

In `crates/neo-agent/src/rpc/server.rs`, replace each:

```rust
workspace_sessions_dir(config).join(&session_id).join("transcript.jsonl")
```

with:

```rust
neo_agent_core::session::main_agent_wire_path(
    &workspace_sessions_dir(config).join(&session_id),
)
```

Replace session counting checks:

```rust
name.starts_with("session_") && path.join("transcript.jsonl").is_file()
```

with:

```rust
name.starts_with("session_") && neo_agent_core::session::main_agent_wire_path(&path).is_file()
```

- [ ] **Step 5: Update `SummarizeSessionsTool`**

In `crates/neo-agent-core/src/tools/sessions.rs`, replace `PathBuf::from(session_dir).join("transcript.jsonl")` with:

```rust
crate::session::main_agent_wire_path(&PathBuf::from(session_dir))
```

In `recent_session_path`, use the same helper.

- [ ] **Step 6: Update compaction test expectations**

In `crates/neo-agent-core/tests/session_jsonl.rs`, update any fixture path named `session_path` so it is constructed with:

```rust
let session_dir = temp.path().join("session_00000000-0000-0000-0000-000000000001");
let session_path = neo_agent_core::session::main_agent_wire_path(&session_dir);
std::fs::create_dir_all(session_path.parent().expect("wire parent")).expect("mkdir wire parent");
```

- [ ] **Step 7: Run tests to verify pass**

Run:

```bash
cargo nextest run -p neo-agent --test cli_commands sessions_show_and_resume_read_jsonl_transcripts
cargo nextest run -p neo-agent --test cli_commands sessions_compact_stores_algorithmic_summary_and_resume_replays_kept_context
cargo nextest run -p neo-agent --test rpc_mode rpc_get_messages_replays_session_jsonl_messages
cargo nextest run -p neo-agent-core --test session_jsonl jsonl_session_compaction_appends_algorithmic_summary_and_replays_kept_context
```

Expected: PASS.

- [ ] **Step 8: Commit after explicit authorization**

Ask the user: `Authorize git add/git commit for Task 3?`

Only after authorization:

```bash
git add crates/neo-agent/src/modes/sessions.rs crates/neo-agent/src/rpc/server.rs crates/neo-agent-core/src/tools/sessions.rs crates/neo-agent/tests/cli_commands.rs crates/neo-agent/tests/rpc_mode.rs crates/neo-agent-core/tests/session_jsonl.rs
git commit -m "refactor(session): read main wire from agent layout"
```

### Task 4: Add One-Shot Session Migration Script

**Files:**
- Create: `scripts/migrate_sessions_to_agent_layout.py`
- Create: `crates/neo-agent/tests/session_migration.rs`
- Modify: `crates/neo-agent/Cargo.toml` if integration test target discovery requires no change, leave untouched.

- [ ] **Step 1: Write a failing migration smoke test**

Create `crates/neo-agent/tests/session_migration.rs`:

```rust
use std::{fs, process::Command};

#[test]
fn migration_script_moves_transcript_to_main_wire_and_writes_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let neo_home = temp.path().join(".neo");
    let session_dir = neo_home
        .join("sessions")
        .join("wd_neo_000000000000")
        .join("session_00000000-0000-0000-0000-000000000001");
    fs::create_dir_all(&session_dir).expect("mkdir session");
    fs::write(session_dir.join("transcript.jsonl"), "{\"kind\":\"neo.session.metadata\",\"format\":\"neo.session.jsonl\",\"schema_version\":1,\"created_at\":\"0\"}\n")
        .expect("write transcript");

    let output = Command::new("python3")
        .arg("scripts/migrate_sessions_to_agent_layout.py")
        .arg("--neo-home")
        .arg(&neo_home)
        .arg("--apply")
        .arg("--no-backup")
        .output()
        .expect("run migration");

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(session_dir.join("agents/main/wire.jsonl").is_file());
    assert!(session_dir.join("state.json").is_file());
    assert!(!session_dir.join("transcript.jsonl").exists());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo nextest run -p neo-agent --test session_migration migration_script_moves_transcript_to_main_wire_and_writes_state
```

Expected: FAIL because `scripts/migrate_sessions_to_agent_layout.py` does not exist.

- [ ] **Step 3: Create migration script**

Create `scripts/migrate_sessions_to_agent_layout.py`:

```python
#!/usr/bin/env python3
import argparse
import json
import shutil
import sys
from pathlib import Path


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--neo-home", default=str(Path.home() / ".neo"))
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--no-backup", action="store_true")
    return parser.parse_args()


def state_json():
    return {
        "schema_version": 1,
        "agents": {
            "main": {
                "kind": "main",
                "record_dir": "agents/main",
                "parent_agent_id": None,
            }
        },
    }


def migrate_session(session_dir: Path, apply: bool, backup: bool):
    old = session_dir / "transcript.jsonl"
    new = session_dir / "agents" / "main" / "wire.jsonl"
    state = session_dir / "state.json"

    if new.exists():
        return "skipped:new-layout"
    if not old.is_file():
        return "skipped:no-transcript"
    if not apply:
        return "would-migrate"

    if backup:
        backup_dir = session_dir.with_name(session_dir.name + ".pre-agent-layout-backup")
        if backup_dir.exists():
            shutil.rmtree(backup_dir)
        shutil.copytree(session_dir, backup_dir)

    new.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(old, new)
    state.write_text(json.dumps(state_json(), indent=2) + "\n", encoding="utf-8")
    old.unlink()
    return "migrated"


def iter_sessions(neo_home: Path):
    sessions_root = neo_home / "sessions"
    if not sessions_root.exists():
        return
    for bucket in sorted(sessions_root.iterdir()):
        if not bucket.is_dir():
            continue
        for session_dir in sorted(bucket.iterdir()):
            if session_dir.is_dir() and session_dir.name.startswith("session_"):
                yield session_dir


def main():
    args = parse_args()
    apply = args.apply and not args.dry_run
    backup = apply and not args.no_backup
    neo_home = Path(args.neo_home).expanduser()
    failed = 0
    count = 0
    for session_dir in iter_sessions(neo_home) or []:
        count += 1
        try:
            status = migrate_session(session_dir, apply, backup)
            print(f"{status}\t{session_dir}")
        except Exception as exc:
            failed += 1
            print(f"failed\t{session_dir}\t{exc}", file=sys.stderr)
    if count == 0:
        print(f"skipped:no-sessions\t{neo_home / 'sessions'}")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
```

- [ ] **Step 4: Run migration test to verify pass**

Run:

```bash
cargo nextest run -p neo-agent --test session_migration migration_script_moves_transcript_to_main_wire_and_writes_state
```

Expected: PASS.

- [ ] **Step 5: Run dry-run manually on the workspace home only if safe**

Run:

```bash
python3 scripts/migrate_sessions_to_agent_layout.py --neo-home /tmp/nonexistent-neo-home --dry-run
```

Expected: exit 0 and output `skipped:no-sessions`.

- [ ] **Step 6: Commit after explicit authorization**

Ask the user: `Authorize git add/git commit for Task 4?`

Only after authorization:

```bash
git add scripts/migrate_sessions_to_agent_layout.py crates/neo-agent/tests/session_migration.rs
git commit -m "feat(session): add agent layout migration"
```

### Task 5: Restore Delegate And Swarm State During Main Replay

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-tui/src/transcript/pane.rs` only if current API cannot replay events directly.
- Test: `crates/neo-agent-core/tests/multi_agent_runtime.rs`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Write failing runtime restore test**

Append to `crates/neo-agent-core/tests/multi_agent_runtime.rs`:

```rust
#[test]
fn replayed_delegate_snapshot_can_be_resumed_after_session_restore() {
    use neo_agent_core::{
        AgentEvent,
        multi_agent::{DelegateRequest, MultiAgentRuntime},
    };

    let runtime = MultiAgentRuntime::new();
    let snapshot = runtime.start_foreground_delegate_for_test("audit session paths");
    let agent_id = snapshot.id.as_str().to_owned();
    let events = vec![AgentEvent::DelegateFinished {
        turn: 3,
        agent: snapshot,
    }];

    let restored = MultiAgentRuntime::new();
    restored.restore_from_replay(events.iter());

    let request = DelegateRequest {
        task: "continue audit".to_owned(),
        resume: Some(agent_id.clone()),
        title: None,
        role: None,
        mode: neo_agent_core::multi_agent::AgentRunMode::Foreground,
        context: neo_agent_core::multi_agent::DelegateContext::Inherit,
    };
    let resumed = restored
        .start_resume_delegate(&agent_id, &request)
        .expect("resume restored agent");

    assert_eq!(resumed.id.as_str(), agent_id);
    assert_eq!(resumed.run_count, 2);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_runtime replayed_delegate_snapshot_can_be_resumed_after_session_restore
```

Expected: FAIL because `restore_from_replay` does not exist.

- [ ] **Step 3: Implement restore API**

In `crates/neo-agent-core/src/multi_agent/runtime.rs`, add:

```rust
impl MultiAgentRuntime {
    pub fn restore_from_replay<'a>(&self, events: impl IntoIterator<Item = &'a AgentEvent>) {
        for event in events {
            match event {
                AgentEvent::DelegateStarted { agent, .. }
                | AgentEvent::DelegateUpdated { agent, .. }
                | AgentEvent::DelegateFinished { agent, .. } => {
                    self.restore_agent_snapshot(agent.clone());
                }
                AgentEvent::DelegateSwarmStarted { swarm, .. }
                | AgentEvent::DelegateSwarmUpdated { swarm, .. }
                | AgentEvent::DelegateSwarmFinished { swarm, .. } => {
                    self.restore_swarm_snapshot(swarm.clone());
                }
                _ => {}
            }
        }
        self.mark_restored_running_agents_lost();
    }

    pub fn restore_agent_snapshot(&self, snapshot: AgentSnapshot) {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        state.register_agent_order(snapshot.id.as_str());
        state
            .agents
            .entry(snapshot.id.as_str().to_owned())
            .and_modify(|current| {
                if snapshot.updated_at_ms >= current.updated_at_ms {
                    *current = snapshot.clone();
                }
            })
            .or_insert(snapshot);
    }

    pub fn restore_swarm_snapshot(&self, snapshot: super::SwarmSnapshot) {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        state.register_swarm_order(&snapshot.swarm_id);
        for child in &snapshot.children {
            state.register_agent_order(child.agent.id.as_str());
            state
                .agents
                .insert(child.agent.id.as_str().to_owned(), child.agent.clone());
        }
        state.swarms.insert(snapshot.swarm_id.clone(), snapshot);
    }

    fn mark_restored_running_agents_lost(&self) {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        for agent in state.agents.values_mut() {
            if matches!(agent.state, AgentLifecycleState::Queued | AgentLifecycleState::Running) {
                agent.state = AgentLifecycleState::Failed;
                agent.terminal_reason = Some(AgentTerminalReason::Lost);
                agent.terminal_at_ms.get_or_insert(now_ms());
                agent.updated_at_ms = now_ms();
                agent.outcome = Some(AgentTerminalOutcome {
                    summary: format!(
                        "Agent was running when the previous Neo process exited. Resume with Delegate(resume=\"{}\").",
                        agent.id.as_str()
                    ),
                    is_error: true,
                });
            }
        }
    }
}
```

- [ ] **Step 4: Write failing interactive transcript replay test**

Add to `crates/neo-agent/src/modes/interactive/tests.rs`:

```rust
#[tokio::test]
async fn load_session_transcript_preserves_delegate_events_for_replay() {
    let harness = TestHarness::new().await;
    let session_id = "session_00000000-0000-0000-0000-000000000001";
    let session_dir = workspace_sessions_dir(&harness.config).join(session_id);
    let wire = neo_agent_core::session::main_agent_wire_path(&session_dir);
    tokio::fs::create_dir_all(wire.parent().expect("wire parent"))
        .await
        .expect("mkdir");
    let mut writer = neo_agent_core::session::JsonlSessionWriter::create(&wire)
        .await
        .expect("writer");
    let snapshot = neo_agent_core::multi_agent::MultiAgentRuntime::new()
        .start_foreground_delegate_for_test("audit paths");
    writer
        .append_event(&neo_agent_core::AgentEvent::DelegateStarted { turn: 1, agent: snapshot })
        .await
        .expect("append delegate");
    writer.flush().await.expect("flush");

    let loaded = super::load_session_transcript(session_id.to_owned(), &harness.config)
        .await
        .expect("load");

    assert!(
        loaded.events.iter().any(|event| matches!(
            event,
            neo_agent_core::AgentEvent::DelegateStarted { .. }
        )),
        "delegate events should be preserved for transcript replay"
    );
}
```

- [ ] **Step 5: Extend loaded transcript model**

Modify the local `LoadedSessionTranscript` type in `crates/neo-agent/src/modes/interactive/mod.rs` to include:

```rust
events: Vec<neo_agent_core::AgentEvent>,
```

When constructing it in `load_session_transcript`, pass `events.clone()` before creating context:

```rust
let events = JsonlSessionReader::read_all(&path).await?;
let context = neo_agent_core::AgentContext::from_replay(events.iter());
let multi_agent = config.multi_agent.clone();
multi_agent.restore_from_replay(events.iter());
```

In `replay_session_into_transcript`, after notices and before message replay, replay delegate events:

```rust
for event in &loaded.events {
    match event {
        AgentEvent::DelegateStarted { .. }
        | AgentEvent::DelegateUpdated { .. }
        | AgentEvent::DelegateFinished { .. }
        | AgentEvent::DelegateSwarmStarted { .. }
        | AgentEvent::DelegateSwarmUpdated { .. }
        | AgentEvent::DelegateSwarmFinished { .. } => {
            transcript.apply_agent_event(event.clone());
        }
        _ => {}
    }
}
```

If `apply_agent_event` is not public on `TranscriptPane`, expose the smallest method that already routes through the existing event handler.

- [ ] **Step 6: Run tests to verify pass**

Run:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_runtime replayed_delegate_snapshot_can_be_resumed_after_session_restore
cargo test --package neo-agent --bin neo -- modes::interactive::tests::load_session_transcript_preserves_delegate_events_for_replay --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 7: Commit after explicit authorization**

Ask the user: `Authorize git add/git commit for Task 5?`

Only after authorization:

```bash
git add crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/tests/multi_agent_runtime.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/tests.rs crates/neo-tui/src/transcript/pane.rs
git commit -m "feat(multi-agent): restore delegates from session replay"
```

### Task 6: Persist Child Agent Wire And Replay It On Delegate Resume

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/config.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate.rs`
- Modify: `crates/neo-agent/src/modes/run/runtime/agent.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Write failing child wire persistence test**

Append to `crates/neo-agent-core/tests/multi_agent_runtime.rs`:

```rust
#[tokio::test]
async fn child_run_appends_events_to_agent_wire() {
    use neo_agent_core::{
        AgentConfig, ToolRegistry,
        multi_agent::{ChildRuntimeDeps, DelegateContext, DelegateRequest, MultiAgentRuntime},
        session::{SessionState, SessionStateStore, agent_wire_path},
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let session_dir = temp.path();
    let mut state = SessionState::new();
    state.ensure_main_agent();
    SessionStateStore::new(session_dir).write(&state).await.expect("state");

    let runtime = MultiAgentRuntime::new().with_session_directory(session_dir.to_path_buf());
    let deps = ChildRuntimeDeps::new(
        AgentConfig::for_model(fake_model_spec()),
        std::sync::Arc::new(neo_ai::FakeModelClient::from_turns(["child done"])),
        std::sync::Arc::new(ToolRegistry::new()),
    );
    let request = DelegateRequest {
        task: "say done".to_owned(),
        resume: None,
        title: None,
        role: None,
        mode: neo_agent_core::multi_agent::AgentRunMode::Foreground,
        context: DelegateContext::None,
    };

    let output = runtime
        .run_child_turn(deps, &request, neo_agent_core::multi_agent::AgentRunMode::Foreground)
        .await
        .expect("child run");
    let wire = agent_wire_path(session_dir, output.snapshot.id.as_str());

    assert!(wire.is_file(), "child wire should exist at {}", wire.display());
    let raw = tokio::fs::read_to_string(wire).await.expect("read wire");
    assert!(raw.contains("MessageAppended"), "{raw}");
}
```

If `fake_model_spec()` is not available in that test module, add a local helper matching existing multi-agent tests.

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_runtime child_run_appends_events_to_agent_wire
```

Expected: FAIL because `MultiAgentRuntime::with_session_directory` and child wire writing do not exist.

- [ ] **Step 3: Add session directory to `MultiAgentRuntime`**

In `crates/neo-agent-core/src/multi_agent/runtime.rs`, extend the struct:

```rust
#[derive(Debug, Clone, Default)]
pub struct MultiAgentRuntime {
    state: Arc<Mutex<MultiAgentState>>,
    session_directory: Option<PathBuf>,
}
```

Add:

```rust
#[must_use]
pub fn with_session_directory(mut self, session_directory: PathBuf) -> Self {
    self.session_directory = Some(session_directory);
    self
}
```

Update imports for `PathBuf`.

- [ ] **Step 4: Register subagent in `state.json` when creating a delegate**

In `create_delegate`, after inserting the snapshot, if `self.session_directory` is set, write a state record asynchronously is not possible inside the sync method. Instead, add an async method:

```rust
async fn register_persistent_agent(&self, snapshot: &AgentSnapshot, swarm_id: Option<&str>, swarm_item: Option<&str>) -> Result<(), String> {
    let Some(session_dir) = &self.session_directory else {
        return Ok(());
    };
    let store = crate::session::SessionStateStore::new(session_dir);
    let mut state = match store.read().await {
        Ok(state) => state,
        Err(_) => {
            let mut state = crate::session::SessionState::new();
            state.ensure_main_agent();
            state
        }
    };
    state.upsert_agent(crate::session::SessionAgentRecord {
        kind: crate::session::SessionAgentKind::Sub,
        record_dir: crate::session::relative_agent_record_dir(snapshot.id.as_str()),
        parent_agent_id: Some(crate::session::MAIN_AGENT_ID.to_owned()),
        role: Some(snapshot.role.as_str().to_owned()),
        swarm_id: swarm_id.map(str::to_owned),
        swarm_item: swarm_item.map(str::to_owned),
    });
    store.write(&state).await.map_err(|err| err.to_string())
}
```

`AgentRole::as_str` already exists in `crates/neo-agent-core/src/multi_agent/identity.rs`; use it for the persisted role string.

- [ ] **Step 5: Write child events to child wire**

In `run_agent_snapshot`, after creating `events`, open a child writer when a session directory and agent id are available. Change the function signature:

```rust
async fn run_agent_snapshot(
    deps: ChildRuntimeDeps,
    prompt: String,
    prior_messages: Vec<AgentMessage>,
    steer_input: SteerInputHandle,
    child_wire_path: Option<PathBuf>,
    mut on_event: impl FnMut(&AgentEvent) + Send,
) -> Result<(Vec<AgentEvent>, Vec<AgentMessage>), String>
```

Before the loop:

```rust
let mut writer = if let Some(path) = child_wire_path {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|err| err.to_string())?;
    }
    Some(crate::session::JsonlSessionWriter::open_append(path).await.map_err(|err| err.to_string())?)
} else {
    None
};
```

Inside the loop after `on_event(&event)`:

```rust
if let Some(writer) = writer.as_mut() {
    writer.append_event(&event).await.map_err(|err| err.to_string())?;
}
```

After the loop:

```rust
if let Some(writer) = writer.as_mut() {
    writer.flush().await.map_err(|err| err.to_string())?;
}
```

Pass `Some(crate::session::agent_wire_path(session_dir, snapshot.id.as_str()))` from `run_started_child_turn` and `run_started_swarm_child_turn` when `session_directory` exists.

- [ ] **Step 6: Replay child wire on resume**

In `run_started_child_turn`, replace:

```rust
let prior_messages = snapshot.prior_messages.clone();
```

with:

```rust
let prior_messages = if let Some(session_dir) = &self.session_directory {
    let wire = crate::session::agent_wire_path(session_dir, snapshot.id.as_str());
    match crate::session::JsonlSessionReader::replay_messages(&wire).await {
        Ok(messages) => messages,
        Err(_) => snapshot.prior_messages.clone(),
    }
} else {
    snapshot.prior_messages.clone()
};
```

This temporary fallback to snapshot messages is not an old file-layout fallback; it is only an in-memory test/runtime path for sessions created before child wire exists in the same process.

- [ ] **Step 7: Pass session directory into shared runtime**

In `crates/neo-agent/src/modes/run/runtime/agent.rs`, after `.with_multi_agent(config.multi_agent.clone())`, ensure the shared multi-agent runtime has session directory when available:

```rust
let multi_agent = if let Some(session_dir) = &config.session_directory {
    config.multi_agent.clone().with_session_directory(session_dir.clone())
} else {
    config.multi_agent.clone()
};
```

Then pass `multi_agent` to `.with_multi_agent(multi_agent)`.

- [ ] **Step 8: Run test to verify pass**

Run:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_runtime child_run_appends_events_to_agent_wire
```

Expected: PASS.

- [ ] **Step 9: Commit after explicit authorization**

Ask the user: `Authorize git add/git commit for Task 6?`

Only after authorization:

```bash
git add crates/neo-agent-core/src/runtime/config.rs crates/neo-agent-core/src/tools/mod.rs crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/src/tools/delegate.rs crates/neo-agent/src/modes/run/runtime/agent.rs crates/neo-agent-core/tests/multi_agent_runtime.rs
git commit -m "feat(multi-agent): persist subagent wire"
```

### Task 7: Restore Background Delegate State As Lost After Resume

**Files:**
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_background.rs`

- [ ] **Step 1: Write failing lost-state test**

Add to `crates/neo-agent-core/tests/multi_agent_background.rs`:

```rust
#[test]
fn restored_running_delegate_is_reported_lost_with_resume_hint() {
    use neo_agent_core::{
        AgentEvent,
        multi_agent::{AgentLifecycleState, AgentTerminalReason, MultiAgentRuntime},
    };

    let runtime = MultiAgentRuntime::new();
    let running = runtime.start_foreground_delegate_for_test("long audit");
    let id = running.id.as_str().to_owned();
    let restored = MultiAgentRuntime::new();
    restored.restore_from_replay([AgentEvent::DelegateStarted { turn: 1, agent: running }].iter());

    let snapshot = restored.agent_snapshot(&id).expect("restored");
    assert_eq!(snapshot.state, AgentLifecycleState::Failed);
    assert_eq!(snapshot.terminal_reason, Some(AgentTerminalReason::Lost));
    assert!(
        snapshot
            .outcome
            .as_ref()
            .expect("outcome")
            .summary
            .contains(&format!("Delegate(resume=\"{id}\")"))
    );
}
```

- [ ] **Step 2: Run test to verify it fails or exposes wording mismatch**

Run:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_background restored_running_delegate_is_reported_lost_with_resume_hint
```

Expected: FAIL if Task 5 wording/state does not match, PASS if Task 5 already implemented exactly.

- [ ] **Step 3: Make lost state visible in delegate controls**

In `crates/neo-agent-core/src/tools/delegate_controls.rs`, ensure `WaitDelegate` and `ListDelegates` do not call a lost agent active. When formatting a `DelegateFinished` or restored failed snapshot with `terminal_reason = Lost`, include:

```text
resume_hint: Delegate(resume="<agent_id>", task="continue")
```

If the formatter already uses `agent_details`, add this field there for lost snapshots:

```rust
if snapshot.terminal_reason == Some(AgentTerminalReason::Lost) {
    details["resume_hint"] = serde_json::json!(
        format!("Delegate(resume=\"{}\", task=\"continue\")", snapshot.id.as_str())
    );
}
```

- [ ] **Step 4: Run lost-state test**

Run:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_background restored_running_delegate_is_reported_lost_with_resume_hint
```

Expected: PASS.

- [ ] **Step 5: Commit after explicit authorization**

Ask the user: `Authorize git add/git commit for Task 7?`

Only after authorization:

```bash
git add crates/neo-agent-core/src/tools/background_tasks.rs crates/neo-agent-core/src/tools/delegate_controls.rs crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/tests/multi_agent_background.rs
git commit -m "fix(multi-agent): mark restored running delegates lost"
```

### Task 8: Persist Agent-Scoped Background Tasks And Output Logs

**Files:**
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`
- Modify: `crates/neo-agent-core/src/tools/bash.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`
- Test: `crates/neo-agent-core/src/tools/background_tasks.rs`

- [ ] **Step 1: Write failing background persistence unit test**

Append to the test module in `crates/neo-agent-core/src/tools/background_tasks.rs`:

```rust
#[tokio::test]
async fn background_task_manager_persists_output_under_agent_tasks_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tasks_dir = temp.path().join("agents/main/tasks");
    let manager = BackgroundTaskManager::new().with_persistence_dir(tasks_dir.clone());

    manager
        .persist_task_output_for_test("bash-12345678", "hello\n")
        .await
        .expect("persist output");

    assert_eq!(
        tokio::fs::read_to_string(tasks_dir.join("bash-12345678/output.log"))
            .await
            .expect("read output"),
        "hello\n"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo nextest run -p neo-agent-core --lib background_task_manager_persists_output_under_agent_tasks_dir
```

Expected: FAIL because `with_persistence_dir` and `persist_task_output_for_test` do not exist.

- [ ] **Step 3: Add optional persistence directory**

In `BackgroundTaskManager`, add:

```rust
#[derive(Clone, Default)]
pub struct BackgroundTaskManager {
    inner: Arc<Mutex<HashMap<String, BackgroundTaskRecord>>>,
    persistence_dir: Option<Arc<PathBuf>>,
}
```

Add methods:

```rust
#[must_use]
pub fn with_persistence_dir(mut self, path: PathBuf) -> Self {
    self.persistence_dir = Some(Arc::new(path));
    self
}

async fn append_persistent_output(&self, task_id: &str, chunk: &str) -> Result<(), ToolError> {
    let Some(root) = &self.persistence_dir else {
        return Ok(());
    };
    let output = root.join(task_id).join("output.log");
    if let Some(parent) = output.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(output)
        .await?;
    use tokio::io::AsyncWriteExt;
    file.write_all(chunk.as_bytes()).await?;
    Ok(())
}

#[cfg(test)]
pub async fn persist_task_output_for_test(&self, task_id: &str, chunk: &str) -> Result<(), ToolError> {
    self.append_persistent_output(task_id, chunk).await
}
```

- [ ] **Step 4: Wire task persistence from `ToolContext`**

In `ToolContext`, add:

```rust
pub agent_id: Option<String>,
pub session_directory: Option<PathBuf>,
```

When constructing runtime tool contexts, set `agent_id = Some("main".to_owned())` for main turns. When a child runtime dispatches tools, set it to the child agent id once Task 6 has that id in scope. Use:

```rust
let task_dir = session_directory
    .as_ref()
    .zip(agent_id.as_deref())
    .map(|(session_dir, agent_id)| crate::session::agent_tasks_dir(session_dir, agent_id));
```

and pass `BackgroundTaskManager::new().with_persistence_dir(task_dir)` for that agent.

- [ ] **Step 5: Append background shell output to `output.log`**

In `crates/neo-agent-core/src/tools/bash.rs`, wherever background stdout/stderr chunks are collected for `BackgroundTaskManager`, call:

```rust
ctx.background_tasks
    .append_persistent_output(&task_id, &chunk)
    .await?;
```

Keep existing bounded in-memory previews unchanged.

- [ ] **Step 6: Run test to verify pass**

Run:

```bash
cargo nextest run -p neo-agent-core --lib background_task_manager_persists_output_under_agent_tasks_dir
```

Expected: PASS.

- [ ] **Step 7: Commit after explicit authorization**

Ask the user: `Authorize git add/git commit for Task 8?`

Only after authorization:

```bash
git add crates/neo-agent-core/src/tools/background_tasks.rs crates/neo-agent-core/src/tools/bash.rs crates/neo-agent-core/src/tools/mod.rs
git commit -m "feat(tasks): persist agent scoped output logs"
```

### Task 9: Remove Obsolete Transcript Path References

**Files:**
- Modify: any source/test files still containing active `transcript.jsonl`
- Test: targeted command below

- [ ] **Step 1: Search for remaining active old-path references**

Run:

```bash
rg -n "transcript\\.jsonl" crates scripts docs --glob '!docs/kimi-code/**'
```

Expected: output contains only historical documentation or migration-test setup that intentionally creates an old session. Source runtime files should not contain active reads or writes to `transcript.jsonl`.

- [ ] **Step 2: Replace active source references**

For each active source hit, replace path construction with:

```rust
neo_agent_core::session::main_agent_wire_path(&session_dir)
```

For test fixtures that intentionally model pre-migration sessions, keep the old filename and add a comment:

```rust
// Migration fixture: old sessions used transcript.jsonl before the agent layout.
```

- [ ] **Step 3: Run narrow regression commands**

Run:

```bash
cargo nextest run -p neo-agent-core --test session_jsonl jsonl_session_appends_reads_and_replays_events
cargo nextest run -p neo-agent-core --test session_tree session_metadata_lists_sessions_with_main_agent_wire
cargo nextest run -p neo-agent-core --test multi_agent_runtime replayed_delegate_snapshot_can_be_resumed_after_session_restore
cargo nextest run -p neo-agent --test session_migration migration_script_moves_transcript_to_main_wire_and_writes_state
cargo nextest run -p neo-agent --test rpc_mode rpc_get_messages_replays_session_jsonl_messages
```

Expected: PASS.

- [ ] **Step 4: Commit after explicit authorization**

Ask the user: `Authorize git add/git commit for Task 9?`

Only after authorization:

```bash
git add crates scripts docs
git commit -m "refactor(session): remove transcript path runtime usage"
```

## Self-Review

Spec coverage:

- New layout and path API: Task 1 and Task 2.
- `state.json` registry: Task 1, Task 4, Task 6.
- Main wire cutover: Task 2 and Task 3.
- Migration script: Task 4.
- Delegate/Swarm transcript and runtime replay: Task 5 and Task 7.
- Subagent child wire persistence and resume: Task 6.
- Agent-scoped tasks/output logs: Task 8.
- No long-term `transcript.jsonl` runtime path: Task 9.
- Narrow tests: every task has exact commands and expected outcomes.

Plan hygiene scan:

- The plan contains no unfinished-marker terms or incomplete implementation slots.
- Angle-bracket strings appear only as path notation in prose or shell descriptions, not as missing values for an engineer to invent.

Type consistency:

- `SessionAgentKind`, `SessionAgentRecord`, `SessionState`, and `SessionStateStore` are defined in Task 1 before later tasks use them.
- `main_agent_wire_path`, `agent_wire_path`, `agent_tasks_dir`, and `relative_agent_record_dir` are defined in Task 1 before later tasks use them.
- `restore_from_replay` is defined in Task 5 before later tasks rely on restored lost state.
- `with_session_directory` is defined in Task 6 before child wire persistence uses it.
