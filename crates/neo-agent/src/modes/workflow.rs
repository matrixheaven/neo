//! Headless `neo workflow` command family.
//!
//! Routes business logic through the session-shared `WorkflowDefinitionRegistry`,
//! `WorkflowRuntime`, and `WorkflowLaunchCoordinator`. This module never owns
//! durable run or definition state.

use std::{
    fs,
    io::IsTerminal as _,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, bail};
use neo_agent_core::workflow::{
    MANIFEST_SUFFIX, SOURCE_SUFFIX, WorkflowActor, WorkflowDefinitionRegistry, WorkflowError,
    WorkflowLaunchBinding, WorkflowLaunchCoordinator, WorkflowLaunchHosts, WorkflowLaunchIntent,
    WorkflowLaunchRequest, WorkflowSourceOrigin, WorkflowState, check_definition,
    load_fixture, resolve_paired_definition, run_fixture,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use neo_agent_core::workflow::journal::canonicalize_json;
use neo_agent_core::workflow::FinalResultBody;

use crate::{
    cli::{WorkflowCommand, WorkflowOutputFormat},
    config::AppConfig,
    modes::sessions,
};

pub async fn execute(command: WorkflowCommand, config: &AppConfig) -> anyhow::Result<String> {
    match command {
        WorkflowCommand::List { json } => list(config, json),
        WorkflowCommand::Check { target, json } => check(config, &target, json),
        WorkflowCommand::Test {
            target,
            case,
            json,
        } => test_fixture(config, &target, &case, json).await,
        WorkflowCommand::Run {
            name,
            args,
            args_file,
            output,
        } => {
            run(config, &name, args.as_deref(), args_file.as_deref(), output).await
        }
    }
}

// --- list ---------------------------------------------------------------

fn list(config: &AppConfig, json_output: bool) -> anyhow::Result<String> {
    let summaries = config
        .workflow_definitions
        .list(neo_agent_core::workflow::WorkflowListScope::Effective)
        .map_err(map_workflow_error)?;

    // Sort by display name, then canonical name.
    let mut sorted = summaries;
    sorted.sort_by(|a, b| {
        a.display_name
            .cmp(&b.display_name)
            .then_with(|| a.name.as_str().cmp(b.name.as_str()))
    });

    if json_output {
        let definitions: Vec<Value> = sorted
            .into_iter()
            .map(|item| {
                json!({
                    "name": item.name.as_str(),
                    "display_name": item.display_name,
                    "description": item.description,
                })
            })
            .collect();
        Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({ "workflows": definitions }))?
        ))
    } else {
        if sorted.is_empty() {
            return Ok("no workflows\n".to_owned());
        }
        let mut lines = Vec::with_capacity(sorted.len());
        for item in sorted {
            lines.push(format!(
                "{}\t{}\t{}",
                item.name.as_str(),
                item.display_name,
                item.description
            ));
        }
        Ok(format!("{}\n", lines.join("\n")))
    }
}

// --- check --------------------------------------------------------------

/// Read-only definition validation. Does not create runs or mutate the registry.
fn check(config: &AppConfig, target: &str, json_output: bool) -> anyhow::Result<String> {
    let report = match load_definition_for_validation(config, target) {
        Ok(definition) => check_definition(&definition),
        Err(error) => neo_agent_core::workflow::WorkflowCheckReport {
            ok: false,
            name: target.to_owned(),
            revision: None,
            source_origin: None,
            source_locator: None,
            diagnostics: vec![neo_agent_core::workflow::CheckDiagnostic {
                severity: neo_agent_core::workflow::CheckSeverity::Error,
                code: "load_failed".to_owned(),
                message: error.to_string(),
            }],
        },
    };

    if json_output {
        let body = report.to_json();
        Ok(format!("{}\n", serde_json::to_string_pretty(&body)?))
    } else if report.ok {
        Ok(format!(
            "ok\t{}\t{}\n",
            report.name,
            report.revision.as_deref().unwrap_or("")
        ))
    } else {
        let message = report
            .diagnostics
            .iter()
            .find(|d| d.severity == neo_agent_core::workflow::CheckSeverity::Error)
            .map_or("check failed", |d| d.message.as_str());
        bail!("workflow check failed: {message}");
    }
}

