//! Queue management — live steering input, follow-up queues, and queue draining.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::workflow::{WorkflowId, WorkflowState};
use crate::{AgentEvent, AgentMessage, QueueKind};

use super::config::{AgentConfig, QueueMode};
use super::events::EventEmitter;

/// Live input pushed into a running turn by the controller.
///
/// `SteerNow` injects at the next step boundary (tool-call end / thinking end)
/// as a steering context message, without interrupting the current step.
/// `FollowUp` is appended to the follow-up queue and starts a fresh turn after
/// the current workflow drains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveTurnInput {
    /// Inject as a steering message at the next natural break point.
    SteerNow(AgentMessage),
    /// Queue as a follow-up turn after the current turn completes (FIFO).
    FollowUp(AgentMessage),
    /// Remove the oldest queued follow-up after the UI pulled it back into the
    /// composer for editing.
    DequeueFollowUpForEdit,
    /// Reclassify the oldest queued follow-up as steering input.
    PromoteFollowUpToSteer,
}

const WORKFLOW_NOTIFICATION_PREFIX: &str = "workflow_notification:";

/// A durable workflow result or host-exit recovery notice waiting for the next
/// user-triggered model turn. This queue is deliberately separate from
/// steering/follow-up input: notifications never start a turn themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowNotification {
    pub id: String,
    pub run_id: WorkflowId,
    pub state: WorkflowState,
    pub reason: String,
    pub session_dir: PathBuf,
}

impl WorkflowNotification {
    #[must_use]
    pub fn new(
        session_dir: impl AsRef<Path>,
        run_id: WorkflowId,
        state: WorkflowState,
        reason: impl Into<String>,
    ) -> Self {
        let reason = reason.into();
        let state_name = state_label(state);
        let digest = Sha256::digest(reason.as_bytes());
        let id = format!(
            "{WORKFLOW_NOTIFICATION_PREFIX}{}:{state_name}:{digest:x}",
            run_id.0
        );
        Self {
            id,
            run_id,
            state,
            reason,
            session_dir: session_dir.as_ref().to_path_buf(),
        }
    }

    #[must_use]
    pub fn reminder_message(&self) -> AgentMessage {
        AgentMessage::system_reminder_with_origin(
            format!(
                "Workflow {} is {} ({}). Call TaskOutput with task_id `{}` to inspect its current status and result; this reminder does not contain the workflow result.",
                self.run_id,
                state_label(self.state),
                self.reason,
                self.run_id
            ),
            self.id.clone(),
        )
    }

    #[must_use]
    pub fn projection_id(message: &AgentMessage) -> Option<&str> {
        let AgentMessage::User {
            origin: crate::MessageOrigin::Injection { variant },
            ..
        } = message
        else {
            return None;
        };
        variant
            .strip_prefix(WORKFLOW_NOTIFICATION_PREFIX)
            .map(|_| variant.as_ref())
    }

    #[must_use]
    pub fn is_projected_in(&self, messages: &[AgentMessage]) -> bool {
        messages
            .iter()
            .any(|message| Self::projection_id(message) == Some(self.id.as_str()))
    }
}

#[derive(Debug, Default)]
struct WorkflowNotificationState {
    queued: VecDeque<WorkflowNotification>,
    projected: HashSet<String>,
}

/// Shared typed workflow notification delivery state.
#[derive(Debug, Clone, Default)]
pub struct WorkflowNotificationQueue {
    inner: Arc<Mutex<WorkflowNotificationState>>,
}

impl WorkflowNotificationQueue {
    fn lock(&self) -> std::sync::MutexGuard<'_, WorkflowNotificationState> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Queue a notice after its workflow journal transition is durable.
    #[must_use]
    pub fn enqueue(&self, notification: WorkflowNotification) -> bool {
        let mut state = self.lock();
        if state.projected.contains(&notification.id)
            || state.queued.iter().any(|item| item.id == notification.id)
        {
            return false;
        }
        state.queued.push_back(notification);
        true
    }

    /// Return pending notices for one origin session without consuming them.
    #[must_use]
    pub fn pending_for_session(&self, session_dir: &Path) -> Vec<WorkflowNotification> {
        self.lock()
            .queued
            .iter()
            .filter(|notification| notification.session_dir == session_dir)
            .cloned()
            .collect()
    }

    /// Mark a reminder durable in Session JSONL and remove it from delivery.
    #[must_use]
    pub fn mark_projected(&self, id: &str) -> bool {
        let mut state = self.lock();
        let queued = state
            .queued
            .iter()
            .any(|notification| notification.id == id);
        state.queued.retain(|notification| notification.id != id);
        state.projected.insert(id.to_owned()) || queued
    }

    /// Restore projection identities from a replayed session.
    pub fn restore_projected<I>(&self, ids: I)
    where
        I: IntoIterator<Item = String>,
    {
        let mut state = self.lock();
        for id in ids {
            state.projected.insert(id.clone());
            state.queued.retain(|notification| notification.id != id);
        }
    }
}

