//! Headless `neo workflow` command family.
//!
//! Routes business logic exclusively through the session-shared
//! `WorkflowDefinitionRegistry` and `WorkflowRuntime` on [`AppConfig`].
//! This module never owns durable run or definition state.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, bail};
use neo_agent_core::workflow::journal::{JournalPayload, canonicalize_json, collect_journal_v2};
use neo_agent_core::workflow::runtime::{LinkedRunRequest, compute_prefix_digest_v2};
use neo_agent_core::workflow::{
    LaunchAuthorizationMode, MANIFEST_SUFFIX, RetentionPolicy, RetentionSubject, SOURCE_SUFFIX,
    WorkflowActor, WorkflowCheckpoint, WorkflowDefinitionRegistry, WorkflowError, WorkflowId,
    WorkflowLaunchBinding, WorkflowLaunchCoordinator, WorkflowLaunchHosts, WorkflowLaunchIntent,
    WorkflowLaunchRequest, WorkflowListScope, WorkflowOutput, WorkflowRunMetadata,
    WorkflowSaveRequest, WorkflowSaveScope, WorkflowSourceOrigin, WorkflowState, check_definition,
    journal, load_fixture, preview_mark_sweep, resolve_paired_definition, run_fixture,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    cli::{
        WorkflowCommand, WorkflowListScopeArg, WorkflowOutputFormat, WorkflowRunOutputFormat,
        WorkflowSaveScopeArg,
    },
    config::{AppConfig, neo_home, workspace_sessions_dir},
    modes::sessions,
};

pub async fn execute(command: WorkflowCommand, config: &AppConfig) -> anyhow::Result<String> {
    match command {
        WorkflowCommand::List { scope, output } => list(config, scope, output),
        WorkflowCommand::Show {
            name,
            scope,
            output,
        } => show(config, &name, scope, output),
        WorkflowCommand::Check { target, output } => check(config, &target, output),
        WorkflowCommand::Test {
            target,
            case,
            output,
        } => test_fixture(config, &target, &case, output).await,
        WorkflowCommand::Run {
            name,
            args_json,
            args_file,
            detach,
            output,
        } => {
            run(
                config,
                &name,
                args_json.as_deref(),
                args_file.as_deref(),
                detach,
                output,
            )
            .await
        }
        WorkflowCommand::Save {
            target,
            scope,
            name,
            force,
            output,
        } => save(config, &target, scope, name.as_deref(), force, output).await,
        WorkflowCommand::Answer {
            run,
            request_id,
            json: json_arg,
            file,
            output,
        } => {
            answer(
                config,
                &run,
                &request_id,
                json_arg.as_deref(),
                file.as_deref(),
                output,
            )
            .await
        }
        WorkflowCommand::Fork {
            run,
            checkpoint,
            name,
            args_json,
            args_file,
            output,
        } => {
            fork(
                config,
                &run,
                checkpoint,
                name.as_deref(),
                args_json.as_deref(),
                args_file.as_deref(),
                output,
            )
            .await
        }
        WorkflowCommand::Prune {
            older_than,
            max_bytes,
            dry_run,
            yes,
            output,
        } => {
            prune(
                config,
                older_than.as_deref(),
                max_bytes.as_deref(),
                dry_run,
                yes,
                output,
            )
            .await
        }
    }
}

fn list_scope(arg: Option<WorkflowListScopeArg>) -> WorkflowListScope {
    match arg.unwrap_or(WorkflowListScopeArg::Effective) {
        WorkflowListScopeArg::Builtin => WorkflowListScope::Builtin,
        WorkflowListScopeArg::User => WorkflowListScope::User,
        WorkflowListScopeArg::Project => WorkflowListScope::Project,
        WorkflowListScopeArg::Effective => WorkflowListScope::Effective,
    }
}

fn save_scope(arg: WorkflowSaveScopeArg) -> WorkflowSaveScope {
    match arg {
        WorkflowSaveScopeArg::User => WorkflowSaveScope::User,
        WorkflowSaveScopeArg::Project => WorkflowSaveScope::Project,
    }
}