// --- test ---------------------------------------------------------------

/// Deterministic fixture harness. Never switches to live providers or shell.
async fn test_fixture(
    config: &AppConfig,
    target: &str,
    case: &Path,
    json_output: bool,
) -> anyhow::Result<String> {
    let fixture = load_fixture(case).map_err(map_workflow_error)?;
    if fixture.mode.supports_live() {
        bail!("fixture harness has no live execution mode");
    }

    let definition = load_definition_for_validation(config, target)?;
    let report = run_fixture(&definition, &fixture, config.workflow_definitions.limits())
        .await
        .map_err(map_workflow_error)?;

    if json_output {
        let body = json!({
            "ok": report.ok,
            "name": report.name,
            "case": case.display().to_string(),
            "run_id": report.run_id,
            "state": report.state,
            "final_result": report.final_result,
            "diagnostics": report.diagnostics,
            "invocation_kinds": report.invocation_kinds,
            "schema_repair_starts": report.schema_repair_starts,
        });
        Ok(format!("{}\n", serde_json::to_string_pretty(&body)?))
    } else if report.ok {
        Ok(format!(
            "ok\t{}\t{}\t{}\n",
            report.name, report.run_id, report.state
        ))
    } else {
        let message = report
            .diagnostics
            .first()
            .and_then(|d| d.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("fixture failed");
        bail!("workflow test failed: {message}");
    }
}

// --- run ----------------------------------------------------------------

async fn run(
    config: &AppConfig,
    name: &str,
    args_json: Option<&str>,
    args_file: Option<&Path>,
    output: WorkflowOutputFormat,
) -> anyhow::Result<String> {
    let definition = config
        .workflow_definitions
        .resolve(name)
        .map_err(map_workflow_error)?;
    let args = load_args_object(args_json, args_file)?;

    if let Some(schema) = &definition.compiled_input_schema {
        schema
            .validate_instance(&args)
            .map_err(|error| anyhow::anyhow!("args failed input_schema validation: {error}"))?;
    }

    // Create a session for the headless run.
    let created = sessions::create_new_session(config).await?;
    let session_dir = session_dir_from_wire(&created.wire_path)?;

    // Set up workflow dispatch so the Lua runner has real model/tool/providers.
    crate::modes::run::setup_workflow_dispatch(config, &session_dir).await?;

    let schema_sha256 = definition
        .input_schema
        .as_ref()
        .map(|schema| {
            format!(
                "{:x}",
                Sha256::digest(canonicalize_json(schema).to_string().as_bytes())
            )
        })
        .unwrap_or_default();

    let request = WorkflowLaunchRequest {
        name: definition.name.as_str().to_owned(),
        description: definition.description.clone(),
        phases: definition.phases.clone(),
        script: definition.lua_source.clone(),
        args,
        launch_source: format!("cli:workflow-run ({})", config.permission_mode.label()),
        parent_run_id: None,
        output_schema: Some(definition.output_schema.clone()),
        display_name: Some(definition.display_name.clone()),
        input_schema: definition.input_schema.clone(),
        definition_origin: Some(definition.source_origin),
        inline_unsaved: false,
    };
    let intent = WorkflowLaunchIntent::from_parts(
        request,
        WorkflowLaunchBinding {
            session_identity: session_dir.display().to_string(),
            workspace_identity: config.project_dir.display().to_string(),
            actor: WorkflowActor::Human,
            permission_mode: config.permission_mode,
            parent_lineage: None,
            compiled_input_schema: definition.compiled_input_schema.clone(),
            schema_sha256,
        },
    );

    let outcome = WorkflowLaunchCoordinator
        .launch(
            &intent,
            WorkflowLaunchHosts {
                runtime: &config.workflow_runtime,
                background_tasks: &config.background_tasks,
                session_dir: &session_dir,
            },
        )
        .await
        .map_err(map_workflow_error)?;

    let run_id = outcome.handle.run_id.as_str().to_owned();
    let is_tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();

    match output {
        WorkflowOutputFormat::Jsonl => stream_run(&outcome.handle, &run_id, is_tty).await,
        _ => wait_run(&outcome.handle, &run_id, output, is_tty).await,
    }
}

/// Stream JSONL events until terminal state.
async fn stream_run(
    handle: &neo_agent_core::workflow::WorkflowHandle,
    run_id: &str,
    is_tty: bool,
) -> anyhow::Result<String> {
    let mut last_state = WorkflowState::Queued;
    let mut last_phase: Option<String> = None;

    // Emit started event.
    emit_jsonl(&json!({
        "type": "started",
        "run_id": run_id,
        "state": "queued",
    }));

    loop {
        let snapshot = handle.snapshot().await;
        let output = handle.output().await.map_err(map_workflow_error)?;

        // Emit state change if different.
        if snapshot.state != last_state {
            last_state = snapshot.state;
            if matches!(last_state, WorkflowState::AwaitingUser) && !is_tty {
                // Non-interactive: emit awaiting_user and exit 3.
                let pending = handle.pending_user_input().await.map_err(map_workflow_error)?;
                emit_jsonl(&json!({
                    "type": "awaiting_user",
                    "run_id": run_id,
                    "state": "awaiting_user",
                    "task_id": handle.run_id.0,
                    "prompt": pending.as_ref().and_then(|p| Some(p.prompt.as_str())),
                    "next_action": "open /tasks to answer in an interactive session",
                }));
                std::process::exit(3);
            }

            if last_state.is_terminal() {
                emit_jsonl(&json!({
                    "type": "terminal",
                    "run_id": run_id,
                    "state": last_state.as_str(),
                    "terminal_reason": output.terminal_reason,
                    "final_result": output.final_result.as_ref().and_then(|r| inline_value(&r.body)),
                }));
                return exit_for_state(last_state);
            }

            emit_jsonl(&json!({
                "type": "state_changed",
                "run_id": run_id,
                "state": last_state.as_str(),
            }));
        }

        // Emit phase change.
        if snapshot.current_phase != last_phase {
            last_phase = snapshot.current_phase.clone();
            if let Some(ref phase) = last_phase {
                emit_jsonl(&json!({
                    "type": "step",
                    "run_id": run_id,
                    "phase": phase,
                    "invocation_count": snapshot.invocation_count,
                    "failure_count": snapshot.failure_count,
                }));
            }
        }

        // Handle TTY awaiting-user.
        if snapshot.state == WorkflowState::AwaitingUser && is_tty {
            let pending = handle.pending_user_input().await.map_err(map_workflow_error)?;
            if let Some(pending) = pending {
                eprintln!("\nWorkflow needs your input:");
                eprintln!("  {}", pending.prompt);
                eprintln!("  Schema: {}", serde_json::to_string_pretty(&pending.answer_schema).unwrap_or_default());
                eprint!("Answer (JSON value): ");
                use std::io::Write as _;
                let _ = std::io::stderr().flush();

                let mut answer = String::new();
                if std::io::stdin().read_line(&mut answer).is_err() {
                    handle.stop(WorkflowActor::Human).await.map_err(map_workflow_error)?;
                    std::process::exit(130);
                }
                let answer = answer.trim().to_owned();
                let value: Value = serde_json::from_str(&answer)
                    .context("answer must be valid JSON")?;
                handle
                    .answer(&pending.request_id, value, WorkflowActor::Human)
                    .await
                    .map_err(map_workflow_error)?;
                continue;
            }
        }

        if snapshot.state.is_terminal() {
            let output = handle.output().await.map_err(map_workflow_error)?;
            emit_jsonl(&json!({
                "type": "terminal",
                "run_id": run_id,
                "state": snapshot.state.as_str(),
                "terminal_reason": output.terminal_reason,
                "final_result": output.final_result.as_ref().and_then(|r| inline_value(&r.body)),
            }));
            return exit_for_state(snapshot.state);
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Wait for terminal state and format result.
async fn wait_run(
    handle: &neo_agent_core::workflow::WorkflowHandle,
    run_id: &str,
    output: WorkflowOutputFormat,
    is_tty: bool,
) -> anyhow::Result<String> {
    loop {
        let snapshot = handle.snapshot().await;

        // Handle awaiting-user for TTY.
        if snapshot.state == WorkflowState::AwaitingUser && is_tty {
            let pending = handle.pending_user_input().await.map_err(map_workflow_error)?;
            if let Some(pending) = pending {
                eprintln!("\nWorkflow needs your input:");
                eprintln!("  {}", pending.prompt);
                eprintln!("  Schema: {}", serde_json::to_string_pretty(&pending.answer_schema).unwrap_or_default());
                eprint!("Answer (JSON value): ");
                use std::io::Write as _;
                let _ = std::io::stderr().flush();

                let mut answer = String::new();
                if std::io::stdin().read_line(&mut answer).is_err() {
                    handle.stop(WorkflowActor::Human).await.map_err(map_workflow_error)?;
                    std::process::exit(130);
                }
                let answer = answer.trim().to_owned();
                let value: Value = serde_json::from_str(&answer)
                    .context("answer must be valid JSON")?;
                handle
                    .answer(&pending.request_id, value, WorkflowActor::Human)
                    .await
                    .map_err(map_workflow_error)?;
                continue;
            }
        }

        // Non-interactive awaiting-user: exit 3.
        if snapshot.state == WorkflowState::AwaitingUser && !is_tty {
            let pending = handle.pending_user_input().await.map_err(map_workflow_error)?;
            let body = json!({
                "run_id": run_id,
                "state": "awaiting_user",
                "task_id": handle.run_id.0,
                "prompt": pending.as_ref().and_then(|p| Some(p.prompt.as_str())),
                "next_action": "open /tasks to answer in an interactive session",
            });
            match output {
                WorkflowOutputFormat::Json => {
                    return Ok(format!("{}\n", serde_json::to_string_pretty(&body)?));
                }
                _ => {
                    eprintln!(
                        "Workflow requires human input. Open /tasks in an interactive session."
                    );
                    std::process::exit(3);
                }
            }
        }

        if snapshot.state.is_terminal() {
            let final_output = handle.output().await.map_err(map_workflow_error)?;
            let display_name = final_output
                .metadata
                .display_name
                .as_deref()
                .unwrap_or(final_output.metadata.name.as_str());
            let state = final_output.state;

            match output {
                WorkflowOutputFormat::Text => {
                    if state == WorkflowState::Completed {
                        let mut lines = vec![format!("{display_name}")];
                        if let Some(ref reason) = final_output.terminal_reason {
                            lines.push(format!("  result: {reason}"));
                        }
                        if let Some(ref result) = final_output.final_result {
                            if let FinalResultBody::Inline { value } = &result.body {
                                lines.push(format!(
                                    "  final: {}",
                                    serde_json::to_string_pretty(value)
                                        .unwrap_or_else(|_| format!("{:?}", value))
                                ));
                            }
                        }
                        return Ok(format!("{}\n", lines.join("\n")));
                    } else {
                        let reason = final_output
                            .terminal_reason
                            .as_deref()
                            .unwrap_or(state.as_str());
                        bail!("workflow {display_name}: {reason}");
                    }
                }
                WorkflowOutputFormat::Json => {
                    let body = json!({
                        "run_id": run_id,
                        "display_name": display_name,
                        "state": state.as_str(),
                        "terminal_reason": final_output.terminal_reason,
                        "final_result": final_output.final_result.as_ref().and_then(|r| inline_value(&r.body)),
                    });
                    return Ok(format!("{}\n", serde_json::to_string_pretty(&body)?));
                }
                _ => unreachable!("Jsonl handled by stream_run"),
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn exit_for_state(state: WorkflowState) -> anyhow::Result<String> {
    let code = match state {
        WorkflowState::Completed => 0,
        WorkflowState::Failed | WorkflowState::Cancelled => 1,
        WorkflowState::ResourceLimited => 4,
        _ => 0,
    };
    std::process::exit(code);
}

/// Extract the inline value from a FinalResultBody, if present.
fn inline_value(body: &FinalResultBody) -> Option<&Value> {
    match body {
        FinalResultBody::Inline { value } => Some(value),
        FinalResultBody::Artifact { .. } => None,
    }
}

fn emit_jsonl(value: &Value) {
    println!("{}", serde_json::to_string(value).unwrap_or_default());
}

// --- helpers ------------------------------------------------------------

fn map_workflow_error(error: WorkflowError) -> anyhow::Error {
    anyhow::anyhow!("{error}")
}

fn session_dir_from_wire(wire_path: &Path) -> anyhow::Result<PathBuf> {
    wire_path
        .parent() // agents/main
        .and_then(Path::parent) // agents
        .and_then(Path::parent) // session
        .map(Path::to_path_buf)
        .with_context(|| {
            format!(
                "failed to resolve session directory from {}",
                wire_path.display()
            )
        })
}

fn load_args_object(args_json: Option<&str>, args_file: Option<&Path>) -> anyhow::Result<Value> {
    let raw = match (args_json, args_file) {
        (None, None) => return Ok(json!({})),
        (Some(text), None) => text.to_owned(),
        (None, Some(path)) => fs::read_to_string(path)
            .with_context(|| format!("failed to read args file {}", path.display()))?,
        (Some(_), Some(_)) => bail!("--args and --args-file are mutually exclusive"),
    };
    let value: Value = serde_json::from_str(&raw).context("args must be valid JSON")?;
    if !value.is_object() {
        bail!("args must be a JSON object");
    }
    Ok(value)
}

pub(crate) fn load_definition_for_validation(
    config: &AppConfig,
    target: &str,
) -> anyhow::Result<neo_agent_core::workflow::ResolvedWorkflowDefinition> {
    let path = Path::new(target);
    if path.exists() {
        return load_pair_from_path(path, &config.workflow_definitions);
    }
    config
        .workflow_definitions
        .resolve(target)
        .map_err(map_workflow_error)
}

pub(crate) fn load_pair_from_path(
    path: &Path,
    registry: &WorkflowDefinitionRegistry,
) -> anyhow::Result<neo_agent_core::workflow::ResolvedWorkflowDefinition> {
    let (stem, manifest_path, source_path) = resolve_pair_paths(path)?;
    let manifest_bytes = fs::read(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let source_bytes = fs::read(&source_path)
        .with_context(|| format!("failed to read {}", source_path.display()))?;
    resolve_paired_definition(
        &stem,
        &manifest_bytes,
        &source_bytes,
        WorkflowSourceOrigin::User,
        Some(manifest_path.display().to_string()),
        &registry.limits(),
    )
    .map_err(map_workflow_error)
}

fn resolve_pair_paths(path: &Path) -> anyhow::Result<(String, PathBuf, PathBuf)> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| format!("invalid definition path {}", path.display()))?;
    if let Some(stem) = file_name.strip_suffix(MANIFEST_SUFFIX) {
        let source = path.with_file_name(format!("{stem}{SOURCE_SUFFIX}"));
        return Ok((stem.to_owned(), path.to_path_buf(), source));
    }
    if let Some(stem) = file_name.strip_suffix(SOURCE_SUFFIX) {
        let manifest = path.with_file_name(format!("{stem}{MANIFEST_SUFFIX}"));
        return Ok((stem.to_owned(), manifest, path.to_path_buf()));
    }
    bail!(
        "definition path must end with `{SOURCE_SUFFIX}` or `{MANIFEST_SUFFIX}`: {}",
        path.display()
    );
}
