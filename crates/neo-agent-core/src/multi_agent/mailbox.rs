use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DelegateMailboxMessage {
    pub id: String,
    pub text: String,
    pub delivered: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DelegateMailbox {
    messages: VecDeque<DelegateMailboxMessage>,
    next_id: u64,
}

impl DelegateMailbox {
    pub fn push(&mut self, text: String) -> DelegateMailboxMessage {
        self.next_id = self.next_id.max(next_global_message_number());
        self.next_id += 1;
        let message = DelegateMailboxMessage {
            id: format!("msg_{}", self.next_id),
            text,
            delivered: false,
        };
        self.messages.push_back(message.clone());
        message
    }

    pub fn pending(&self) -> Vec<DelegateMailboxMessage> {
        self.messages
            .iter()
            .filter(|message| !message.delivered)
            .cloned()
            .collect()
    }

    pub fn pending_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|message| !message.delivered)
            .count()
    }

    pub fn latest_message_id(&self) -> Option<String> {
        self.messages.back().map(|message| message.id.clone())
    }

    pub fn take_pending(&mut self) -> Vec<DelegateMailboxMessage> {
        let mut pending = Vec::new();
        for message in &mut self.messages {
            if !message.delivered {
                message.delivered = true;
                pending.push(message.clone());
            }
        }
        pending
    }

    pub fn mark_delivered(&mut self, id: &str) {
        if let Some(message) = self.messages.iter_mut().find(|message| message.id == id) {
            message.delivered = true;
        }
    }
}

fn next_global_message_number() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