fn list(
    config: &AppConfig,
    scope: Option<WorkflowListScopeArg>,
    output: WorkflowOutputFormat,
) -> anyhow::Result<String> {
    let summaries = config
        .workflow_definitions
        .list(list_scope(scope))
        .map_err(map_workflow_error)?;
    match output {
        WorkflowOutputFormat::Text => {
            if summaries.is_empty() {
                return Ok("no workflows\n".to_owned());
            }
            let mut lines = Vec::with_capacity(summaries.len());
            for item in summaries {
                lines.push(format!(
                    "{}\t{}\t{}\t{}",
                    item.name.as_str(),
                    item.source_origin.as_str(),
                    item.revision.as_str(),
                    item.display_name
                ));
            }
            Ok(format!("{}\n", lines.join("\n")))
        }
        WorkflowOutputFormat::Json => {
            let definitions: Vec<Value> = summaries
                .into_iter()
                .map(|item| {
                    json!({
                        "name": item.name.as_str(),
                        "display_name": item.display_name,
                        "description": item.description,
                        "revision": item.revision.as_str(),
                        "source_origin": item.source_origin.as_str(),
                        "source_locator": item.source_locator,
                    })
                })
                .collect();
            Ok(format!(
                "{}\n",
                serde_json::to_string_pretty(&json!({ "definitions": definitions }))?
            ))
        }
    }
}

fn show(
    config: &AppConfig,
    name: &str,
    scope: Option<WorkflowListScopeArg>,
    output: WorkflowOutputFormat,
) -> anyhow::Result<String> {
    // Effective resolve is authoritative; optional scope is reserved for
    // future filtered show and currently ignored without changing precedence.
    let _ = scope;
    let definition = config
        .workflow_definitions
        .resolve(name)
        .map_err(map_workflow_error)?;
    match output {
        WorkflowOutputFormat::Text => Ok(format!(
            "name:\t{}\ndisplay_name:\t{}\ndescription:\t{}\nrevision:\t{}\nsource_origin:\t{}\nsource_locator:\t{}\nsource_sha256:\t{}\nphases:\t{}\n",
            definition.name.as_str(),
            definition.display_name,
            definition.description,
            definition.revision.as_str(),
            definition.source_origin.as_str(),
            definition.source_locator.as_deref().unwrap_or(""),
            definition.source_sha256,
            definition
                .phases
                .iter()
                .map(|phase| phase.id.as_str())
                .collect::<Vec<_>>()
                .join(","),
        )),
        WorkflowOutputFormat::Json => {
            let phases: Vec<Value> = definition
                .phases
                .iter()
                .map(|phase| {
                    json!({
                        "id": phase.id,
                        "description": phase.description,
                    })
                })
                .collect();
            Ok(format!(
                "{}\n",
                serde_json::to_string_pretty(&json!({
                    "name": definition.name.as_str(),
                    "display_name": definition.display_name,
                    "description": definition.description,
                    "revision": definition.revision.as_str(),
                    "source_origin": definition.source_origin.as_str(),
                    "source_locator": definition.source_locator,
                    "source_sha256": definition.source_sha256,
                    "definition_format_version": definition.definition_format_version,
                    "phases": phases,
                    "input_schema": definition.input_schema,
                    "output_schema": definition.output_schema,
                    "lua_source_len": definition.lua_source.len(),
                }))?
            ))
        }
    }
}

/// Read-only definition validation. Does not create runs or mutate the registry.
fn check(config: &AppConfig, target: &str, output: WorkflowOutputFormat) -> anyhow::Result<String> {
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
    let body = report.to_json();

    match output {
        WorkflowOutputFormat::Text => {
            if report.ok {
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
                    .map(|d| d.message.as_str())
                    .unwrap_or("check failed");
                bail!("workflow check failed: {message}");
            }
        }
        WorkflowOutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(&body)?)),
    }
}

