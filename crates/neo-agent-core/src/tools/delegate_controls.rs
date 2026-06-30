use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{Tool, ToolContext, ToolFuture, ToolResult, parse_input, schema};

// ---------------------------------------------------------------------------
// ListDelegates
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct ListDelegatesInput {
    #[schemars(
        description = "Whether to include completed/cancelled delegates. Defaults to false (active only)."
    )]
    include_completed: Option<bool>,
}

pub struct ListDelegatesTool;

impl Tool for ListDelegatesTool {
    fn name(&self) -> &'static str {
        "ListDelegates"
    }

    fn description(&self) -> &'static str {
        "List all delegate agents and their current status. Returns a compact summary per agent \
         without dumping full child transcripts."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<ListDelegatesInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: ListDelegatesInput = parse_input(self.name(), input)?;
            let include_completed = input.include_completed.unwrap_or(false);

            let agents = ctx.multi_agent.list_agents(include_completed);
            let tasks = ctx.background_tasks.list(false, 100).await;
            let delegate_tasks: Vec<_> = tasks
                .iter()
                .filter(|t| t.delegate.is_some() && (include_completed || t.status.is_active()))
                .cloned()
                .collect();

            let mut content = format!("delegates: {}\n", agents.len());
            for agent in &agents {
                content.push_str(&format!(
                    "\n- {} ({}) state: {} task: {} | mailbox_pending: {}",
                    agent.id.as_str(),
                    agent.display_name.as_str(),
                    format!("{:?}", agent.state).to_lowercase(),
                    agent.task,
                    ctx.multi_agent.mailbox_pending_count(agent.id.as_str()),
                ));
                if let Some(message_id) =
                    ctx.multi_agent.latest_mailbox_message_id(agent.id.as_str())
                {
                    content.push_str(&format!(" | latest_message_id: {message_id}"));
                }
                if let Some(outcome) = &agent.outcome {
                    content.push_str(&format!(" | summary: {}", outcome.summary));
                }
            }
            if agents.is_empty() && !delegate_tasks.is_empty() {
                for task in &delegate_tasks {
                    content.push_str(&format!(
                        "\n- {} state: {} kind: {}",
                        task.task_id,
                        task.status.as_str(),
                        task.kind.as_str(),
                    ));
                }
            }
            if agents.is_empty() && delegate_tasks.is_empty() {
                content.push_str("\nNo delegates found.");
            }

            Ok(ToolResult::ok(content).with_details(json!({
                "kind": "delegate_list",
                "count": agents.len(),
                "include_completed": include_completed,
            })))
        })
    }
}

// ---------------------------------------------------------------------------
// WaitDelegate
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct WaitDelegateInput {
    #[schemars(description = "The agent or task ID to wait for.")]
    id: String,
    #[schemars(
        description = "Maximum time to wait in milliseconds. Defaults to 30000 (30s). Returns timed_out if the delegate hasn't finished."
    )]
    timeout_ms: Option<u64>,
}

pub struct WaitDelegateTool;

impl Tool for WaitDelegateTool {
    fn name(&self) -> &'static str {
        "WaitDelegate"
    }

    fn description(&self) -> &'static str {
        "Wait for a delegate agent to reach a terminal state (completed, failed, or cancelled). \
         Returns the agent's final status. Times out if the delegate doesn't finish within \
         the specified timeout."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<WaitDelegateInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: WaitDelegateInput = parse_input(self.name(), input)?;
            let timeout = Duration::from_millis(input.timeout_ms.unwrap_or(30_000));
            let deadline = std::time::Instant::now() + timeout;

            loop {
                // Check runtime state by searching agents list for matching ID.
                let agents = ctx.multi_agent.list_agents(true);
                if let Some(snapshot) = agents.iter().find(|a| a.id.as_str() == input.id).cloned() {
                    if matches!(
                        snapshot.state,
                        crate::multi_agent::AgentLifecycleState::Completed
                            | crate::multi_agent::AgentLifecycleState::Failed
                            | crate::multi_agent::AgentLifecycleState::Cancelled
                    ) {
                        let state_label = format!("{:?}", snapshot.state).to_lowercase();
                        let summary = snapshot
                            .outcome
                            .as_ref()
                            .map(|o| o.summary.clone())
                            .unwrap_or_default();
                        return Ok(ToolResult::ok(format!(
                            "id: {}\nstatus: {}\nmailbox_pending: {}\nsummary: {}",
                            snapshot.id.as_str(),
                            state_label,
                            ctx.multi_agent.mailbox_pending_count(snapshot.id.as_str()),
                            summary,
                        ))
                        .with_details(json!({
                            "kind": "delegate_wait",
                            "agent": snapshot,
                            "outcome": "completed",
                        })));
                    }
                }

                // Also check background task state.
                if let Ok(task_snap) = ctx.background_tasks.snapshot(&input.id).await {
                    if !task_snap.status.is_active() {
                        return Ok(ToolResult::ok(format!(
                            "id: {}\nstatus: {}\noutcome: completed",
                            input.id,
                            task_snap.status.as_str(),
                        ))
                        .with_details(json!({
                            "kind": "delegate_wait",
                            "task_id": input.id,
                            "outcome": "completed",
                        })));
                    }
                }

                if std::time::Instant::now() >= deadline {
                    return Ok(ToolResult::ok(format!(
                        "id: {}\nstatus: timed_out\nnext_step: The delegate is still running. Increase the timeout or use ListDelegates to check status.",
                        input.id,
                    ))
                    .with_details(json!({
                        "kind": "delegate_wait",
                        "task_id": input.id,
                        "outcome": "timed_out",
                    })));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
    }
}

