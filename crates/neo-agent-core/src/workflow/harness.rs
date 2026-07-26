//! Deterministic workflow fixture harness (design §39.2).
//!
//! Runs the real resolved definition, Lua host, `WorkflowRuntime`, journal/replay,
//! schema, and artifact code in temporary storage. External effects are driven by
//! fixture-owned fake outcomes (`FakeModelClient`, scripted tools, awaited answers).
//!
//! There is no live-provider, shell, MCP, or hidden live-execution switch.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use neo_ai::{AiStreamEvent, StopReason, TokenUsage};

use crate::AgentContext;
use crate::harness::FakeHarness;
use crate::runtime::WorkflowDispatchHandle;
use crate::tools::{
    ProcessSupervisor, SleepTool, Tool, ToolContext, ToolFuture, ToolRegistry, ToolResult,
};
use crate::workflow::artifacts::{ArtifactKind, ArtifactValue};
use crate::workflow::definition::ResolvedWorkflowDefinition;
use crate::workflow::error::{WorkflowError, WorkflowErrorCode};
use crate::workflow::journal::{JournalPayload, collect_journal_v2, run_dir};
use crate::workflow::lua::LuaWorkflowRunner;
use crate::workflow::output::FinalResultBody;
use crate::workflow::state::{WorkflowActor, WorkflowInvocationKind, WorkflowState};
use crate::workflow::{WorkflowLimits, WorkflowRuntime};

/// Fixture execution is always deterministic. Live mode is intentionally absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FixtureExecutionMode {
    #[default]
    Deterministic,
}

impl FixtureExecutionMode {
    /// Live provider/tool execution is outside the V2 fixture harness.
    #[must_use]
    pub const fn supports_live(self) -> bool {
        false
    }
}

/// Scripted model turn for `FakeModelClient` (text only; no live provider).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureModelTurn {
    pub text: String,
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
}

/// Scripted generic tool outcome (`neo.tool`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureToolOutcome {
    pub name: String,
    #[serde(default = "default_true")]
    pub ok: bool,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub details: Value,
}

/// Scripted delegate outcome when no model turns drive the real Delegate path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureDelegateOutcome {
    #[serde(default = "default_true")]
    pub ok: bool,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub details: Value,
    /// Optional invalid first model text; pairs with `repair_raw` for one tools-disabled repair.
    #[serde(default)]
    pub first_raw: Option<String>,
    /// Exactly one tools-disabled repair model text.
    #[serde(default)]
    pub repair_raw: Option<String>,
}

/// One swarm item outcome (homogeneous or heterogeneous batch).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureSwarmItemOutcome {
    #[serde(default = "default_true")]
    pub ok: bool,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub details: Value,
}

/// Durable answer applied when the run enters `awaiting_user`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureAwaitedAnswer {
    /// When omitted, the current open request is answered.
    #[serde(default)]
    pub request_id: Option<String>,
    pub value: Value,
}

/// Artifact the harness commits through the real store (fixture-owned seed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureArtifactSpec {
    pub logical_name: String,
    #[serde(default = "default_json_kind")]
    pub kind: String,
    pub value: Value,
}

/// Expected journal invocation kind entry (order-sensitive subsequence).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureExpectedInvocation {
    pub kind: String,
}

/// Concrete deterministic fixture document.
///
/// Unknown fields are rejected — including any `live` / provider-execution flags.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowFixture {
    #[serde(default)]
    pub args: Value,
    #[serde(default)]
    pub model_turns: Vec<FixtureModelTurn>,
    #[serde(default)]
    pub delegate_outcomes: Vec<FixtureDelegateOutcome>,
    #[serde(default)]
    pub swarm_outcomes: Vec<FixtureSwarmItemOutcome>,
    #[serde(default)]
    pub tool_outcomes: Vec<FixtureToolOutcome>,
    #[serde(default)]
    pub awaited_answers: Vec<FixtureAwaitedAnswer>,
    /// Artifacts committed via real `commit_artifact` while the run is non-terminal.
    #[serde(default)]
    pub seed_artifacts: Vec<FixtureArtifactSpec>,
    #[serde(default)]
    pub expected_result: Option<Value>,
    #[serde(default)]
    pub expected_reports: Vec<Value>,
    #[serde(default)]
    pub expected_artifacts: Vec<FixtureArtifactSpec>,
    #[serde(default)]
    pub expected_invocation_trace: Vec<FixtureExpectedInvocation>,
    /// Only `deterministic` is accepted. Live execution is not a fixture mode.
    #[serde(default)]
    pub mode: FixtureExecutionMode,
}

