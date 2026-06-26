use std::sync::{Arc, RwLock};

use crate::mode::{PlanInjectionVariant, PlanMode};
use crate::{AgentContext, AgentMessage};

#[derive(Debug, Clone)]
pub struct PlanModeInjector {
    plan_mode: Arc<RwLock<PlanMode>>,
}

impl PlanModeInjector {
    #[must_use]
    pub fn new(plan_mode: Arc<RwLock<PlanMode>>) -> Self {
        Self { plan_mode }
    }

    /// Returns an injected plan-mode reminder message if one is due for this
    /// turn, or `None`.
    pub fn inject(&mut self, context: &AgentContext) -> Option<AgentMessage> {
        let user_msg = matches!(context.messages().last(), Some(AgentMessage::User { .. }));
        let asst_count = u32::try_from(
            context
                .messages()
                .iter()
                .filter(|m| matches!(m, AgentMessage::Assistant { .. }))
                .count(),
        )
        .unwrap_or(u32::MAX);

        let mut pm = self.plan_mode.write().expect("plan mode lock poisoned");
        let variant = pm.next_injection_variant(asst_count, user_msg);
        let Some(variant) = variant else {
            pm.increment_assistant_turns();
            return None;
        };

        let path = pm
            .plan_file_path()
            .map_or_else(|| "(no plan file)".to_string(), |p| p.display().to_string());
        Some(AgentMessage::system_text(reminder_text(variant, &path)))
    }
}

fn reminder_text(variant: PlanInjectionVariant, path: &str) -> String {
    match variant {
        PlanInjectionVariant::Full => format!(
"Plan mode is active. You MUST NOT make any edits (with the exception of the current plan file) or otherwise make changes to the system unless a tool request is explicitly approved. Prefer read-only tools. Use Bash only when needed; Bash follows the normal permission mode and rules. This supersedes any other instructions you have received.\n\n\
Workflow:\n\
  1. Understand — explore the codebase with Glob, Grep, Read.\n\
  2. Design — converge on the best approach; consider trade-offs but aim for a single recommendation.\n\
  3. Review — re-read key files to verify understanding.\n\
  4. Write Plan — modify the plan file with Write or Edit. Use Write if the plan file does not exist yet.\n\
  5. Exit — call ExitPlanMode for user approval.\n\n\
AskUserQuestion is for clarifying missing requirements or user preferences that affect the plan.\n\
Never ask about plan approval via text or AskUserQuestion.\n\
Your turn must end with either AskUserQuestion (to clarify requirements or preferences) or ExitPlanMode (to request plan approval). Do NOT end your turn any other way.\n\n\
Plan file: {path}"
        ),
        PlanInjectionVariant::Sparse => format!(
"Plan mode still active (see full instructions). Prefer read-only tools except the current plan file. Use Write or Edit to modify the plan file. If it does not exist yet, create it with Write first. Use Bash only when needed; Bash follows the normal permission mode and rules. End turns with AskUserQuestion (for clarifications) or ExitPlanMode (for approval).\n\n\
Plan file: {path}"
        ),
        PlanInjectionVariant::Reentry => format!(
"Plan mode is active. You MUST NOT make any edits (with the exception of the current plan file) or otherwise make changes to the system unless a tool request is explicitly approved. Prefer read-only tools. Use Bash only when needed; Bash follows the normal permission mode and rules. This supersedes any other instructions you have received.\n\n\
## Re-entering Plan Mode\n\
A plan file from a previous planning session already exists.\n\
Before proceeding:\n\
  1. Read the existing plan file to understand what was previously planned.\n\
  2. Evaluate the user's current request against that plan.\n\
  3. If different task: replace the old plan with a fresh one. If same task: update the existing plan.\n\
  4. You may use Write or Edit to modify the plan file.\n\
  5. Always edit the plan file before calling ExitPlanMode.\n\n\
Plan file: {path}"
        ),
        PlanInjectionVariant::Exit => "Plan mode is no longer active. The read-only and plan-file-only restrictions from plan mode no longer apply. Continue with the approved plan using the normal tool and permission rules.".to_string(),
    }
}
