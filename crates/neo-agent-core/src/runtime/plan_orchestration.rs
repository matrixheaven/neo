use std::path::PathBuf;

use super::config::AgentConfig;
use super::events::EventEmitter;
use crate::{AgentEvent, AgentToolCall, ToolResult};

pub(super) fn plan_mode_plans_dir(config: &AgentConfig) -> Option<PathBuf> {
    config
        .session_directory
        .as_deref()
        .map(|dir| dir.join("plans"))
}

pub(super) fn attach_exit_plan_details(
    config: &AgentConfig,
    tool_results: &mut [(AgentToolCall, ToolResult)],
) {
    let pm = config.plan_mode.read().unwrap();
    if !pm.is_active() {
        return;
    }
    let Some(plan_data) = pm.data().ok().flatten() else {
        return;
    };
    let mut selected_labels = config.plan_review_selected_label.lock().ok();
    for (tool_call, result) in tool_results {
        if tool_call.name == "ExitPlanMode" {
            if result.details.is_none() {
                result.details = Some(serde_json::json!({
                    "plan_content": plan_data.content,
                    "plan_path": plan_data.path.display().to_string(),
                }));
            }
            // When the user approved a specific model-supplied option from
            // the plan-review picker, prefix the tool result so the model runs
            // only the selected branch. The label is consumed once.
            if !result.is_error
                && let Some(labels) = selected_labels.as_mut()
                && let Some(label) = labels.remove(&tool_call.id)
                && !label.trim().is_empty()
            {
                result.content = format!(
                    "Selected approach: {label}\n\
                     Execute ONLY the selected approach. Do not execute any unselected alternatives.\n\n{}",
                    result.content
                );
                if let Some(details) = result.details.as_mut()
                    && let Some(obj) = details.as_object_mut()
                {
                    obj.insert(
                        "plan_selected_label".to_string(),
                        serde_json::Value::String(label),
                    );
                }
            }
        }
    }
}

pub(super) fn emit_plan_tool_event(
    turn: u32,
    config: &AgentConfig,
    tool_name: &str,
    result: &ToolResult,
    emitter: &mut EventEmitter,
) {
    if !result.terminate {
        return;
    }
    match tool_name {
        "EnterPlanMode" => emit_plan_mode_entered(turn, config, emitter),
        "ExitPlanMode" => emit_plan_mode_exited(turn, config, emitter),
        _ => {}
    }
}

fn emit_plan_mode_entered(turn: u32, config: &AgentConfig, emitter: &mut EventEmitter) {
    // If plan mode is already active (the enter side-effect was executed
    // earlier in the turn via `enter_plan_mode_state`), skip the duplicate
    // enter and just emit the events with the existing id.
    let id = {
        let pm = config.plan_mode.read().unwrap();
        if pm.is_active() {
            pm.plan_id().unwrap_or("").to_owned()
        } else {
            drop(pm);
            enter_plan_mode_state(config)
        }
    };
    emitter.emit(AgentEvent::PlanModeEntered {
        turn,
        id: id.clone(),
    });
    emitter.emit(AgentEvent::PlanUpdated {
        turn,
        enabled: true,
    });
}

/// Execute the plan-mode enter side-effect (create plan file, set active state)
/// and return the plan id. Extracted so [`attach_enter_plan_details`] can run
/// the side-effect *before* tool results are appended to the model context.
pub(super) fn enter_plan_mode_state(config: &AgentConfig) -> String {
    let mut pm = config.plan_mode.write().unwrap();
    if let Some(plans_dir) = plan_mode_plans_dir(config) {
        pm.enter(&plans_dir, true).map_or_else(
            |_| {
                pm.enter_in_memory();
                pm.plan_id().unwrap_or("").to_owned()
            },
            |data| data.id,
        )
    } else {
        pm.enter_in_memory();
        pm.plan_id().unwrap_or("").to_owned()
    }
}

/// After EnterPlanMode runs and the plan file has been created, inject the plan
/// file path into the tool result so the model knows where to write its plan.
/// Must run *after* [`enter_plan_mode_state`] and *before*
/// [`append_tool_result_messages`].
pub(super) fn attach_enter_plan_details(
    config: &AgentConfig,
    tool_results: &mut [(AgentToolCall, ToolResult)],
) {
    let plan_path = config
        .plan_mode
        .read()
        .ok()
        .and_then(|pm| pm.plan_file_path().map(|p| p.display().to_string()));
    let Some(plan_path) = plan_path else {
        return;
    };
    for (tool_call, result) in tool_results {
        if tool_call.name == "EnterPlanMode" && !result.is_error {
            result.content = format!(
                "{}\n\nThe plan file is at: {plan_path}\nWrite your plan to this file.",
                result.content
            );
        }
    }
}

fn emit_plan_mode_exited(turn: u32, config: &AgentConfig, emitter: &mut EventEmitter) {
    let mut pm = config.plan_mode.write().unwrap();
    let id = pm.plan_id().unwrap_or("").to_owned();
    pm.exit();
    drop(pm);
    emitter.emit(AgentEvent::PlanModeExited { turn, id });
    emitter.emit(AgentEvent::PlanUpdated {
        turn,
        enabled: false,
    });
}

pub(super) fn exit_plan_mode_has_reviewable_plan(config: &AgentConfig) -> bool {
    let Ok(pm) = config.plan_mode.read() else {
        return false;
    };
    if !pm.is_active() {
        return false;
    }
    pm.data()
        .ok()
        .flatten()
        .is_some_and(|data| !data.content.trim().is_empty())
}