/// Deterministic fixture harness. Never switches to live providers or shell.
async fn test_fixture(
    config: &AppConfig,
    target: &str,
    case: &Path,
    output: WorkflowOutputFormat,
) -> anyhow::Result<String> {
    let fixture = load_fixture(case).map_err(map_workflow_error)?;
    if fixture.mode.supports_live() {
        bail!("fixture harness has no live execution mode");
    }

    let definition = load_definition_for_validation(config, target)?;
    let report = run_fixture(&definition, &fixture, config.workflow_definitions.limits())
        .await
        .map_err(map_workflow_error)?;

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

    match output {
        WorkflowOutputFormat::Text => {
            if report.ok {
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
        WorkflowOutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(&body)?)),
    }
}

async fn run(
    config: &AppConfig,
    name: &str,
    args_json: Option<&str>,
    args_file: Option<&Path>,
    detach: bool,
    output: WorkflowRunOutputFormat,
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

    let created = sessions::create_new_session(config).await?;
    let session_dir = session_dir_from_wire(&created.wire_path)?;

    ensure_headless_runner(config);

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
    };
    let intent = WorkflowLaunchIntent::from_parts(
        request,
        WorkflowLaunchBinding {
            session_identity: session_dir.display().to_string(),
            workspace_identity: config.project_dir.display().to_string(),
            launch_nonce: uuid::Uuid::new_v4().to_string(),
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
                capability: &config.workflow_capability,
                background_tasks: &config.background_tasks,
                session_dir: &session_dir,
            },
            LaunchAuthorizationMode::Headless,
        )
        .await
        .map_err(map_workflow_error)?;

    let run_id = outcome.handle.run_id.as_str().to_owned();
    let task_id = outcome.task_id.clone();

    if detach {
        let snapshot = outcome.handle.snapshot().await;
        return format_run_result(
            &run_id,
            &task_id,
            snapshot.state,
            true,
            None,
            output,
            &[json!({
                "type": "detached",
                "run_id": run_id,
                "task_id": task_id,
                "state": snapshot.state.as_str(),
                "detached": true,
            })],
        );
    }

    let mut events = vec![json!({
        "type": "started",
        "run_id": run_id,
        "task_id": task_id,
        "state": "queued",
    })];
    let final_output = wait_for_terminal(&outcome.handle).await?;
    events.push(json!({
        "type": "finished",
        "run_id": run_id,
        "task_id": task_id,
        "state": final_output.state.as_str(),
        "terminal_reason": final_output.terminal_reason,
        "detached": false,
    }));
    format_run_result(
        &run_id,
        &task_id,
        final_output.state,
        false,
        Some(&final_output),
        output,
        &events,
    )
}

async fn save(
    config: &AppConfig,
    target: &str,
    scope: WorkflowSaveScopeArg,
    name_override: Option<&str>,
    force: bool,
    output: WorkflowOutputFormat,
) -> anyhow::Result<String> {
    let request = build_save_request(config, target, name_override).await?;
    let resolved = config
        .workflow_definitions
        .save(save_scope(scope), &request, force)
        .map_err(map_workflow_error)?;
    let scope_label = match scope {
        WorkflowSaveScopeArg::User => "user",
        WorkflowSaveScopeArg::Project => "project",
    };
    match output {
        WorkflowOutputFormat::Text => Ok(format!(
            "saved\t{}\t{}\t{}\n",
            resolved.name.as_str(),
            scope_label,
            resolved.revision.as_str(),
        )),
        WorkflowOutputFormat::Json => Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "name": resolved.name.as_str(),
                "scope": scope_label,
                "revision": resolved.revision.as_str(),
                "source_origin": resolved.source_origin.as_str(),
                "source_locator": resolved.source_locator,
            }))?
        )),
    }
}

async fn answer(
    config: &AppConfig,
    run_ref: &str,
    request_id: &str,
    json_value: Option<&str>,
    file: Option<&Path>,
    output: WorkflowOutputFormat,
) -> anyhow::Result<String> {
    let value = load_json_value(json_value, file)?;
    let located = locate_run(config, run_ref)?;
    rehydrate_session_runs(config, &located.session_dir).await?;
    let run_id = WorkflowId::from_existing(located.run_id.clone());
    config
        .workflow_runtime
        .answer(&run_id, request_id, value, WorkflowActor::Human)
        .await
        .map_err(map_workflow_error)?;
    match output {
        WorkflowOutputFormat::Text => Ok(format!("answered\t{}\t{}\n", located.run_id, request_id)),
        WorkflowOutputFormat::Json => Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "run_id": located.run_id,
                "request_id": request_id,
                "status": "answered",
            }))?
        )),
    }
}

