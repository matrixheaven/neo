//! Workflow behavior: admission, artifacts, builtins, check, child journal,
//! dispatch, harness, journal, launch, lineage, lua, model visibility, output,
//! recovery dispatch, registry, runtime contract/effects/lifecycle/recovery,
//! schema, swarm, tool policy, and user input (Task 6).

#[path = "workflow_behavior/admission.rs"]
mod admission;
#[path = "workflow_behavior/artifacts.rs"]
mod artifacts;
#[path = "workflow_behavior/builtins.rs"]
mod builtins;
#[path = "workflow_behavior/check.rs"]
mod check;
#[path = "workflow_behavior/child_journal.rs"]
mod child_journal;
#[path = "workflow_behavior/dispatch.rs"]
mod dispatch;
#[path = "workflow_behavior/dispatch_delegate.rs"]
mod dispatch_delegate;
#[path = "workflow_behavior/dispatch_resolver.rs"]
mod dispatch_resolver;
#[path = "workflow_behavior/harness.rs"]
mod harness;
#[path = "workflow_behavior/journal.rs"]
mod journal;
#[path = "workflow_behavior/journal_recovery.rs"]
mod journal_recovery;
#[path = "workflow_behavior/launch.rs"]
mod launch;
#[path = "workflow_behavior/lineage.rs"]
mod lineage;
#[path = "workflow_behavior/lua.rs"]
mod lua;
#[path = "workflow_behavior/model_visibility.rs"]
mod model_visibility;
#[path = "workflow_behavior/output.rs"]
mod output;
#[path = "workflow_behavior/recovery_dispatch.rs"]
mod recovery_dispatch;
#[path = "workflow_behavior/registry.rs"]
mod registry;
#[path = "workflow_behavior/registry_validation.rs"]
mod registry_validation;
#[path = "workflow_behavior/runtime_contract.rs"]
mod runtime_contract;
#[path = "workflow_behavior/runtime_effects.rs"]
mod runtime_effects;
#[path = "workflow_behavior/runtime_lifecycle.rs"]
mod runtime_lifecycle;
#[path = "workflow_behavior/runtime_recovery.rs"]
mod runtime_recovery;
#[path = "workflow_behavior/schema.rs"]
mod schema;
#[path = "workflow_behavior/swarm.rs"]
mod swarm;
#[path = "workflow_behavior/tool_policy.rs"]
mod tool_policy;
#[path = "workflow_behavior/user_input.rs"]
mod user_input;