fn default_true() -> bool {
    true
}

fn default_json_kind() -> String {
    "json".to_owned()
}

/// Parse a fixture document. Rejects unknown fields (including live switches).
pub fn parse_fixture(text: &str) -> Result<WorkflowFixture, WorkflowError> {
    let fixture: WorkflowFixture = serde_json::from_str(text).map_err(|err| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!("fixture parse failed: {err}"),
        )
    })?;
    if fixture.mode.supports_live() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            "fixture harness has no live execution mode",
        ));
    }
    if !fixture.args.is_null() && !fixture.args.is_object() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            "fixture args must be a JSON object when present",
        ));
    }
    Ok(fixture)
}

/// Load a fixture JSON file.
pub fn load_fixture(path: &Path) -> Result<WorkflowFixture, WorkflowError> {
    let text = std::fs::read_to_string(path).map_err(|err| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!("failed to read fixture {}: {err}", path.display()),
        )
    })?;
    parse_fixture(&text)
}

/// Result of one deterministic fixture run.
#[derive(Debug, Clone, Serialize)]
pub struct FixtureRunReport {
    pub ok: bool,
    pub name: String,
    pub run_id: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_result: Option<Value>,
    pub diagnostics: Vec<Value>,
    pub invocation_kinds: Vec<String>,
    pub schema_repair_starts: usize,
    pub journal_path: PathBuf,
    pub session_dir: PathBuf,
}

/// Execute a resolved definition against a fixture (drops temp session after read).
pub async fn run_fixture(
    definition: &ResolvedWorkflowDefinition,
    fixture: &WorkflowFixture,
    limits: WorkflowLimits,
) -> Result<FixtureRunReport, WorkflowError> {
    let (report, _session, _runtime) = run_fixture_retained(definition, fixture, limits).await?;
    Ok(report)
}

