use neo_agent_core::PermissionMode;
use neo_tui::dialogs::ChoiceItem;

use super::InlineSkillDirectives;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WorkflowId {
    Init,
    Skill(String),
}

impl WorkflowId {
    pub(super) fn key(&self) -> String {
        match self {
            Self::Init => "init".to_owned(),
            Self::Skill(name) => format!("skill:{name}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PendingInteractiveWorkflow {
    Init {
        instruction: String,
    },
    Skill {
        directives: InlineSkillDirectives,
        generated_prompt: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AutoModePolicy {
    OptionalClarification { best_effort_note: String },
    RequiredClarification { missing_input: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SkillPreflightDecision {
    Ready,
    InvalidUsage,
    Open {
        spec: InteractivePreflightSpec,
        generated_prompt: Option<String>,
    },
    Blocked(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreflightAction {
    SwitchPermissionMode(PermissionMode),
    ContinueAutoBestEffort,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PreflightChoice {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) description: String,
    pub(super) action: PreflightAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InteractivePreflightSpec {
    pub(super) workflow_id: WorkflowId,
    pub(super) title: String,
    pub(super) body: String,
    pub(super) recommended: PreflightChoice,
    pub(super) alternate: Option<PreflightChoice>,
    pub(super) cancel: PreflightChoice,
    pub(super) auto_mode_policy: AutoModePolicy,
}

impl InteractivePreflightSpec {
    pub(super) fn initial_id(&self) -> String {
        self.recommended.id.clone()
    }

    pub(super) fn choices(&self) -> Vec<PreflightChoice> {
        let mut choices = vec![self.recommended.clone()];
        if let Some(alternate) = &self.alternate {
            choices.push(alternate.clone());
        }
        choices.push(self.cancel.clone());
        choices
    }

    pub(super) fn choice_items(&self) -> Vec<ChoiceItem> {
        self.choices()
            .into_iter()
            .map(|choice| {
                ChoiceItem::new(choice.id, choice.label).with_description(choice.description)
            })
            .collect()
    }

    pub(super) fn action_for_choice(&self, id: &str) -> Option<PreflightAction> {
        self.choices()
            .into_iter()
            .find_map(|choice| (choice.id == id).then_some(choice.action))
    }
}

pub(super) fn init_preflight() -> InteractivePreflightSpec {
    optional_preflight(
        WorkflowId::Init,
        "Switch to Ask mode?",
        "Generating a strong AGENTS.md usually requires asking about reference locations, project preferences, and durable workflow rules.",
        "Switch to Ask and start",
        "Ask mode lets the workflow clarify missing project guidance before writing.",
        "Stay Auto and generate best effort",
        "Start /init without user questions. The agent will proceed with explicit best-effort assumptions.",
        "Do not start /init.",
    )
}

pub(super) fn preflight_for_skill_directives(
    directives: &InlineSkillDirectives,
) -> Result<Option<(InteractivePreflightSpec, Option<String>)>, String> {
    let required = directives
        .invocations
        .iter()
        .filter_map(|invocation| required_skill_preflight(&invocation.name, &invocation.args))
        .collect::<Vec<_>>();
    if required.len() > 1 {
        return Err("Run one interactive skill workflow at a time".to_owned());
    }
    Ok(required.into_iter().next())
}

pub(super) fn skill_preflight_decision(
    directives: &InlineSkillDirectives,
    permission_mode: PermissionMode,
) -> SkillPreflightDecision {
    if directives
        .invocations
        .iter()
        .any(|invocation| invocation.name.is_empty())
    {
        return SkillPreflightDecision::InvalidUsage;
    }
    if permission_mode != PermissionMode::Auto {
        return SkillPreflightDecision::Ready;
    }
    match preflight_for_skill_directives(directives) {
        Ok(Some((spec, generated_prompt))) => SkillPreflightDecision::Open {
            spec,
            generated_prompt,
        },
        Ok(None) => SkillPreflightDecision::Ready,
        Err(message) => SkillPreflightDecision::Blocked(message),
    }
}

pub(super) fn auto_best_effort_note() -> &'static str {
    "Auto permission mode remained active. User questions are unavailable during this workflow. Proceed with explicit best-effort assumptions and report any assumption that materially affects the result."
}

fn required_skill_preflight(
    name: &str,
    args: &str,
) -> Option<(InteractivePreflightSpec, Option<String>)> {
    match (name, args.trim().is_empty()) {
        ("self-evo", true) => Some((
            required_preflight(
                WorkflowId::Skill("self-evo".to_owned()),
                "Choose self-evo scope?",
                "self-evo writes reusable skills. It needs a scope before it can safely distill recent work.",
                "Switch to Ask and choose scope",
                "Ask mode lets self-evo ask whether to use the current session, recent sessions, or a specific session/topic.",
                "Do not start self-evo.",
                "scope is required before distillation",
            ),
            Some("Ask me which session scope to distill before creating any skill.".to_owned()),
        )),
        ("create-skill", true) => Some((
            required_preflight(
                WorkflowId::Skill("create-skill".to_owned()),
                "Describe the skill to create?",
                "create-skill writes a persistent skill. It needs your requirement before it can create one safely.",
                "Switch to Ask and describe it",
                "Ask mode lets create-skill ask what capability, inputs, outputs, and verification the skill needs.",
                "Do not start create-skill.",
                "skill requirement is required before authoring",
            ),
            Some("Ask me what skill to create before drafting or calling CreateSkill.".to_owned()),
        )),
        _ => None,
    }
}

fn optional_preflight(
    workflow_id: WorkflowId,
    title: &str,
    body: &str,
    recommended_label: &str,
    recommended_description: &str,
    alternate_label: &str,
    alternate_description: &str,
    cancel_description: &str,
) -> InteractivePreflightSpec {
    let key = workflow_id.key();
    InteractivePreflightSpec {
        workflow_id,
        title: title.to_owned(),
        body: body.to_owned(),
        recommended: PreflightChoice {
            id: format!("preflight:{key}:switch-ask"),
            label: recommended_label.to_owned(),
            description: recommended_description.to_owned(),
            action: PreflightAction::SwitchPermissionMode(PermissionMode::Ask),
        },
        alternate: Some(PreflightChoice {
            id: format!("preflight:{key}:continue-auto"),
            label: alternate_label.to_owned(),
            description: alternate_description.to_owned(),
            action: PreflightAction::ContinueAutoBestEffort,
        }),
        cancel: PreflightChoice {
            id: format!("preflight:{key}:cancel"),
            label: "Cancel".to_owned(),
            description: cancel_description.to_owned(),
            action: PreflightAction::Cancel,
        },
        auto_mode_policy: AutoModePolicy::OptionalClarification {
            best_effort_note: auto_best_effort_note().to_owned(),
        },
    }
}

fn required_preflight(
    workflow_id: WorkflowId,
    title: &str,
    body: &str,
    recommended_label: &str,
    recommended_description: &str,
    cancel_description: &str,
    missing_input: &str,
) -> InteractivePreflightSpec {
    let key = workflow_id.key();
    InteractivePreflightSpec {
        workflow_id,
        title: title.to_owned(),
        body: body.to_owned(),
        recommended: PreflightChoice {
            id: format!("preflight:{key}:switch-ask"),
            label: recommended_label.to_owned(),
            description: recommended_description.to_owned(),
            action: PreflightAction::SwitchPermissionMode(PermissionMode::Ask),
        },
        alternate: None,
        cancel: PreflightChoice {
            id: format!("preflight:{key}:cancel"),
            label: "Cancel".to_owned(),
            description: cancel_description.to_owned(),
            action: PreflightAction::Cancel,
        },
        auto_mode_policy: AutoModePolicy::RequiredClarification {
            missing_input: missing_input.to_owned(),
        },
    }
}
