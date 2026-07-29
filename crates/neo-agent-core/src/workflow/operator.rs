//! Workflow Operator projections built from the canonical workflow journal.

use std::collections::{BTreeMap, HashMap};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::WorkflowError;
use super::child_projection::{WorkflowChildRow, WorkflowChildState, for_each_journal_envelope};
use super::journal::{JournalPayload, WorkflowChildKey};
use super::state::{
    WorkflowId, WorkflowOutcomeStatus, WorkflowRunMetadata, WorkflowSnapshot, WorkflowState,
};
use super::user_input::PendingUserInput;

/// Key that identifies a workflow step for paging.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct WorkflowStepKey {
    pub phase_id: Option<String>,
    pub phase_marker_sequence: u64,
}

/// Paged child rows for one step.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowChildPage {
    pub items: Vec<super::child_projection::WorkflowChildRow>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub query_hash: String,
}

/// Operator query request from the TUI.
#[derive(Debug, Clone)]
pub struct WorkflowOperatorRequest {
    pub step: Option<WorkflowStepKey>,
    pub cursor: Option<String>,
    pub limit: usize,
}

/// Immutable operator snapshot consumed by the TUI.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowOperatorSnapshot {
    pub task_id: String,
    pub run_id: super::state::WorkflowId,
    pub display_name: String,
    pub purpose: String,
    pub state: super::state::WorkflowState,
    pub elapsed_ms: u64,
    pub updated_at_ms: u64,
    pub current_step_key: Option<WorkflowStepKey>,
    pub child_counts: ChildCounts,
    pub steps: Vec<WorkflowStepRow>,
    pub pending_user: Option<PendingUserRequest>,
    pub final_summary: Option<String>,
    pub failure_reason: Option<String>,
    pub generated_files: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChildCounts {
    pub done: u64,
    pub working: u64,
    pub queued: u64,
    pub failed: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowStepRow {
    pub key: WorkflowStepKey,
    pub title: String,
    pub order: u64,
    pub state: StepRowState,
    pub done_count: u64,
    pub working_count: u64,
    pub queued_count: u64,
    pub failed_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepRowState {
    Pending,
    Active,
    Completed,
    Failed,
    Paused,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingUserRequest {
    pub request_id: String,
    pub prompt: String,
    pub answer_schema: Option<serde_json::Value>,
    pub default: Option<serde_json::Value>,
    pub title: Option<String>,
    pub answer_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChildCursor {
    query_hash: String,
    after_seq: u64,
}

#[derive(Debug, Clone)]
struct ChildState {
    step: WorkflowStepKey,
    state: WorkflowChildState,
}

#[derive(Debug, Default)]
struct StepCounts {
    done: u64,
    working: u64,
    queued: u64,
    failed: u64,
}

type StepScan = (
    Vec<WorkflowStepRow>,
    BTreeMap<WorkflowStepKey, StepCounts>,
    Option<WorkflowStepKey>,
);

impl StepCounts {
    fn add(&mut self, state: WorkflowChildState) {
        match state {
            WorkflowChildState::Completed => self.done = self.done.saturating_add(1),
            WorkflowChildState::Queued => self.queued = self.queued.saturating_add(1),
            WorkflowChildState::Running | WorkflowChildState::Recovering => {
                self.working = self.working.saturating_add(1);
            }
            WorkflowChildState::Failed
            | WorkflowChildState::Cancelled
            | WorkflowChildState::Interrupted => self.failed = self.failed.saturating_add(1),
        }
    }

    fn remove(&mut self, state: WorkflowChildState) {
        match state {
            WorkflowChildState::Completed => self.done = self.done.saturating_sub(1),
            WorkflowChildState::Queued => self.queued = self.queued.saturating_sub(1),
            WorkflowChildState::Running | WorkflowChildState::Recovering => {
                self.working = self.working.saturating_sub(1);
            }
            WorkflowChildState::Failed
            | WorkflowChildState::Cancelled
            | WorkflowChildState::Interrupted => self.failed = self.failed.saturating_sub(1),
        }
    }
}

#[derive(Debug, Clone)]
struct StepDefinition {
    key: WorkflowStepKey,
    title: String,
    order: u64,
}

pub(crate) fn project_snapshot(
    task_id: &str,
    snapshot: &WorkflowSnapshot,
    metadata: &WorkflowRunMetadata,
    pending_user: Option<PendingUserInput>,
    journal_path: &std::path::Path,
    journal_record_bytes: u64,
    journal_total_bytes: u64,
) -> Result<WorkflowOperatorSnapshot, WorkflowError> {
    let (steps, counts, current_step_key) = scan_steps(
        snapshot,
        metadata,
        journal_path,
        journal_record_bytes,
        journal_total_bytes,
    )?;
    let child_counts = counts
        .values()
        .fold(ChildCounts::default(), |mut total, count| {
            total.done = total.done.saturating_add(count.done);
            total.working = total.working.saturating_add(count.working);
            total.queued = total.queued.saturating_add(count.queued);
            total.failed = total.failed.saturating_add(count.failed);
            total
        });
    let terminal_at_ms = snapshot
        .state
        .is_terminal()
        .then_some(snapshot.updated_at_ms)
        .flatten();
    let elapsed_ms = elapsed_ms(snapshot.started_at_ms, terminal_at_ms);
    Ok(WorkflowOperatorSnapshot {
        task_id: task_id.to_owned(),
        run_id: snapshot.id.clone(),
        display_name: snapshot.display_name.clone(),
        purpose: snapshot.purpose.clone(),
        state: snapshot.state,
        elapsed_ms,
        updated_at_ms: snapshot.updated_at_ms.unwrap_or_default(),
        current_step_key,
        child_counts,
        steps,
        pending_user: pending_user.map(pending_user_request),
        final_summary: snapshot.latest_report_summary.clone(),
        failure_reason: snapshot.terminal_reason.clone(),
        generated_files: Vec::new(),
    })
}

pub(crate) fn project_child_page(
    run_id: &WorkflowId,
    metadata: &WorkflowRunMetadata,
    request: &WorkflowOperatorRequest,
    journal_path: &std::path::Path,
    journal_record_bytes: u64,
    journal_total_bytes: u64,
) -> Result<WorkflowChildPage, WorkflowError> {
    if request.limit == 0 {
        return Err(WorkflowError::InvalidInput(
            "workflow child page limit must be greater than zero".to_owned(),
        ));
    }
    let query_hash = child_query_hash(request.step.as_ref());
    let after_seq = decode_cursor(request.cursor.as_deref(), &query_hash)?;
    let definitions = step_definitions(metadata);
    let mut dynamic_steps = BTreeMap::new();
    let mut items = Vec::with_capacity(request.limit);
    let mut page_rows = HashMap::<WorkflowChildKey, usize>::new();
    let mut has_more = false;
    let mut last_selected_seq = after_seq;

    for_each_journal_envelope(
        journal_path,
        Some(run_id),
        journal_record_bytes,
        journal_total_bytes,
        |envelope| {
            match envelope.payload {
                JournalPayload::ChildQueued {
                    child_key,
                    child_kind,
                    phase_id,
                    title,
                    role,
                    ..
                } => {
                    let step =
                        step_for_child(phase_id, envelope.seq, &definitions, &mut dynamic_steps);
                    if request
                        .step
                        .as_ref()
                        .is_none_or(|selected| selected == &step)
                        && envelope.seq > after_seq
                    {
                        if items.len() < request.limit {
                            last_selected_seq = envelope.seq;
                            page_rows.insert(child_key.clone(), items.len());
                            items.push(WorkflowChildRow {
                                key: child_key,
                                child_kind,
                                phase_id: step.phase_id.clone(),
                                agent_id: None,
                                state: WorkflowChildState::Queued,
                                title,
                                role,
                                queued_at_ms: Some(envelope.timestamp_ms),
                                started_at_ms: None,
                                updated_at_ms: envelope.timestamp_ms,
                                terminal_at_ms: None,
                                terminal_summary: None,
                                error_summary: None,
                                actual_usage: None,
                                latest_activity: None,
                                generated_files: Vec::new(),
                            });
                        } else {
                            has_more = true;
                        }
                    }
                }
                JournalPayload::ChildStarted {
                    child_key,
                    agent_id,
                } => {
                    if let Some(index) = page_rows.get(&child_key) {
                        let row = &mut items[*index];
                        row.state = WorkflowChildState::Recovering;
                        row.agent_id = agent_id.or_else(|| row.agent_id.clone());
                        row.started_at_ms = Some(envelope.timestamp_ms);
                        row.updated_at_ms = envelope.timestamp_ms;
                    }
                }
                JournalPayload::ChildFinished {
                    child_key,
                    agent_id,
                    status,
                    summary,
                    actual_usage,
                    error,
                } => {
                    if let Some(index) = page_rows.get(&child_key) {
                        let row = &mut items[*index];
                        if row.started_at_ms.is_none() && agent_id.is_some() {
                            row.agent_id = agent_id;
                        }
                        row.state = child_state(status);
                        row.terminal_at_ms = Some(envelope.timestamp_ms);
                        row.updated_at_ms = envelope.timestamp_ms;
                        row.terminal_summary = Some(summary);
                        row.error_summary = error;
                        row.actual_usage =
                            actual_usage.and_then(|usage| serde_json::to_value(usage).ok());
                    }
                }
                _ => {}
            }
            Ok(())
        },
    )?;
    Ok(WorkflowChildPage {
        next_cursor: has_more
            .then(|| encode_cursor(&query_hash, last_selected_seq).expect("cursor serialization")),
        has_more,
        items,
        query_hash,
    })
}

fn scan_steps(
    snapshot: &WorkflowSnapshot,
    metadata: &WorkflowRunMetadata,
    journal_path: &std::path::Path,
    journal_record_bytes: u64,
    journal_total_bytes: u64,
) -> Result<StepScan, WorkflowError> {
    let definitions = step_definitions(metadata);
    let mut dynamic_steps = BTreeMap::new();
    let mut counts = BTreeMap::<WorkflowStepKey, StepCounts>::new();
    let mut active = HashMap::<WorkflowChildKey, ChildState>::new();
    for definition in definitions.values() {
        counts.entry(definition.key.clone()).or_default();
    }
    for_each_journal_envelope(
        journal_path,
        Some(&snapshot.id),
        journal_record_bytes,
        journal_total_bytes,
        |envelope| {
            match envelope.payload {
                JournalPayload::ChildQueued {
                    child_key,
                    phase_id,
                    ..
                } => {
                    let step =
                        step_for_child(phase_id, envelope.seq, &definitions, &mut dynamic_steps);
                    counts
                        .entry(step.clone())
                        .or_default()
                        .add(WorkflowChildState::Queued);
                    active.insert(
                        child_key,
                        ChildState {
                            step,
                            state: WorkflowChildState::Queued,
                        },
                    );
                }
                JournalPayload::ChildStarted { child_key, .. } => {
                    if let Some(child) = active.get_mut(&child_key) {
                        let count = counts.entry(child.step.clone()).or_default();
                        count.remove(child.state);
                        child.state = WorkflowChildState::Recovering;
                        count.add(child.state);
                    }
                }
                JournalPayload::ChildFinished {
                    child_key, status, ..
                } => {
                    if let Some(child) = active.remove(&child_key) {
                        let count = counts.entry(child.step).or_default();
                        count.remove(child.state);
                        count.add(child_state(status));
                    }
                }
                _ => {}
            }
            Ok(())
        },
    )?;

    let mut definitions: Vec<_> = definitions.into_values().collect();
    definitions.extend(dynamic_steps.into_values());
    definitions.sort_by_key(|definition| definition.order);
    if definitions.is_empty() {
        definitions.push(StepDefinition {
            key: WorkflowStepKey {
                phase_id: None,
                phase_marker_sequence: 0,
            },
            title: "Execution".to_owned(),
            order: 0,
        });
    }
    let current_step_key = definitions
        .iter()
        .find(|definition| definition.key.phase_id.as_deref() == snapshot.current_phase.as_deref())
        .map(|definition| definition.key.clone())
        .or_else(|| definitions.first().map(|definition| definition.key.clone()));
    let steps = definitions
        .into_iter()
        .map(|definition| {
            let count = counts.entry(definition.key.clone()).or_default();
            let state = step_state(
                snapshot.state,
                current_step_key.as_ref(),
                &definition.key,
                count,
            );
            WorkflowStepRow {
                key: definition.key,
                title: definition.title,
                order: definition.order,
                state,
                done_count: count.done,
                working_count: count.working,
                queued_count: count.queued,
                failed_count: count.failed,
            }
        })
        .collect();
    Ok((steps, counts, current_step_key))
}

fn step_definitions(metadata: &WorkflowRunMetadata) -> BTreeMap<Option<String>, StepDefinition> {
    metadata
        .phases
        .iter()
        .enumerate()
        .map(|(index, phase)| {
            let order = u64::try_from(index).unwrap_or(u64::MAX);
            let phase_id = Some(phase.id.clone());
            (
                phase_id.clone(),
                StepDefinition {
                    key: WorkflowStepKey {
                        phase_id,
                        phase_marker_sequence: order,
                    },
                    title: if phase.description.trim().is_empty() {
                        phase.id.clone()
                    } else {
                        phase.description.clone()
                    },
                    order,
                },
            )
        })
        .collect()
}

fn step_for_child(
    phase_id: Option<String>,
    sequence: u64,
    definitions: &BTreeMap<Option<String>, StepDefinition>,
    dynamic_steps: &mut BTreeMap<Option<String>, StepDefinition>,
) -> WorkflowStepKey {
    if let Some(definition) = definitions
        .get(&phase_id)
        .or_else(|| dynamic_steps.get(&phase_id))
    {
        return definition.key.clone();
    }
    let definition = StepDefinition {
        key: WorkflowStepKey {
            phase_id: phase_id.clone(),
            phase_marker_sequence: sequence,
        },
        title: phase_id.clone().unwrap_or_else(|| "Execution".to_owned()),
        order: sequence,
    };
    let key = definition.key.clone();
    dynamic_steps.insert(phase_id, definition);
    key
}

fn step_state(
    workflow_state: WorkflowState,
    current: Option<&WorkflowStepKey>,
    key: &WorkflowStepKey,
    count: &StepCounts,
) -> StepRowState {
    if count.failed > 0 {
        StepRowState::Failed
    } else if workflow_state == WorkflowState::Paused && current == Some(key) {
        StepRowState::Paused
    } else if workflow_state == WorkflowState::Completed
        || count.done > 0 && count.working == 0 && count.queued == 0
    {
        StepRowState::Completed
    } else if current == Some(key) {
        StepRowState::Active
    } else {
        StepRowState::Pending
    }
}

fn child_state(status: WorkflowOutcomeStatus) -> WorkflowChildState {
    match status {
        WorkflowOutcomeStatus::Completed => WorkflowChildState::Completed,
        WorkflowOutcomeStatus::Cancelled => WorkflowChildState::Cancelled,
        WorkflowOutcomeStatus::Interrupted => WorkflowChildState::Interrupted,
        WorkflowOutcomeStatus::Failed
        | WorkflowOutcomeStatus::Denied
        | WorkflowOutcomeStatus::ResourceLimited => WorkflowChildState::Failed,
    }
}

fn elapsed_ms(started_at_ms: Option<u64>, terminal_at_ms: Option<u64>) -> u64 {
    let Some(started_at_ms) = started_at_ms else {
        return 0;
    };
    let ended_at_ms = terminal_at_ms.unwrap_or_else(current_epoch_ms);
    ended_at_ms.saturating_sub(started_at_ms)
}

fn current_epoch_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

fn pending_user_request(pending: PendingUserInput) -> PendingUserRequest {
    PendingUserRequest {
        request_id: pending.request_id,
        prompt: pending.prompt,
        answer_schema: Some(pending.answer_schema),
        default: pending.default,
        title: pending.title,
        answer_policy: pending.answer_policy.as_str().to_owned(),
    }
}

fn child_query_hash(step: Option<&WorkflowStepKey>) -> String {
    let bytes = serde_json::to_vec(&step).expect("workflow step key serializes");
    format!("{:x}", Sha256::digest(bytes))
}

fn encode_cursor(query_hash: &str, after_seq: u64) -> Result<String, WorkflowError> {
    let bytes = serde_json::to_vec(&ChildCursor {
        query_hash: query_hash.to_owned(),
        after_seq,
    })
    .map_err(|error| {
        WorkflowError::InvalidInput(format!("workflow child cursor encode failed: {error}"))
    })?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn decode_cursor(raw: Option<&str>, query_hash: &str) -> Result<u64, WorkflowError> {
    let Some(raw) = raw else {
        return Ok(0);
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw.trim().as_bytes())
        .map_err(|error| {
            WorkflowError::InvalidInput(format!("invalid workflow child cursor: {error}"))
        })?;
    let cursor: ChildCursor = serde_json::from_slice(&bytes).map_err(|error| {
        WorkflowError::InvalidInput(format!("invalid workflow child cursor: {error}"))
    })?;
    if cursor.query_hash != query_hash {
        return Err(WorkflowError::InvalidInput(
            "workflow child cursor does not match the selected step".to_owned(),
        ));
    }
    Ok(cursor.after_seq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::{JournalEnvelope, JournalWriter, WorkflowLimits, WorkflowPhase};

    fn metadata(run_id: WorkflowId) -> WorkflowRunMetadata {
        WorkflowRunMetadata {
            run_id,
            name: "operator".to_owned(),
            description: "projection".to_owned(),
            phases: vec![WorkflowPhase {
                id: "review".to_owned(),
                description: "Review".to_owned(),
            }],
            script: String::new(),
            script_sha256: String::new(),
            args: serde_json::json!({}),
            launch_source: "test".to_owned(),
            output_schema: None,
            display_name: None,
            input_schema: None,
            definition_origin: None,
            inline_unsaved: false,
        }
    }

    fn journal_with_children(count: usize) -> (tempfile::TempDir, WorkflowId, WorkflowRunMetadata) {
        let dir = tempfile::tempdir().expect("temporary journal directory");
        let run_id = WorkflowId(format!("wf_operator_{count}"));
        let limits = WorkflowLimits::default();
        let mut writer =
            JournalWriter::open(&dir.path().join("journal.jsonl"), run_id.clone(), &limits)
                .expect("open journal");
        let created = JournalEnvelope::new(
            writer.next_seq(),
            1,
            run_id.clone(),
            JournalPayload::RunCreated {
                name: "operator".to_owned(),
                description: None,
                launch_source: Some("test".to_owned()),
            },
        );
        writer.append(&created, &limits).expect("append run");
        for index in 0..count {
            let queued = JournalEnvelope::new(
                writer.next_seq(),
                u64::try_from(index).unwrap_or(u64::MAX).saturating_add(2),
                run_id.clone(),
                JournalPayload::ChildQueued {
                    child_key: WorkflowChildKey::DirectDelegate {
                        invocation_id: format!("child-{index}"),
                    },
                    child_kind: crate::workflow::WorkflowChildKind::Delegate,
                    invocation_id: format!("inv-{index}"),
                    phase_id: Some("review".to_owned()),
                    title: None,
                    role: None,
                },
            );
            writer.append(&queued, &limits).expect("append child");
        }
        (dir, run_id.clone(), metadata(run_id))
    }

    #[test]
    fn child_pages_cover_thousand_and_ten_thousand_rows_with_stable_cursor() {
        for count in [1_000, 10_000] {
            let (dir, run_id, metadata) = journal_with_children(count);
            let path = dir.path().join("journal.jsonl");
            let step = WorkflowStepKey {
                phase_id: Some("review".to_owned()),
                phase_marker_sequence: 0,
            };
            let mut request = WorkflowOperatorRequest {
                step: Some(step),
                cursor: None,
                limit: 2_048,
            };
            let first = project_child_page(
                &run_id,
                &metadata,
                &request,
                &path,
                WorkflowLimits::default().journal_record_bytes,
                WorkflowLimits::default().journal_total_bytes,
            )
            .expect("first page");
            let stable = project_child_page(
                &run_id,
                &metadata,
                &request,
                &path,
                WorkflowLimits::default().journal_record_bytes,
                WorkflowLimits::default().journal_total_bytes,
            )
            .expect("repeat first page");
            assert_eq!(first.items, stable.items);
            assert_eq!(first.next_cursor, stable.next_cursor);

            let mut seen = 0usize;
            let mut page = first;
            loop {
                seen = seen.saturating_add(page.items.len());
                let Some(cursor) = page.next_cursor else {
                    break;
                };
                request.cursor = Some(cursor);
                page = project_child_page(
                    &run_id,
                    &metadata,
                    &request,
                    &path,
                    WorkflowLimits::default().journal_record_bytes,
                    WorkflowLimits::default().journal_total_bytes,
                )
                .expect("next page");
            }
            assert_eq!(seen, count);
        }
    }

    #[test]
    fn snapshot_keeps_typed_pending_request_and_terminal_elapsed() {
        let (dir, run_id, metadata) = journal_with_children(1);
        let snapshot = WorkflowSnapshot {
            id: run_id,
            title: "operator".to_owned(),
            state: WorkflowState::Completed,
            current_phase: Some("review".to_owned()),
            projection_sequence: None,
            recovery_failure: false,
            started_at_ms: Some(100),
            updated_at_ms: Some(650),
            invocation_count: 0,
            failure_count: 0,
            actual_usage: None,
            latest_log_summary: None,
            latest_report_summary: Some("finished".to_owned()),
            terminal_reason: None,
            display_name: "Operator".to_owned(),
            purpose: "projection".to_owned(),
        };
        let pending = PendingUserInput {
            request_id: "request-1".to_owned(),
            prompt: "Choose".to_owned(),
            answer_schema: serde_json::json!({"type":"string"}),
            default: Some(serde_json::json!("default")),
            title: Some("Choice".to_owned()),
            answer_policy: super::super::user_input::UserAnswerPolicy::Human,
            answer: None,
        };
        let first = project_snapshot(
            "task-1",
            &snapshot,
            &metadata,
            Some(pending.clone()),
            &dir.path().join("journal.jsonl"),
            WorkflowLimits::default().journal_record_bytes,
            WorkflowLimits::default().journal_total_bytes,
        )
        .expect("snapshot");
        let reloaded = project_snapshot(
            "task-1",
            &snapshot,
            &metadata,
            Some(pending),
            &dir.path().join("journal.jsonl"),
            WorkflowLimits::default().journal_record_bytes,
            WorkflowLimits::default().journal_total_bytes,
        )
        .expect("reloaded snapshot");
        assert_eq!(first.elapsed_ms, 550);
        assert_eq!(reloaded.elapsed_ms, 550);
        assert_eq!(
            first
                .pending_user
                .as_ref()
                .map(|value| value.request_id.as_str()),
            Some("request-1")
        );
        assert_eq!(
            first
                .pending_user
                .as_ref()
                .and_then(|value| value.answer_schema.as_ref()),
            Some(&serde_json::json!({"type":"string"}))
        );
        assert_eq!(
            first
                .pending_user
                .as_ref()
                .and_then(|value| value.default.as_ref()),
            Some(&serde_json::json!("default"))
        );
        assert_eq!(
            first
                .pending_user
                .as_ref()
                .and_then(|value| value.title.as_deref()),
            Some("Choice")
        );
        assert_eq!(
            first
                .pending_user
                .as_ref()
                .map(|value| value.answer_policy.as_str()),
            Some("human")
        );
    }
}
