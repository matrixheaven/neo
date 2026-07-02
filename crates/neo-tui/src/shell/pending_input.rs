use std::collections::VecDeque;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingInputState {
    /// Steers already submitted to the runtime but not yet drained.
    pending_steers: VecDeque<String>,
    /// Follow-ups queued while a turn is running (FIFO).
    queued_follow_ups: VecDeque<String>,
    /// Shell commands queued while a turn, compaction, or shell command is running.
    queued_shell_commands: VecDeque<String>,
    /// Runtime `SteeringQueued` events expected for items already shown locally.
    optimistic_steer_acks: VecDeque<String>,
    /// Runtime `FollowUpQueued` events expected for items already shown locally.
    optimistic_follow_up_acks: VecDeque<String>,
    /// Runtime follow-up drain events expected for items already removed locally.
    optimistic_follow_up_drains: usize,
}

impl PendingInputState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a follow-up message that will start a fresh turn after the
    /// current one finishes.
    pub fn queue_follow_up(&mut self, text: impl Into<String>) {
        let text = text.into();
        if consume_expected_ack(&mut self.optimistic_follow_up_acks, &text) {
            return;
        }
        self.queued_follow_ups.push_back(text);
    }

    /// Show a follow-up immediately while waiting for the runtime queue event
    /// that makes it durable.
    pub fn queue_follow_up_optimistic(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.queued_follow_ups.push_back(text.clone());
        self.optimistic_follow_up_acks.push_back(text);
    }

    /// Queue a steer message that will be injected at the next natural break
    /// point in the running turn.
    pub fn queue_steer(&mut self, text: impl Into<String>) {
        let text = text.into();
        if consume_expected_ack(&mut self.optimistic_steer_acks, &text) {
            return;
        }
        self.pending_steers.push_back(text);
    }

    /// Show a steer immediately while waiting for the runtime queue event that
    /// makes it durable.
    pub fn queue_steer_optimistic(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.pending_steers.push_back(text.clone());
        self.optimistic_steer_acks.push_back(text);
    }

    /// Reclassify the oldest visible follow-up as a steer, mirroring the
    /// runtime promotion that will be acknowledged by queue events later.
    pub fn promote_oldest_follow_up_to_steer_optimistic(&mut self) -> Option<String> {
        let text = self.queued_follow_ups.pop_front()?;
        self.pending_steers.push_back(text.clone());
        self.optimistic_follow_up_drains += 1;
        self.optimistic_steer_acks.push_back(text.clone());
        Some(text)
    }

    pub fn queue_shell_command(&mut self, text: impl Into<String>) {
        self.queued_shell_commands.push_back(text.into());
    }

    /// Drain `count` messages from the matching queue (used when the runtime
    /// consumes queued messages).
    pub fn drain(&mut self, kind: neo_agent_core::QueueKind, count: usize) {
        match kind {
            neo_agent_core::QueueKind::Steering => {
                let drain_count = count.min(self.pending_steers.len());
                self.pending_steers.drain(0..drain_count);
            }
            neo_agent_core::QueueKind::FollowUp => {
                let acknowledged = count.min(self.optimistic_follow_up_drains);
                self.optimistic_follow_up_drains -= acknowledged;
                let count = count.saturating_sub(acknowledged);
                let drain_count = count.min(self.queued_follow_ups.len());
                self.queued_follow_ups.drain(0..drain_count);
            }
        }
    }

    /// Dequeue the oldest visible follow-up back into the composer for editing,
    /// without expecting a later runtime drain acknowledgement.
    pub fn dequeue_oldest_follow_up_for_edit(&mut self) -> Option<String> {
        self.queued_follow_ups.pop_front()
    }

    /// Dequeue the oldest visible follow-up back into the composer for editing,
    /// mirroring a runtime drain that will be acknowledged by queue events later.
    pub fn dequeue_oldest_follow_up_for_edit_optimistic(&mut self) -> Option<String> {
        let text = self.dequeue_oldest_follow_up_for_edit()?;
        self.optimistic_follow_up_drains += 1;
        Some(text)
    }

    pub fn drain_next_follow_up(&mut self) -> Option<String> {
        self.queued_follow_ups.pop_front()
    }

    pub fn drain_next_shell_command(&mut self) -> Option<String> {
        self.queued_shell_commands.pop_front()
    }

    pub fn pop_most_recent_shell_command_for_edit(&mut self) -> Option<String> {
        self.queued_shell_commands.pop_back()
    }

    #[must_use]
    pub fn pending_steers(&self) -> &VecDeque<String> {
        &self.pending_steers
    }

    #[must_use]
    pub fn queued_follow_ups(&self) -> &VecDeque<String> {
        &self.queued_follow_ups
    }

    #[must_use]
    pub fn queued_shell_commands(&self) -> &VecDeque<String> {
        &self.queued_shell_commands
    }

    #[must_use]
    pub fn has_queued_shell_commands(&self) -> bool {
        !self.queued_shell_commands.is_empty()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending_steers.is_empty()
            && self.queued_follow_ups.is_empty()
            && self.queued_shell_commands.is_empty()
    }
}

fn consume_expected_ack(expected: &mut VecDeque<String>, text: &str) -> bool {
    if expected.front().is_some_and(|pending| pending == text) {
        expected.pop_front();
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_queue_drains_fifo_but_edits_lifo() {
        let mut state = PendingInputState::new();
        state.queue_shell_command("one");
        state.queue_shell_command("two");
        assert_eq!(state.drain_next_shell_command(), Some("one".to_owned()));
        assert_eq!(
            state.pop_most_recent_shell_command_for_edit(),
            Some("two".to_owned())
        );
        assert!(state.is_empty());
    }

    #[test]
    fn shell_queue_counts_as_pending_input() {
        let mut state = PendingInputState::new();
        state.queue_shell_command("whoami");
        assert!(!state.is_empty());
        assert!(state.has_queued_shell_commands());
    }

    #[test]
    fn optimistic_follow_up_waits_for_drain_without_duplicate_ack() {
        let mut state = PendingInputState::new();
        state.queue_follow_up_optimistic("one");
        state.queue_follow_up("one");

        assert_eq!(
            state
                .queued_follow_ups()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["one"]
        );

        state.drain(neo_agent_core::QueueKind::FollowUp, 1);
        assert!(state.is_empty());
    }

    #[test]
    fn optimistic_promote_ack_does_not_remove_next_follow_up() {
        let mut state = PendingInputState::new();
        state.queue_follow_up("one");
        state.queue_follow_up("two");

        assert_eq!(
            state
                .promote_oldest_follow_up_to_steer_optimistic()
                .as_deref(),
            Some("one")
        );
        state.drain(neo_agent_core::QueueKind::FollowUp, 1);
        state.queue_steer("one");

        assert_eq!(
            state
                .queued_follow_ups()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["two"]
        );
        assert_eq!(
            state
                .pending_steers()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["one"]
        );
    }

    #[test]
    fn optimistic_edit_dequeues_oldest_follow_up_without_ack_removing_next() {
        let mut state = PendingInputState::new();
        state.queue_follow_up("one");
        state.queue_follow_up("two");

        assert_eq!(
            state
                .dequeue_oldest_follow_up_for_edit_optimistic()
                .as_deref(),
            Some("one")
        );
        state.drain(neo_agent_core::QueueKind::FollowUp, 1);

        assert_eq!(
            state
                .queued_follow_ups()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["two"]
        );
    }
}