/// Like [`run_fixture`] but retains the temporary session directory for rehydrate tests.
pub async fn run_fixture_retained(
    definition: &ResolvedWorkflowDefinition,
    fixture: &WorkflowFixture,
    limits: WorkflowLimits,
) -> Result<(FixtureRunReport, tempfile::TempDir, WorkflowRuntime), WorkflowError> {
    if fixture.mode.supports_live() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            "fixture harness has no live execution mode",
        ));
    }

    let session = tempfile::tempdir().map_err(|err| {
        WorkflowError::coded(
            WorkflowErrorCode::Host,
            format!("failed to create temp session dir: {err}"),
        )
    })?;
    let session_dir = session.path().to_path_buf();

    let use_real_delegate = fixture_uses_real_delegate(fixture);
    let model_turns = model_turns_from_fixture(fixture);
    let harness = FakeHarness::from_turns(model_turns);

    let scripted_tools: Arc<Mutex<VecDeque<FixtureToolOutcome>>> =
        Arc::new(Mutex::new(fixture.tool_outcomes.iter().cloned().collect()));
    let scripted_delegates: Arc<Mutex<VecDeque<FixtureDelegateOutcome>>> = Arc::new(Mutex::new(
        fixture.delegate_outcomes.iter().cloned().collect(),
    ));
    let scripted_swarm: Arc<Mutex<VecDeque<FixtureSwarmItemOutcome>>> =
        Arc::new(Mutex::new(fixture.swarm_outcomes.iter().cloned().collect()));

    let registry = build_fixture_registry(
        &scripted_tools,
        &scripted_delegates,
        &scripted_swarm,
        use_real_delegate,
    );

    let runtime = WorkflowRuntime::new(limits.clone());
    let config = crate::AgentConfig::for_model(harness.model())
        .with_workspace_root(session_dir.clone())
        .map_err(|err| {
            WorkflowError::coded(
                WorkflowErrorCode::Host,
                format!("workspace root failed: {err}"),
            )
        })?
        .with_permission_mode(crate::PermissionMode::Yolo)
        .with_session_directory(session_dir.clone())
        .with_workflow_runtime(runtime.clone());

    let model_client = harness.client();
    let registry = Arc::new(registry);
    let process_supervisor = ProcessSupervisor::default();
    let context = AgentContext::new();

    let definition_script = definition.lua_source.clone();
    let definition_args = if fixture.args.is_null() {
        json!({})
    } else {
        fixture.args.clone()
    };
    let final_schema = definition.compiled_output_schema.clone();
    let revision = Some(definition.revision.clone());
    let limits_for_runner = limits.clone();

    runtime
        .bind_runner({
            let config = config.clone();
            let model_client = Arc::clone(&model_client);
            let registry = Arc::clone(&registry);
            let process_supervisor = process_supervisor.clone();
            let context = context.clone();
            let script = definition_script;
            let args = definition_args.clone();
            let final_schema = final_schema.clone();
            let revision = revision.clone();
            move |handle, _metadata, _session| {
                let dispatch = WorkflowDispatchHandle {
                    config: config.clone(),
                    model_client: Arc::clone(&model_client),
                    registry: Arc::clone(&registry),
                    process_supervisor: process_supervisor.clone(),
                    context: context.clone(),
                };
                let script = script.clone();
                let args = args.clone();
                let final_schema = final_schema.clone();
                let revision = revision.clone();
                let limits = limits_for_runner.clone();
                async move {
                    let runner = LuaWorkflowRunner::new(dispatch, handle, limits)
                        .with_final_schema(final_schema, revision);
                    runner.execute(&script, args).await?;
                    Ok(())
                }
            }
        })
        .map_err(|err| {
            WorkflowError::coded(
                WorkflowErrorCode::Host,
                format!("bind runner failed: {err}"),
            )
        })?;

    let handle = runtime
        .create_run(
            &session_dir,
            crate::workflow::WorkflowLaunchRequest {
                name: definition.name.as_str().to_owned(),
                description: definition.description.clone(),
                phases: definition.phases.clone(),
                script: definition.lua_source.clone(),
                args: definition_args,
                launch_source: "workflow-fixture".to_owned(),
                parent_run_id: None,
                output_schema: Some(definition.output_schema.clone()),
            },
        )
        .await?;

    runtime.start_worker(&handle.run_id).await?;

    let mut answer_queue: VecDeque<_> = fixture.awaited_answers.iter().cloned().collect();
    let mut seeded = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if tokio::time::Instant::now() > deadline {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::Failed,
                "fixture run timed out waiting for terminal state",
            ));
        }
        let snapshot = handle.snapshot().await;
        if snapshot.state.is_terminal() {
            break;
        }
        if !seeded && !fixture.seed_artifacts.is_empty() {
            for art in &fixture.seed_artifacts {
                commit_fixture_artifact(&handle, art).await?;
            }
            seeded = true;
        }
        if snapshot.state == WorkflowState::AwaitingUser {
            let Some(answer) = answer_queue.pop_front() else {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::AwaitingUser,
                    "fixture has open await_user but no remaining awaited_answers",
                ));
            };
            let request_id = match answer.request_id {
                Some(id) => id,
                None => {
                    handle
                        .pending_user_input()
                        .await?
                        .ok_or_else(|| {
                            WorkflowError::coded(
                                WorkflowErrorCode::StaleUserRequest,
                                "awaiting_user without pending request",
                            )
                        })?
                        .request_id
                }
            };
            handle
                .answer(&request_id, answer.value, WorkflowActor::Human)
                .await?;
            continue;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let output = handle.output().await?;
    let run_directory = run_dir(&session_dir, &handle.run_id);
    let journal_path = run_directory.join("journal.jsonl");
    let envelopes = collect_journal_v2(&journal_path, Some(&handle.run_id))?;

    let invocation_kinds: Vec<String> = envelopes
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            JournalPayload::InvocationStarted { kind, .. } => {
                Some(invocation_kind_str(*kind).to_owned())
            }
            _ => None,
        })
        .collect();

    let schema_repair_starts = envelopes
        .iter()
        .filter(|e| matches!(e.payload, JournalPayload::SchemaRepairStarted { .. }))
        .count();

    let mut diagnostics = Vec::new();
    let final_result = output.final_result.as_ref().map(|meta| match &meta.body {
        FinalResultBody::Inline { value } => value.clone(),
        FinalResultBody::Artifact {
            artifact_id,
            logical_name,
            ..
        } => json!({
            "artifact_id": artifact_id.as_content_sha256(),
            "logical_name": logical_name,
        }),
    });

    if let Some(expected) = &fixture.expected_result {
        match &final_result {
            Some(actual) if actual == expected => {}
            Some(actual) => diagnostics.push(json!({
                "severity": "error",
                "code": "expected_result_mismatch",
                "message": format!("expected {expected}, got {actual}"),
            })),
            None => diagnostics.push(json!({
                "severity": "error",
                "code": "expected_result_missing",
                "message": "fixture expected a final result but none was recorded",
            })),
        }
    }

    if !fixture.expected_invocation_trace.is_empty() {
        let expected_kinds: Vec<&str> = fixture
            .expected_invocation_trace
            .iter()
            .map(|item| item.kind.as_str())
            .collect();
        let actual: Vec<&str> = invocation_kinds.iter().map(String::as_str).collect();
        if !is_subsequence(&actual, &expected_kinds) {
            diagnostics.push(json!({
                "severity": "error",
                "code": "invocation_trace_mismatch",
                "message": format!(
                    "expected invocation subsequence {expected_kinds:?}, got {invocation_kinds:?}"
                ),
            }));
        }
    }

    for report in &fixture.expected_reports {
        let found = envelopes.iter().any(|envelope| match &envelope.payload {
            JournalPayload::InvocationFinished { outcome, .. } => {
                outcome.details.get("report") == Some(report)
            }
            _ => false,
        });
        if !found {
            diagnostics.push(json!({
                "severity": "error",
                "code": "expected_report_missing",
                "message": format!("expected report not found in journal: {report}"),
            }));
        }
    }

    for expected in &fixture.expected_artifacts {
        let store_ok = output
            .artifacts
            .iter()
            .any(|meta| meta.logical_name == expected.logical_name);
        if !store_ok {
            diagnostics.push(json!({
                "severity": "error",
                "code": "expected_artifact_missing",
                "message": format!(
                    "expected artifact `{}` not present after fixture run",
                    expected.logical_name
                ),
            }));
        }
    }

    if use_real_delegate {
        let requests = harness.requests();
        if requests.len() >= 2 && !requests[1].tools.is_empty() {
            diagnostics.push(json!({
                "severity": "error",
                "code": "repair_tools_not_disabled",
                "message": format!(
                    "repair turn advertised tools: {:?}",
                    requests[1]
                        .tools
                        .iter()
                        .map(|t| t.name.as_str())
                        .collect::<Vec<_>>()
                ),
            }));
        }
        if fixture_expects_repair(fixture) {
            if schema_repair_starts != 1 {
                diagnostics.push(json!({
                    "severity": "error",
                    "code": "schema_repair_count",
                    "message": format!(
                        "expected exactly one schema repair start, got {schema_repair_starts}"
                    ),
                }));
            }
            if requests.len() < 2 {
                diagnostics.push(json!({
                    "severity": "error",
                    "code": "schema_repair_model_turns",
                    "message": format!(
                        "expected original + one repair model turn, got {}",
                        requests.len()
                    ),
                }));
            }
        }
    }

    if output.state != WorkflowState::Completed && fixture.expected_result.is_some() {
        diagnostics.push(json!({
            "severity": "error",
            "code": "non_completed_state",
            "message": format!("expected completed, got {}", output.state.as_str()),
        }));
    }

    let ok = diagnostics.is_empty();
    let report = FixtureRunReport {
        ok,
        name: definition.name.as_str().to_owned(),
        run_id: handle.run_id.as_str().to_owned(),
        state: output.state.as_str().to_owned(),
        final_result,
        diagnostics,
        invocation_kinds,
        schema_repair_starts,
        journal_path,
        session_dir,
    };
    Ok((report, session, runtime))
}