pub(super) fn append_pending_workflow_notifications(
    config: &AgentConfig,
    emitter: &mut EventEmitter,
) {
    let Some(session_dir) = config.session_directory.as_deref() else {
        return;
    };
    for notification in config
        .workflow_runtime
        .notification_queue()
        .pending_for_session(session_dir)
    {
        if !notification.is_projected_in(emitter.context.messages()) {
            emitter.emit(AgentEvent::MessageAppended {
                message: notification.reminder_message(),
            });
        }
    }
}

fn state_label(state: WorkflowState) -> &'static str {
    match state {
        WorkflowState::Running => "running",
        WorkflowState::Paused => "paused",
        WorkflowState::Completed => "completed",
        WorkflowState::Failed => "failed",
        WorkflowState::Cancelled => "cancelled",
        WorkflowState::ResourceLimited => "resource_limited",
    }
}

/// Shared handle used to push live input into a running turn.
///
/// Created by the controller before a turn starts, threaded into the
/// [`AgentRuntime`], and drained at each step boundary by `run_agent_turn`.
/// Both the controller and the runtime share the same cell.
#[derive(Debug, Clone, Default)]
pub struct SteerInputHandle {
    inner: Arc<Mutex<VecDeque<ActiveTurnInput>>>,
}

impl SteerInputHandle {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a live input onto the queue. Called by the controller.
    pub fn push(&self, input: ActiveTurnInput) {
        if let Ok(mut queue) = self.inner.lock() {
            queue.push_back(input);
        }
    }

    /// Drain all pending live inputs. Called by the runtime at step boundaries.
    fn drain(&self) -> Vec<ActiveTurnInput> {
        self.inner
            .lock()
            .map(|mut queue| queue.drain(..).collect())
            .unwrap_or_default()
    }

    /// Number of pending live inputs (for UI status).
    #[must_use]
    pub fn pending(&self) -> usize {
        self.inner
            .lock()
            .map(|queue| queue.len())
            .unwrap_or_default()
    }
}

pub(super) fn drain_next_pending_queue(
    config: &AgentConfig,
    emitter: &mut EventEmitter,
) -> (Vec<AgentMessage>, Option<QueueKind>) {
    let steering = drain_steering_queue(config, emitter);
    if steering.is_empty() {
        let follow_up = drain_follow_up_queue(config, emitter);
        let kind = (!follow_up.is_empty()).then_some(QueueKind::FollowUp);
        (follow_up, kind)
    } else {
        (steering, Some(QueueKind::Steering))
    }
}

pub(super) fn drain_steering_queue(
    config: &AgentConfig,
    emitter: &mut EventEmitter,
) -> Vec<AgentMessage> {
    let messages = take_messages(&emitter.context.steering_queue, config.steering_queue_mode);
    emit_queue_drained(emitter, QueueKind::Steering, messages.len());
    messages
}

fn drain_follow_up_queue(config: &AgentConfig, emitter: &mut EventEmitter) -> Vec<AgentMessage> {
    let messages = take_messages(
        &emitter.context.follow_up_queue,
        config.follow_up_queue_mode,
    );
    emit_queue_drained(emitter, QueueKind::FollowUp, messages.len());
    messages
}

/// Drain live input pushed by the controller into the running turn and route
/// each item into the matching context queue via a persisted queue event.
///
/// `SteerNow` feeds the steering queue (injected at the next model call);
/// `FollowUp` feeds the follow-up queue (starts a fresh turn after the current
/// workflow drains). Both emit their queue events so the TUI and JSONL replay
/// stay in sync — this is the only production emitter of `SteeringQueued` and
/// `FollowUpQueued`.
pub(super) fn drain_live_steer_input(handle: &SteerInputHandle, emitter: &mut EventEmitter) {
    for input in handle.drain() {
        match input {
            ActiveTurnInput::SteerNow(message) => {
                emitter.emit(AgentEvent::SteeringQueued { message });
            }
            ActiveTurnInput::FollowUp(message) => {
                emitter.emit(AgentEvent::FollowUpQueued { message });
            }
            ActiveTurnInput::DequeueFollowUpForEdit => {
                if emitter.context.follow_up_queue.is_empty() {
                    continue;
                }
                emitter.emit(AgentEvent::QueueDrained {
                    kind: QueueKind::FollowUp,
                    count: 1,
                });
            }
            ActiveTurnInput::PromoteFollowUpToSteer => {
                let Some(message) = emitter.context.follow_up_queue.first().cloned() else {
                    continue;
                };
                emitter.emit(AgentEvent::QueueDrained {
                    kind: QueueKind::FollowUp,
                    count: 1,
                });
                emitter.emit(AgentEvent::SteeringQueued { message });
            }
        }
    }
}

fn emit_queue_drained(emitter: &mut EventEmitter, kind: QueueKind, count: usize) {
    if count > 0 {
        emitter.emit(AgentEvent::QueueDrained { kind, count });
    }
}

fn take_messages(queue: &[AgentMessage], mode: QueueMode) -> Vec<AgentMessage> {
    let count = match mode {
        QueueMode::All => queue.len(),
        QueueMode::OneAtATime => usize::from(!queue.is_empty()),
    };
    queue.iter().take(count).cloned().collect()
}
