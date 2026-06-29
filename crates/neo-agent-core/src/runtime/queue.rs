//! Queue management — live steering input, follow-up queues, and queue draining.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

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
    /// Reclassify the oldest queued follow-up as steering input.
    PromoteFollowUpToSteer,
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
) -> Vec<AgentMessage> {
    let steering = drain_steering_queue(config, emitter);
    if steering.is_empty() {
        drain_follow_up_queue(config, emitter)
    } else {
        steering
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