fn fixture_uses_real_delegate(fixture: &WorkflowFixture) -> bool {
    !fixture.model_turns.is_empty()
        || fixture
            .delegate_outcomes
            .iter()
            .any(|d| d.first_raw.is_some() || d.repair_raw.is_some())
}

fn fixture_expects_repair(fixture: &WorkflowFixture) -> bool {
    fixture.model_turns.len() >= 2
        || fixture
            .delegate_outcomes
            .iter()
            .any(|d| d.repair_raw.is_some())
}

fn model_turns_from_fixture(fixture: &WorkflowFixture) -> Vec<Vec<AiStreamEvent>> {
    if !fixture.model_turns.is_empty() {
        return fixture
            .model_turns
            .iter()
            .map(|turn| text_turn(&turn.text, Some((turn.input_tokens, turn.output_tokens))))
            .collect();
    }
    let mut turns = Vec::new();
    if let Some(delegate) = fixture
        .delegate_outcomes
        .iter()
        .find(|d| d.first_raw.is_some() || d.repair_raw.is_some())
    {
        if let Some(first) = &delegate.first_raw {
            turns.push(text_turn(first, Some((10, 20))));
        }
        if let Some(repair) = &delegate.repair_raw {
            turns.push(text_turn(repair, Some((5, 7))));
        }
    }
    turns
}

