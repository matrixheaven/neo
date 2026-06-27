use std::collections::VecDeque;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingInputState {
    /// Steers already submitted to the runtime but not yet drained.
    pending_steers: VecDeque<String>,
    /// Follow-ups queued while a turn is running (FIFO).
    queued_follow_ups: VecDeque<String>,
    /// Shell commands queued while a turn, compaction, or shell command is running.
    queued_shell_commands: VecDeque<String>,
}

impl PendingInputState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a follow-up message that will start a fresh turn after the
    /// current one finishes.
    pub fn queue_follow_up(&mut self, text: impl Into<String>) {
        self.queued_follow_ups.push_back(text.into());
    }

    /// Queue a steer message that will be injected at the next natural break
    /// point in the running turn.
    pub fn queue_steer(&mut self, text: impl Into<String>) {
        self.pending_steers.push_back(text.into());
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
                let drain_count = count.min(self.queued_follow_ups.len());
                self.queued_follow_ups.drain(0..drain_count);
            }
        }
    }

    /// Pop the most recent queued follow-up back into the composer for editing
    /// (LIFO). Returns the text if any was available.
    pub fn pop_most_recent_follow_up_for_edit(&mut self) -> Option<String> {
        self.queued_follow_ups.pop_back()
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
}