async fn fork(
    config: &AppConfig,
    run_ref: &str,
    checkpoint_seq: u64,
    name_override: Option<&str>,
    args_json: Option<&str>,
    args_file: Option<&Path>,
    output: WorkflowOutputFormat,
) -> anyhow::Result<String> {
    let located = locate_run(config, run_ref)?;
    rehydrate_session_runs(config, &located.session_dir).await?;
    let parent_id = WorkflowId::from_existing(located.run_id.clone());
    let parent_meta = journal::read_run_metadata(&located.run_dir).map_err(map_workflow_error)?;
    let args = if args_json.is_some() || args_file.is_some() {
        load_args_object(args_json, args_file)?
    } else {
        parent_meta.args.clone()
    };

    let journal_path = located.run_dir.join("journal.jsonl");
    let envelopes =
        collect_journal_v2(&journal_path, Some(&parent_id)).map_err(map_workflow_error)?;
    let digest =
        compute_prefix_digest_v2(&envelopes, checkpoint_seq).map_err(map_workflow_error)?;
    let checkpoint = WorkflowCheckpoint::new(parent_id.clone(), checkpoint_seq, digest)
        .map_err(map_workflow_error)?;

    config.workflow_capability.grant();
    let reservation = config
        .workflow_capability
        .reserve()
        .ok_or_else(|| anyhow::anyhow!("failed to reserve launch authorization for linked run"))?;

    ensure_headless_runner(config);

    let launch = WorkflowLaunchRequest {
        name: name_override
            .unwrap_or(parent_meta.name.as_str())
            .to_owned(),
        description: parent_meta.description.clone(),
        phases: parent_meta.phases.clone(),
        script: parent_meta.script.clone(),
        args,
        launch_source: format!("cli:workflow-fork ({})", config.permission_mode.label()),
        parent_run_id: Some(parent_id.clone()),
        output_schema: parent_meta.output_schema.clone(),
    };
    let handle = config
        .workflow_runtime
        .create_linked_run(
            &located.session_dir,
            LinkedRunRequest {
                parent_run_id: parent_id,
                checkpoint: Some(checkpoint),
                link_reason: "cli_fork".to_owned(),
                launch,
            },
            Some(reservation),
        )
        .await
        .map_err(map_workflow_error)?;

    let child_id = handle.run_id.as_str().to_owned();
    match output {
        WorkflowOutputFormat::Text => Ok(format!(
            "forked\t{}\t->\t{}\tcheckpoint={checkpoint_seq}\n",
            located.run_id, child_id
        )),
        WorkflowOutputFormat::Json => Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "parent_run_id": located.run_id,
                "run_id": child_id,
                "checkpoint": checkpoint_seq,
            }))?
        )),
    }
}