fn text_turn(text: &str, usage: Option<(u32, u32)>) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            id: format!("msg_{}", text.len()),
        },
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: usage.map(|(input_tokens, output_tokens)| TokenUsage {
                input_tokens,
                output_tokens,
                input_cache_read_tokens: 0,
                input_cache_write_tokens: 0,
            }),
        },
    ]
}

async fn commit_fixture_artifact(
    handle: &crate::workflow::WorkflowHandle,
    art: &FixtureArtifactSpec,
) -> Result<(), WorkflowError> {
    let kind = match art.kind.as_str() {
        "text" => ArtifactKind::Text,
        _ => ArtifactKind::Json,
    };
    let value = match kind {
        ArtifactKind::Text => ArtifactValue::Text(
            art.value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| art.value.to_string()),
        ),
        ArtifactKind::Json => ArtifactValue::Json(art.value.clone()),
    };
    handle
        .commit_artifact(&art.logical_name, kind, value, None)
        .await?;
    Ok(())
}

fn is_subsequence(haystack: &[&str], needle: &[&str]) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut n = 0;
    for item in haystack {
        if *item == needle[n] {
            n += 1;
            if n == needle.len() {
                return true;
            }
        }
    }
    false
}

fn invocation_kind_str(kind: WorkflowInvocationKind) -> &'static str {
    match kind {
        WorkflowInvocationKind::Phase => "phase",
        WorkflowInvocationKind::Log => "log",
        WorkflowInvocationKind::Delegate => "delegate",
        WorkflowInvocationKind::Swarm => "swarm",
        WorkflowInvocationKind::Verify => "verify",
        WorkflowInvocationKind::VerifyCommand => "verify_command",
        WorkflowInvocationKind::Report => "report",
        WorkflowInvocationKind::Fail => "fail",
        WorkflowInvocationKind::Tool => "tool",
    }
}

fn build_fixture_registry(
    tools: &Arc<Mutex<VecDeque<FixtureToolOutcome>>>,
    delegates: &Arc<Mutex<VecDeque<FixtureDelegateOutcome>>>,
    swarm: &Arc<Mutex<VecDeque<FixtureSwarmItemOutcome>>>,
    use_real_delegate: bool,
) -> ToolRegistry {
    let mut registry = if use_real_delegate {
        ToolRegistry::with_builtin_tools()
    } else {
        let mut reg = ToolRegistry::new();
        reg.register(SleepTool);
        reg
    };

    let tool_names: BTreeSet<String> = tools
        .lock()
        .expect("tool queue")
        .iter()
        .map(|t| t.name.clone())
        .collect();
    for name in tool_names {
        registry.register(ScriptedTool {
            name: name.clone(),
            queue: Arc::clone(tools),
        });
    }

    if !use_real_delegate {
        registry.register(ScriptedDelegateTool {
            queue: Arc::clone(delegates),
        });
        registry.register(ScriptedSwarmTool {
            queue: Arc::clone(swarm),
        });
    }

    registry
}

struct ScriptedTool {
    name: String,
    queue: Arc<Mutex<VecDeque<FixtureToolOutcome>>>,
}