// ---------------------------------------------------------------------------
// InterruptDelegate
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct InterruptDelegateInput {
    #[schemars(description = "The agent or task ID to interrupt.")]
    id: String,
}

pub struct InterruptDelegateTool;

impl Tool for InterruptDelegateTool {
    fn name(&self) -> &'static str {
        "InterruptDelegate"
    }

    fn description(&self) -> &'static str {
        "Interrupt and cancel a running delegate agent or swarm. The agent is marked as cancelled."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<InterruptDelegateInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: InterruptDelegateInput = parse_input(self.name(), input)?;

            // Find the agent by ID in the runtime.
            let agents = ctx.multi_agent.list_agents(true);
            if let Some(agent) = agents.iter().find(|a| a.id.as_str() == input.id).cloned() {
                let agent_id = agent.id.clone();
                if let Some(snapshot) = ctx.multi_agent.cancel_agent(&agent_id) {
                    let _ = ctx
                        .background_tasks
                        .cancel_delegate(&input.id, snapshot.clone())
                        .await;
                    return Ok(ToolResult::ok(format!(
                        "id: {}\nstatus: cancelled\nname: {}",
                        snapshot.id.as_str(),
                        snapshot.display_name.as_str(),
                    ))
                    .with_details(json!({
                        "kind": "delegate_interrupt",
                        "agent": snapshot,
                        "outcome": "cancelled",
                    })));
                }
            }

            // Fall back to background task stop.
            match ctx
                .background_tasks
                .stop(&input.id, "Interrupted by InterruptDelegate", 1024)
                .await
            {
                Ok(result) => Ok(result),
                Err(err) => Ok(ToolResult::error(format!(
                    "id: {}\nerror: {}",
                    input.id, err
                ))),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// MessageDelegate
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct MessageDelegateInput {
    #[schemars(description = "The agent or swarm ID to message.")]
    id: String,
    #[schemars(description = "The message text to deliver.")]
    message: String,
}

pub struct MessageDelegateTool;

impl Tool for MessageDelegateTool {
    fn name(&self) -> &'static str {
        "MessageDelegate"
    }

    fn description(&self) -> &'static str {
        "Send a follow-up message to a background or idle delegate agent. \
         The message is queued in the agent's mailbox and does not auto-inject \
         a large transcript into the parent context."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<MessageDelegateInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: MessageDelegateInput = parse_input(self.name(), input)?;

            // Verify the target exists in the runtime.
            let agents = ctx.multi_agent.list_agents(true);
            let target = agents.iter().find(|a| a.id.as_str() == input.id);

            let target_name = if let Some(agent) = target {
                agent.display_name.as_str().to_owned()
            } else {
                input.id.clone()
            };

            let Some(msg) = ctx
                .multi_agent
                .push_mailbox_message(&input.id, input.message)
            else {
                return Ok(ToolResult::error(format!(
                    "id: {}\nerror: unknown delegate target",
                    input.id
                ))
                .with_details(json!({
                    "kind": "delegate_message",
                    "target": input.id,
                    "outcome": "unknown_target",
                })));
            };
            let delivered_live = ctx.multi_agent.deliver_live_message(&input.id, &msg);
            if delivered_live {
                ctx.multi_agent
                    .mark_mailbox_message_delivered(&input.id, &msg.id);
            }
            let outcome = if delivered_live {
                "delivered"
            } else {
                "queued"
            };
            let next_step = if delivered_live {
                "message delivered to the running delegate and will be injected at the next model boundary"
            } else {
                "queued messages are delivered before a delegate run starts"
            };

            Ok(ToolResult::ok(format!(
                "target: {}\nstatus: {}\nmessage_id: {}\nmailbox_pending: {}\nnext_step: {}.",
                target_name,
                outcome,
                msg.id,
                ctx.multi_agent.mailbox_pending_count(&input.id),
                next_step,
            ))
            .with_details(json!({
                "kind": "delegate_message",
                "target": target_name,
                "message_id": msg.id,
                "delivered_live": delivered_live,
                "mailbox_pending_count": ctx.multi_agent.mailbox_pending_count(&input.id),
                "outcome": outcome,
            })))
        })
    }
}