async fn prune(
    config: &AppConfig,
    older_than: Option<&str>,
    max_bytes: Option<&str>,
    dry_run_flag: bool,
    yes: bool,
    output: WorkflowOutputFormat,
) -> anyhow::Result<String> {
    // Default is dry-run. `--yes` is required for deletion.
    let perform_delete = yes && !dry_run_flag;
    let min_age_ms = older_than
        .map(parse_duration_ms)
        .transpose()
        .context("invalid --older-than duration")?;
    let reclaim_target_bytes = max_bytes
        .map(parse_byte_size)
        .transpose()
        .context("invalid --max-bytes value")?;

    let subjects = collect_retention_subjects(config)?;
    let preview = preview_mark_sweep(
        &subjects,
        &RetentionPolicy {
            min_age_ms,
            reclaim_target_bytes,
        },
    );

    let mut deleted = Vec::new();
    if perform_delete {
        for candidate in &preview.candidates {
            if let Some(run_dir) = find_run_dir_anywhere(config, candidate.run_id.as_str())? {
                fs::remove_dir_all(&run_dir).with_context(|| {
                    format!("failed to delete workflow run {}", run_dir.display())
                })?;
                config
                    .workflow_runtime
                    .admission()
                    .release_storage_owner(candidate.run_id.as_str());
                deleted.push(candidate.run_id.as_str().to_owned());
            }
        }
    }

    let candidates: Vec<Value> = preview
        .candidates
        .iter()
        .map(|subject| {
            json!({
                "run_id": subject.run_id.as_str(),
                "state": subject.state.as_str(),
                "bytes": subject.bytes,
                "age_ms": subject.age_ms,
            })
        })
        .collect();
    let excluded: Vec<Value> = preview
        .excluded
        .iter()
        .map(|(subject, reason)| {
            json!({
                "run_id": subject.run_id.as_str(),
                "state": subject.state.as_str(),
                "reason": reason.as_str(),
            })
        })
        .collect();

    let report = json!({
        "dry_run": !perform_delete,
        "reclaimable_bytes": preview.reclaimable_bytes,
        "candidates": candidates,
        "excluded": excluded,
        "deleted": deleted,
    });

    match output {
        WorkflowOutputFormat::Text => {
            let mode = if perform_delete { "delete" } else { "dry-run" };
            let mut lines = vec![format!(
                "{mode}\treclaimable_bytes={}\tcandidates={}",
                preview.reclaimable_bytes,
                preview.candidates.len()
            )];
            for subject in &preview.candidates {
                lines.push(format!(
                    "candidate\t{}\t{}\t{}",
                    subject.run_id.as_str(),
                    subject.state.as_str(),
                    subject.bytes
                ));
            }
            for id in &deleted {
                lines.push(format!("deleted\t{id}"));
            }
            Ok(format!("{}\n", lines.join("\n")))
        }
        WorkflowOutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(&report)?)),
    }
}

// --- helpers ----------------------------------------------------------------

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

fn ensure_headless_runner(config: &AppConfig) {
    // Bind only when unbound so interactive/session dispatch remains sole owner
    // when already wired. Headless pure scripts complete with a minimal final.
    let _ =
        config
            .workflow_runtime
            .bind_runner_if_unbound(|handle, metadata, _session| async move {
                let _ = handle.enter_running_for_direct_execution().await;
                let _ = handle
                    .accept_final_lua_result(
                        json!({ "ok": true, "name": metadata.name }),
                        None,
                        None,
                    )
                    .await;
                Ok(())
            });
}