impl Tool for ScriptedTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "fixture-scripted tool outcome"
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: Value) -> ToolFuture<'a> {
        let queue = Arc::clone(&self.queue);
        let name = self.name.clone();
        Box::pin(async move {
            let outcome = {
                let mut guard = queue.lock().expect("tool queue");
                guard
                    .iter()
                    .position(|item| item.name == name)
                    .and_then(|idx| guard.remove(idx))
            };
            match outcome {
                Some(item) => {
                    let result = if item.ok {
                        ToolResult::ok(item.content)
                    } else {
                        ToolResult::error(item.content)
                    };
                    Ok(result.with_details(item.details))
                }
                None => Ok(ToolResult::error(format!(
                    "fixture has no remaining outcome for tool `{name}`"
                ))),
            }
        })
    }
}

struct ScriptedDelegateTool {
    queue: Arc<Mutex<VecDeque<FixtureDelegateOutcome>>>,
}

impl Tool for ScriptedDelegateTool {
    fn name(&self) -> &str {
        "Delegate"
    }

    fn description(&self) -> &str {
        "fixture-scripted Delegate outcome"
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: Value) -> ToolFuture<'a> {
        let queue = Arc::clone(&self.queue);
        Box::pin(async move {
            let outcome = queue.lock().expect("delegate queue").pop_front();
            match outcome {
                Some(item) => {
                    let details = if item.details.is_null() {
                        json!({
                            "kind": "delegate",
                            "status": if item.ok { "completed" } else { "failed" },
                            "mode": "foreground",
                            "agent_id": "fixture_agent",
                        })
                    } else {
                        item.details
                    };
                    let result = if item.ok {
                        ToolResult::ok(item.summary)
                    } else {
                        ToolResult::error(item.summary)
                    };
                    Ok(result.with_details(details))
                }
                None => Ok(ToolResult::error(
                    "fixture has no remaining delegate outcome",
                )),
            }
        })
    }
}

struct ScriptedSwarmTool {
    queue: Arc<Mutex<VecDeque<FixtureSwarmItemOutcome>>>,
}

impl Tool for ScriptedSwarmTool {
    fn name(&self) -> &str {
        "DelegateSwarm"
    }

    fn description(&self) -> &str {
        "fixture-scripted DelegateSwarm outcome"
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: Value) -> ToolFuture<'a> {
        let queue = Arc::clone(&self.queue);
        Box::pin(async move {
            let item_count = input
                .get("items")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(1);
            let mut results = Vec::new();
            let mut all_ok = true;
            for index in 0..item_count {
                let item = queue.lock().expect("swarm queue").pop_front().unwrap_or(
                    FixtureSwarmItemOutcome {
                        ok: true,
                        summary: format!("fixture swarm item {index}"),
                        details: json!({}),
                    },
                );
                all_ok &= item.ok;
                results.push(json!({
                    "ok": item.ok,
                    "summary": item.summary,
                    "details": item.details,
                }));
            }
            let details = json!({
                "kind": "delegate_swarm",
                "status": if all_ok { "completed" } else { "failed" },
                "items": results,
            });
            let result = if all_ok {
                ToolResult::ok("swarm completed")
            } else {
                ToolResult::error("swarm failed")
            };
            Ok(result.with_details(details))
        })
    }
}

/// Resolve one ordinary built-in definition through the public paired path.
pub fn resolve_builtin_definition(
    name: &str,
    limits: &WorkflowLimits,
) -> Result<ResolvedWorkflowDefinition, WorkflowError> {
    let def = crate::workflow::builtins::builtin_workflow_definition(name).ok_or_else(|| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!("unknown builtin workflow `{name}`"),
        )
    })?;
    crate::workflow::definition::resolve_paired_definition(
        &def.name,
        &def.manifest_bytes,
        &def.source_bytes,
        crate::workflow::state::WorkflowSourceOrigin::Builtin,
        Some(format!("builtin://{}", def.name)),
        limits,
    )
}

/// Run a built-in against a deterministic fixture document.
pub async fn run_builtin_fixture(
    name: &str,
    fixture: &WorkflowFixture,
    limits: WorkflowLimits,
) -> Result<FixtureRunReport, WorkflowError> {
    let definition = resolve_builtin_definition(name, &limits)?;
    run_fixture(&definition, fixture, limits).await
}