async fn wait_for_terminal(
    handle: &neo_agent_core::workflow::WorkflowHandle,
) -> anyhow::Result<WorkflowOutput> {
    loop {
        let output = handle.output().await.map_err(map_workflow_error)?;
        if output.state.is_terminal() {
            return Ok(output);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn format_run_result(
    run_id: &str,
    task_id: &str,
    state: WorkflowState,
    detached: bool,
    final_output: Option<&WorkflowOutput>,
    output: WorkflowRunOutputFormat,
    events: &[Value],
) -> anyhow::Result<String> {
    match output {
        WorkflowRunOutputFormat::Text => {
            if detached {
                Ok(format!(
                    "run_id={run_id}\ttask_id={task_id}\tstate={}\tdetached=true\n",
                    state.as_str()
                ))
            } else {
                Ok(format!(
                    "run_id={run_id}\ttask_id={task_id}\tstate={}\n",
                    state.as_str()
                ))
            }
        }
        WorkflowRunOutputFormat::Json => {
            let mut body = json!({
                "run_id": run_id,
                "task_id": task_id,
                "state": state.as_str(),
                "detached": detached,
            });
            if let Some(out) = final_output {
                body["terminal_reason"] = json!(out.terminal_reason);
                body["final_result"] = serde_json::to_value(&out.final_result)?;
            }
            Ok(format!("{}\n", serde_json::to_string_pretty(&body)?))
        }
        WorkflowRunOutputFormat::Jsonl => {
            let mut buf = String::new();
            for event in events {
                buf.push_str(&serde_json::to_string(event)?);
                buf.push('\n');
            }
            if buf.is_empty() {
                let line = json!({
                    "type": if detached { "detached" } else { "finished" },
                    "run_id": run_id,
                    "task_id": task_id,
                    "state": state.as_str(),
                    "detached": detached,
                });
                buf.push_str(&serde_json::to_string(&line)?);
                buf.push('\n');
            }
            Ok(buf)
        }
    }
}

fn load_args_object(args_json: Option<&str>, args_file: Option<&Path>) -> anyhow::Result<Value> {
    let raw = match (args_json, args_file) {
        (None, None) => return Ok(json!({})),
        (Some(text), None) => text.to_owned(),
        (None, Some(path)) => fs::read_to_string(path)
            .with_context(|| format!("failed to read args file {}", path.display()))?,
        (Some(_), Some(_)) => bail!("--args-json and --args-file are mutually exclusive"),
    };
    let value: Value = serde_json::from_str(&raw).context("args must be valid JSON")?;
    if !value.is_object() {
        bail!("args must be a JSON object");
    }
    Ok(value)
}

fn load_json_value(json_value: Option<&str>, file: Option<&Path>) -> anyhow::Result<Value> {
    let raw = match (json_value, file) {
        (Some(text), None) => text.to_owned(),
        (None, Some(path)) => fs::read_to_string(path)
            .with_context(|| format!("failed to read answer file {}", path.display()))?,
        (None, None) => bail!("answer requires --json or --file"),
        (Some(_), Some(_)) => bail!("--json and --file are mutually exclusive"),
    };
    serde_json::from_str(&raw).context("answer value must be valid JSON")
}

fn load_definition_for_validation(
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

fn load_pair_from_path(
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

async fn build_save_request(
    config: &AppConfig,
    target: &str,
    name_override: Option<&str>,
) -> anyhow::Result<WorkflowSaveRequest> {
    let path = Path::new(target);
    if path.exists() {
        let definition = load_pair_from_path(path, &config.workflow_definitions)?;
        return Ok(WorkflowSaveRequest {
            name: name_override.unwrap_or(definition.name.as_str()).to_owned(),
            display_name: definition.display_name,
            description: definition.description,
            phases: definition.phases,
            lua_source: definition.lua_source,
            input_schema: definition.input_schema,
            output_schema: definition.output_schema,
        });
    }

    if let Ok(definition) = config.workflow_definitions.resolve(target) {
        return Ok(WorkflowSaveRequest {
            name: name_override.unwrap_or(definition.name.as_str()).to_owned(),
            display_name: definition.display_name,
            description: definition.description,
            phases: definition.phases,
            lua_source: definition.lua_source,
            input_schema: definition.input_schema,
            output_schema: definition.output_schema,
        });
    }

    let located = locate_run(config, target)?;
    let meta = journal::read_run_metadata(&located.run_dir).map_err(map_workflow_error)?;
    Ok(save_request_from_run_metadata(&meta, name_override))
}

fn save_request_from_run_metadata(
    meta: &WorkflowRunMetadata,
    name_override: Option<&str>,
) -> WorkflowSaveRequest {
    WorkflowSaveRequest {
        name: name_override.unwrap_or(meta.name.as_str()).to_owned(),
        display_name: meta.name.clone(),
        description: meta.description.clone(),
        phases: meta.phases.clone(),
        lua_source: meta.script.clone(),
        input_schema: None,
        // Runs pin script bytes but not definition schemas; use a minimal
        // object schema so the pair remains launchable and no-clobber-safe.
        output_schema: json!({ "type": "object" }),
    }
}

struct LocatedRun {
    run_id: String,
    run_dir: PathBuf,
    session_dir: PathBuf,
}

fn locate_run(config: &AppConfig, run_ref: &str) -> anyhow::Result<LocatedRun> {
    let as_path = Path::new(run_ref);
    if as_path.is_dir() && as_path.join("run.json").is_file() {
        let run_dir = as_path.to_path_buf();
        let meta = journal::read_run_metadata(&run_dir).map_err(map_workflow_error)?;
        let session_dir = run_dir
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .with_context(|| format!("run dir {} has no session parent", run_dir.display()))?;
        return Ok(LocatedRun {
            run_id: meta.run_id.as_str().to_owned(),
            run_dir,
            session_dir,
        });
    }

    if let Some(run_dir) = find_run_dir_anywhere(config, run_ref)? {
        let session_dir = run_dir
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .with_context(|| format!("run dir {} has no session parent", run_dir.display()))?;
        return Ok(LocatedRun {
            run_id: run_ref.to_owned(),
            run_dir,
            session_dir,
        });
    }

    bail!("workflow run not found: {run_ref}");
}

fn find_run_dir_anywhere(config: &AppConfig, run_id: &str) -> anyhow::Result<Option<PathBuf>> {
    let workspace_bucket = workspace_sessions_dir(config);
    if let Some(found) = scan_bucket_for_run(&workspace_bucket, run_id)? {
        return Ok(Some(found));
    }
    let sessions_root = neo_home()
        .map(|home| home.join("sessions"))
        .unwrap_or_else(|| config.sessions_dir.clone());
    if sessions_root.is_dir() {
        for entry in fs::read_dir(&sessions_root)
            .with_context(|| format!("failed to read {}", sessions_root.display()))?
            .flatten()
        {
            let path = entry.path();
            if path.is_dir()
                && let Some(found) = scan_bucket_for_run(&path, run_id)?
            {
                return Ok(Some(found));
            }
        }
    }
    Ok(None)
}

fn scan_bucket_for_run(bucket: &Path, run_id: &str) -> anyhow::Result<Option<PathBuf>> {
    if !bucket.is_dir() {
        return Ok(None);
    }
    for entry in fs::read_dir(bucket)
        .with_context(|| format!("failed to read session bucket {}", bucket.display()))?
        .flatten()
    {
        let session_dir = entry.path();
        if !session_dir.is_dir() {
            continue;
        }
        let candidate = session_dir.join("workflows").join(run_id);
        if candidate.join("run.json").is_file() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

async fn rehydrate_session_runs(config: &AppConfig, session_dir: &Path) -> anyhow::Result<()> {
    let _ = config
        .workflow_runtime
        .rehydrate(session_dir)
        .await
        .map_err(map_workflow_error)?;
    Ok(())
}

fn collect_retention_subjects(config: &AppConfig) -> anyhow::Result<Vec<RetentionSubject>> {
    let mut subjects = Vec::new();
    let now_ms = current_unix_ms();
    let mut referenced_parents = HashSet::new();
    let mut discovered: Vec<(WorkflowId, WorkflowState, u64, u64)> = Vec::new();

    let sessions_root = neo_home()
        .map(|home| home.join("sessions"))
        .unwrap_or_else(|| config.sessions_dir.clone());
    if !sessions_root.is_dir() {
        return Ok(subjects);
    }

    for bucket in fs::read_dir(&sessions_root)
        .with_context(|| format!("failed to read {}", sessions_root.display()))?
        .flatten()
    {
        let bucket_path = bucket.path();
        if !bucket_path.is_dir() {
            continue;
        }
        for session in fs::read_dir(&bucket_path)
            .with_context(|| format!("failed to read {}", bucket_path.display()))?
            .flatten()
        {
            let session_dir = session.path();
            let workflows_dir = session_dir.join("workflows");
            if !workflows_dir.is_dir() {
                continue;
            }
            for run_entry in fs::read_dir(&workflows_dir)
                .with_context(|| format!("failed to read {}", workflows_dir.display()))?
                .flatten()
            {
                let run_dir = run_entry.path();
                if !run_dir.is_dir() || !run_dir.join("run.json").is_file() {
                    continue;
                }
                let meta = match journal::read_run_metadata(&run_dir) {
                    Ok(meta) => meta,
                    Err(_) => continue,
                };
                if let Some(parent) = &meta.parent_run_id {
                    referenced_parents.insert(parent.as_str().to_owned());
                }
                let state = infer_run_state(&run_dir, &meta.run_id);
                let bytes = dir_byte_size(&run_dir).unwrap_or(0);
                let age_ms = file_age_ms(&run_dir, now_ms).unwrap_or(0);
                discovered.push((meta.run_id, state, bytes, age_ms));
            }
        }
    }

    for (run_id, state, bytes, age_ms) in discovered {
        let referenced = referenced_parents.contains(run_id.as_str());
        subjects.push(RetentionSubject {
            run_id,
            state,
            bytes,
            age_ms,
            referenced,
            pinned: false,
        });
    }
    Ok(subjects)
}

fn infer_run_state(run_dir: &Path, run_id: &WorkflowId) -> WorkflowState {
    let journal_path = run_dir.join("journal.jsonl");
    if let Ok(envelopes) = collect_journal_v2(&journal_path, Some(run_id)) {
        for envelope in envelopes.iter().rev() {
            if let JournalPayload::StateChanged { new, .. } = &envelope.payload {
                return *new;
            }
        }
    }
    WorkflowState::Completed
}

fn dir_byte_size(path: &Path) -> anyhow::Result<u64> {
    let mut total = 0_u64;
    if path.is_file() {
        return Ok(fs::metadata(path)?.len());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        if child.is_dir() {
            total = total.saturating_add(dir_byte_size(&child)?);
        } else {
            total = total.saturating_add(fs::metadata(&child)?.len());
        }
    }
    Ok(total)
}

fn file_age_ms(path: &Path, now_ms: u64) -> anyhow::Result<u64> {
    let modified = fs::metadata(path)?.modified()?;
    let modified_ms = modified
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    Ok(now_ms.saturating_sub(modified_ms))
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn parse_duration_ms(raw: &str) -> anyhow::Result<u64> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("empty duration");
    }
    let (number, unit) = split_trailing_unit(raw)?;
    let amount: u64 = number
        .parse()
        .with_context(|| format!("invalid duration number in `{raw}`"))?;
    let ms = match unit {
        "ms" => amount,
        "s" | "sec" | "secs" | "second" | "seconds" => amount.saturating_mul(1_000),
        "m" | "min" | "mins" | "minute" | "minutes" => amount.saturating_mul(60_000),
        "h" | "hr" | "hrs" | "hour" | "hours" => amount.saturating_mul(3_600_000),
        "d" | "day" | "days" => amount.saturating_mul(86_400_000),
        "w" | "week" | "weeks" => amount.saturating_mul(604_800_000),
        other => bail!("unsupported duration unit `{other}` in `{raw}`"),
    };
    Ok(ms)
}

fn parse_byte_size(raw: &str) -> anyhow::Result<u64> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("empty byte size");
    }
    let (number, unit) = split_trailing_unit(raw)?;
    let amount: u64 = number
        .parse()
        .with_context(|| format!("invalid byte size number in `{raw}`"))?;
    let bytes = match unit {
        "" | "b" | "B" => amount,
        "k" | "K" | "kb" | "KB" => amount.saturating_mul(1_000),
        "ki" | "Ki" | "kib" | "KiB" => amount.saturating_mul(1_024),
        "m" | "M" | "mb" | "MB" => amount.saturating_mul(1_000_000),
        "mi" | "Mi" | "mib" | "MiB" => amount.saturating_mul(1_024 * 1_024),
        "g" | "G" | "gb" | "GB" => amount.saturating_mul(1_000_000_000),
        "gi" | "Gi" | "gib" | "GiB" => amount.saturating_mul(1_024 * 1_024 * 1_024),
        other => bail!("unsupported byte unit `{other}` in `{raw}`"),
    };
    Ok(bytes)
}

fn split_trailing_unit(raw: &str) -> anyhow::Result<(&str, &str)> {
    let split_at = raw
        .char_indices()
        .find(|(_, ch)| ch.is_ascii_alphabetic())
        .map(|(idx, _)| idx)
        .unwrap_or(raw.len());
    let (number, unit) = raw.split_at(split_at);
    if number.is_empty() {
        bail!("missing number in `{raw}`");
    }
    Ok((number, unit))
}
