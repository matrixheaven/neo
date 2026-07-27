use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolFuture, ToolResult, schema};
use crate::WorkflowApprovalPresentation;
use crate::workflow::{
    DynamicWorkflowDefinitionInput, LaunchAuthorizationMode, ResolvedWorkflowDefinition,
    WorkflowActor, WorkflowDefinitionRegistry, WorkflowError, WorkflowErrorCode,
    WorkflowLaunchBinding, WorkflowLaunchCoordinator, WorkflowLaunchHosts, WorkflowLaunchIntent,
    WorkflowLaunchRequest, WorkflowListScope, WorkflowPhase, WorkflowSaveRequest,
    WorkflowSaveScope, canonical_input_hash, resolve_dynamic_definition,
};

const DEFAULT_LIST_LIMIT: u32 = 20;
const MAX_LIST_LIMIT: u32 = 100;
const LIST_CURSOR_PREFIX: &str = "workflow-list-v1:";

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkflowAction {
    List,
    Show,
    ValidateInline,
    ValidateSaved,
    Save,
    RunInline,
    RunSaved,
}

impl WorkflowAction {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Show => "show",
            Self::ValidateInline => "validate_inline",
            Self::ValidateSaved => "validate_saved",
            Self::Save => "save",
            Self::RunInline => "run_inline",
            Self::RunSaved => "run_saved",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "list" => Some(Self::List),
            "show" => Some(Self::Show),
            "validate_inline" => Some(Self::ValidateInline),
            "validate_saved" => Some(Self::ValidateSaved),
            "save" => Some(Self::Save),
            "run_inline" => Some(Self::RunInline),
            "run_saved" => Some(Self::RunSaved),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkflowScope {
    User,
    Project,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkflowInput {
    #[schemars(description = "Workflow action to perform.")]
    action: WorkflowAction,
    #[serde(default)]
    #[schemars(description = "Saved or inline workflow name.")]
    name: Option<String>,
    #[serde(default)]
    #[schemars(description = "What the inline or saved workflow orchestrates.")]
    description: Option<String>,
    #[serde(default)]
    #[schemars(description = "Ordered workflow phase declarations.")]
    phases: Option<Vec<WorkflowPhase>>,
    #[serde(default)]
    #[schemars(description = "Complete Lua workflow source.")]
    script: Option<String>,
    #[serde(default)]
    #[schemars(
        with = "std::collections::BTreeMap<String, serde_json::Value>",
        description = "JSON Schema for workflow arguments."
    )]
    input_schema: Option<Value>,
    #[serde(default)]
    #[schemars(
        with = "std::collections::BTreeMap<String, serde_json::Value>",
        description = "JSON Schema for the final workflow result."
    )]
    output_schema: Option<Value>,
    #[serde(default)]
    #[schemars(
        with = "std::collections::BTreeMap<String, serde_json::Value>",
        description = "Object passed to the workflow when it runs."
    )]
    args: Option<Value>,
    #[serde(default)]
    #[schemars(description = "Saved-definition scope: user or project.")]
    scope: Option<WorkflowScope>,
    #[serde(default)]
    #[schemars(description = "Replace an existing saved definition. Defaults to false.")]
    replace: Option<bool>,
    #[serde(default)]
    #[schemars(description = "Opaque cursor returned by a previous list action.")]
    cursor: Option<String>,
    #[serde(default)]
    #[schemars(description = "Requested list page size; the host clamps it to 100.")]
    limit: Option<u32>,
}

#[derive(Debug, Clone)]
pub(crate) enum PreparedWorkflowAction {
    List {
        scope: WorkflowListScope,
        offset: usize,
        limit: usize,
    },
    Show {
        name: String,
    },
    ValidateInline {
        definition: DynamicWorkflowDefinitionInput,
    },
    ValidateSaved {
        name: String,
    },
    Save {
        request: WorkflowSaveRequest,
        scope: WorkflowSaveScope,
        replace: bool,
    },
    RunInline {
        definition: DynamicWorkflowDefinitionInput,
        args: Value,
    },
    RunSaved {
        name: String,
        args: Value,
    },
}

impl PreparedWorkflowAction {
    pub(crate) const fn action(&self) -> WorkflowAction {
        match self {
            Self::List { .. } => WorkflowAction::List,
            Self::Show { .. } => WorkflowAction::Show,
            Self::ValidateInline { .. } => WorkflowAction::ValidateInline,
            Self::ValidateSaved { .. } => WorkflowAction::ValidateSaved,
            Self::Save { .. } => WorkflowAction::Save,
            Self::RunInline { .. } => WorkflowAction::RunInline,
            Self::RunSaved { .. } => WorkflowAction::RunSaved,
        }
    }
}

#[derive(Debug)]
pub(crate) struct WorkflowInputError {
    action: Option<WorkflowAction>,
    field: Option<&'static str>,
    message: String,
    code: &'static str,
}

impl WorkflowInputError {
    fn action(action: Option<WorkflowAction>, message: impl Into<String>) -> Self {
        Self {
            action,
            field: Some("action"),
            message: message.into(),
            code: "workflow_action_invalid",
        }
    }

    fn input(
        action: WorkflowAction,
        field: Option<&'static str>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            action: Some(action),
            field,
            message: message.into(),
            code: "workflow_input_invalid",
        }
    }
}

fn required_string(
    action: WorkflowAction,
    field: &'static str,
    value: &Option<String>,
) -> Result<String, WorkflowInputError> {
    let Some(value) = value.as_deref() else {
        return Err(WorkflowInputError::input(
            action,
            Some(field),
            format!("{field} is required for {}", action.as_str()),
        ));
    };
    if value.trim().is_empty() {
        return Err(WorkflowInputError::input(
            action,
            Some(field),
            format!("{field} must not be empty"),
        ));
    }
    Ok(value.to_owned())
}

fn required_object(
    action: WorkflowAction,
    field: &'static str,
    value: &Option<Value>,
) -> Result<Value, WorkflowInputError> {
    let Some(value) = value else {
        return Err(WorkflowInputError::input(
            action,
            Some(field),
            format!("{field} is required for {}", action.as_str()),
        ));
    };
    if !value.is_object() {
        return Err(WorkflowInputError::input(
            action,
            Some(field),
            format!("{field} must be a JSON object"),
        ));
    }
    Ok(value.clone())
}

fn optional_args(
    action: WorkflowAction,
    value: &Option<Value>,
) -> Result<Value, WorkflowInputError> {
    let value = value.clone().unwrap_or_else(|| json!({}));
    if !value.is_object() {
        return Err(WorkflowInputError::input(
            action,
            Some("args"),
            "args must be a JSON object",
        ));
    }
    Ok(value)
}

fn required_phases(
    action: WorkflowAction,
    value: &Option<Vec<WorkflowPhase>>,
) -> Result<Vec<WorkflowPhase>, WorkflowInputError> {
    value.clone().ok_or_else(|| {
        WorkflowInputError::input(
            action,
            Some("phases"),
            format!("phases is required for {}", action.as_str()),
        )
    })
}

fn reject_fields(
    action: WorkflowAction,
    fields: &[(&'static str, bool)],
) -> Result<(), WorkflowInputError> {
    if let Some((field, _)) = fields.iter().find(|(_, present)| *present) {
        return Err(WorkflowInputError::input(
            action,
            Some(*field),
            format!("{field} is not allowed for {}", action.as_str()),
        ));
    }
    Ok(())
}

fn inline_definition(
    input: &WorkflowInput,
    action: WorkflowAction,
) -> Result<DynamicWorkflowDefinitionInput, WorkflowInputError> {
    Ok(DynamicWorkflowDefinitionInput {
        name: required_string(action, "name", &input.name)?,
        display_name: None,
        description: required_string(action, "description", &input.description)?,
        phases: required_phases(action, &input.phases)?,
        script: required_string(action, "script", &input.script)?,
        input_schema: Some(required_object(
            action,
            "input_schema",
            &input.input_schema,
        )?),
        output_schema: required_object(action, "output_schema", &input.output_schema)?,
    })
}

fn expected_shape(action: Option<WorkflowAction>) -> Value {
    let Some(action) = action else {
        return json!({
            "action": [
                "list", "show", "validate_inline", "validate_saved", "save", "run_inline",
                "run_saved"
            ]
        });
    };
    match action {
        WorkflowAction::List => json!({
            "required": ["action"],
            "optional": ["scope", "cursor", "limit"]
        }),
        WorkflowAction::Show | WorkflowAction::ValidateSaved => json!({
            "required": ["action", "name"],
            "optional": []
        }),
        WorkflowAction::ValidateInline => json!({
            "required": [
                "action", "name", "description", "phases", "script", "input_schema",
                "output_schema"
            ],
            "optional": []
        }),
        WorkflowAction::Save => json!({
            "required": [
                "action", "name", "description", "phases", "script", "input_schema",
                "output_schema", "scope"
            ],
            "optional": ["replace"]
        }),
        WorkflowAction::RunInline => json!({
            "required": [
                "action", "name", "description", "phases", "script", "input_schema",
                "output_schema"
            ],
            "optional": ["args"]
        }),
        WorkflowAction::RunSaved => json!({
            "required": ["action", "name"],
            "optional": ["args"]
        }),
    }
}

fn parse_cursor(action: WorkflowAction, cursor: Option<&str>) -> Result<usize, WorkflowInputError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    cursor
        .strip_prefix(LIST_CURSOR_PREFIX)
        .and_then(|offset| offset.parse::<usize>().ok())
        .ok_or_else(|| {
            WorkflowInputError::input(
                action,
                Some("cursor"),
                "cursor is not a valid Workflow list cursor",
            )
        })
}

pub(crate) fn prepare_action(input: &Value) -> Result<PreparedWorkflowAction, WorkflowInputError> {
    let action = match input.get("action") {
        Some(Value::String(action)) => WorkflowAction::parse(action).ok_or_else(|| {
            WorkflowInputError::action(None, format!("unknown workflow action `{action}`"))
        })?,
        Some(Value::Null) | None => {
            return Err(WorkflowInputError::action(
                None,
                "action is required and must not be null",
            ));
        }
        Some(_) => {
            return Err(WorkflowInputError::action(None, "action must be a string"));
        }
    };
    let input = serde_json::from_value::<WorkflowInput>(input.clone()).map_err(|error| {
        WorkflowInputError::input(action, None, format!("invalid input: {error}"))
    })?;
    debug_assert_eq!(input.action, action);

    let definition_fields = [
        ("name", input.name.is_some()),
        ("description", input.description.is_some()),
        ("phases", input.phases.is_some()),
        ("script", input.script.is_some()),
        ("input_schema", input.input_schema.is_some()),
        ("output_schema", input.output_schema.is_some()),
    ];
    let paging_fields = [
        ("cursor", input.cursor.is_some()),
        ("limit", input.limit.is_some()),
    ];

    match action {
        WorkflowAction::List => {
            reject_fields(
                action,
                &[
                    definition_fields[0],
                    definition_fields[1],
                    definition_fields[2],
                    definition_fields[3],
                    definition_fields[4],
                    definition_fields[5],
                    ("args", input.args.is_some()),
                    ("replace", input.replace.is_some()),
                ],
            )?;
            let requested_limit = input.limit.unwrap_or(DEFAULT_LIST_LIMIT);
            if requested_limit == 0 {
                return Err(WorkflowInputError::input(
                    action,
                    Some("limit"),
                    "limit must be greater than zero",
                ));
            }
            let scope = match input.scope {
                Some(WorkflowScope::User) => WorkflowListScope::User,
                Some(WorkflowScope::Project) => WorkflowListScope::Project,
                None => WorkflowListScope::Effective,
            };
            Ok(PreparedWorkflowAction::List {
                scope,
                offset: parse_cursor(action, input.cursor.as_deref())?,
                limit: requested_limit.min(MAX_LIST_LIMIT) as usize,
            })
        }
        WorkflowAction::Show => {
            reject_fields(
                action,
                &[
                    definition_fields[1],
                    definition_fields[2],
                    definition_fields[3],
                    definition_fields[4],
                    definition_fields[5],
                    ("args", input.args.is_some()),
                    ("scope", input.scope.is_some()),
                    ("replace", input.replace.is_some()),
                    paging_fields[0],
                    paging_fields[1],
                ],
            )?;
            Ok(PreparedWorkflowAction::Show {
                name: required_string(action, "name", &input.name)?,
            })
        }
        WorkflowAction::ValidateInline => {
            reject_fields(
                action,
                &[
                    ("args", input.args.is_some()),
                    ("scope", input.scope.is_some()),
                    ("replace", input.replace.is_some()),
                    paging_fields[0],
                    paging_fields[1],
                ],
            )?;
            Ok(PreparedWorkflowAction::ValidateInline {
                definition: inline_definition(&input, action)?,
            })
        }
        WorkflowAction::ValidateSaved => {
            reject_fields(
                action,
                &[
                    definition_fields[1],
                    definition_fields[2],
                    definition_fields[3],
                    definition_fields[4],
                    definition_fields[5],
                    ("args", input.args.is_some()),
                    ("scope", input.scope.is_some()),
                    ("replace", input.replace.is_some()),
                    paging_fields[0],
                    paging_fields[1],
                ],
            )?;
            Ok(PreparedWorkflowAction::ValidateSaved {
                name: required_string(action, "name", &input.name)?,
            })
        }
        WorkflowAction::Save => {
            reject_fields(
                action,
                &[
                    ("args", input.args.is_some()),
                    paging_fields[0],
                    paging_fields[1],
                ],
            )?;
            let scope = match input.scope {
                Some(WorkflowScope::User) => WorkflowSaveScope::User,
                Some(WorkflowScope::Project) => WorkflowSaveScope::Project,
                None => {
                    return Err(WorkflowInputError::input(
                        action,
                        Some("scope"),
                        "scope is required for save",
                    ));
                }
            };
            let definition = inline_definition(&input, action)?;
            Ok(PreparedWorkflowAction::Save {
                request: WorkflowSaveRequest {
                    display_name: definition.name.clone(),
                    name: definition.name,
                    description: definition.description,
                    phases: definition.phases,
                    lua_source: definition.script,
                    input_schema: definition.input_schema,
                    output_schema: definition.output_schema,
                },
                scope,
                replace: input.replace.unwrap_or(false),
            })
        }
        WorkflowAction::RunInline => {
            reject_fields(
                action,
                &[
                    ("scope", input.scope.is_some()),
                    ("replace", input.replace.is_some()),
                    paging_fields[0],
                    paging_fields[1],
                ],
            )?;
            Ok(PreparedWorkflowAction::RunInline {
                definition: inline_definition(&input, action)?,
                args: optional_args(action, &input.args)?,
            })
        }
        WorkflowAction::RunSaved => {
            reject_fields(
                action,
                &[
                    definition_fields[1],
                    definition_fields[2],
                    definition_fields[3],
                    definition_fields[4],
                    definition_fields[5],
                    ("scope", input.scope.is_some()),
                    ("replace", input.replace.is_some()),
                    paging_fields[0],
                    paging_fields[1],
                ],
            )?;
            Ok(PreparedWorkflowAction::RunSaved {
                name: required_string(action, "name", &input.name)?,
                args: optional_args(action, &input.args)?,
            })
        }
    }
}

fn input_error_result(error: WorkflowInputError) -> ToolResult {
    let action = error.action.map(WorkflowAction::as_str);
    ToolResult::error(error.message.clone()).with_details(json!({
        "ok": false,
        "action": action,
        "status": "error",
        "error": {
            "code": error.code,
            "message": error.message,
            "field": error.field,
            "expected_shape": expected_shape(error.action),
            "side_effect_occurred": false,
        },
        "next_actions": [],
    }))
}

pub(crate) fn invalid_input_result(message: impl Into<String>) -> ToolResult {
    input_error_result(WorkflowInputError::input(
        WorkflowAction::RunInline,
        None,
        message,
    ))
}

fn workflow_error_code(action: WorkflowAction, error: &WorkflowError) -> &'static str {
    match error.code() {
        WorkflowErrorCode::DefinitionNotFound | WorkflowErrorCode::NotFound => "workflow_not_found",
        WorkflowErrorCode::DefinitionConflict => "workflow_conflict",
        WorkflowErrorCode::DefinitionSavePartial => "workflow_save_failed",
        WorkflowErrorCode::UntrustedProjectDefinition => "workflow_scope_untrusted",
        WorkflowErrorCode::StorageAdmissionDenied => "workflow_admission_waiting",
        WorkflowErrorCode::InvalidDefinition
        | WorkflowErrorCode::InvalidManifest
        | WorkflowErrorCode::InvalidSchema
        | WorkflowErrorCode::InputSchemaInvalid
        | WorkflowErrorCode::LuaCompileFailed
        | WorkflowErrorCode::SchemaInvalid => {
            if matches!(
                action,
                WorkflowAction::ValidateInline | WorkflowAction::ValidateSaved
            ) {
                "workflow_validation_failed"
            } else {
                "workflow_definition_invalid"
            }
        }
        WorkflowErrorCode::InvalidInput => "workflow_input_invalid",
        _ if matches!(action, WorkflowAction::RunInline | WorkflowAction::RunSaved) => {
            "workflow_launch_failed"
        }
        _ => "workflow_feature_unavailable",
    }
}

fn workflow_error_result_with_context(
    action: WorkflowAction,
    error: WorkflowError,
    field: Option<&'static str>,
    side_effect_occurred: bool,
    supplied_next_actions: Option<Value>,
) -> ToolResult {
    let code = workflow_error_code(action, &error);
    let save_conflict = action == WorkflowAction::Save && code == "workflow_conflict";
    let message = if save_conflict {
        "workflow definition already exists; retry the same save with replace set to true"
            .to_owned()
    } else {
        error.to_string()
    };
    let next_actions = supplied_next_actions.unwrap_or_else(|| match code {
        "workflow_not_found" => json!([{
            "tool": "Workflow",
            "arguments": {"action": "list"},
            "reason": "List the trusted saved workflows and choose an existing name."
        }]),
        _ => json!([]),
    });
    ToolResult::error(message.clone()).with_details(json!({
        "ok": false,
        "action": action.as_str(),
        "status": "error",
        "error": {
            "code": code,
            "message": message,
            "field": field,
            "expected_shape": expected_shape(Some(action)),
            "side_effect_occurred": side_effect_occurred,
        },
        "next_actions": next_actions,
    }))
}

fn workflow_error_result(action: WorkflowAction, error: WorkflowError) -> ToolResult {
    workflow_error_result_with_context(action, error, None, false, None)
}

fn workflow_save_error_result(error: WorkflowError, next_actions: Option<Value>) -> ToolResult {
    let side_effect_occurred = error.code() == WorkflowErrorCode::DefinitionSavePartial;
    workflow_error_result_with_context(
        WorkflowAction::Save,
        error,
        None,
        side_effect_occurred,
        next_actions,
    )
}

fn feature_error(action: WorkflowAction, message: impl Into<String>) -> ToolResult {
    let message = message.into();
    ToolResult::error(message.clone()).with_details(json!({
        "ok": false,
        "action": action.as_str(),
        "status": "error",
        "error": {
            "code": "workflow_feature_unavailable",
            "message": message,
            "field": null,
            "expected_shape": expected_shape(Some(action)),
            "side_effect_occurred": false,
        },
        "next_actions": [],
    }))
}

fn workflow_details(definition: &ResolvedWorkflowDefinition, include_source: bool) -> Value {
    json!({
        "name": definition.name.as_str(),
        "display_name": definition.display_name,
        "description": definition.description,
        "phases": definition.phases,
        "input_schema": definition.input_schema,
        "output_schema": definition.output_schema,
        "source_origin": definition.source_origin.as_str(),
        "source_locator": definition.source_locator,
        "source_sha256": definition.source_sha256,
        "revision": definition.revision.as_str(),
        "definition_format_version": definition.definition_format_version,
        "script": include_source.then_some(definition.lua_source.as_str()),
    })
}

fn inline_action_arguments(
    definition: &ResolvedWorkflowDefinition,
    action: WorkflowAction,
) -> Value {
    json!({
        "action": action.as_str(),
        "name": definition.name.as_str(),
        "description": definition.description,
        "phases": definition.phases,
        "script": definition.lua_source,
        "input_schema": definition.input_schema,
        "output_schema": definition.output_schema,
    })
}

fn result(
    action: WorkflowAction,
    status: &'static str,
    content: impl Into<String>,
    workflow: Option<Value>,
    validation: Option<Value>,
    items: Option<Value>,
    task: Option<Value>,
    next_actions: Value,
) -> ToolResult {
    ToolResult::ok(content).with_details(json!({
        "ok": true,
        "action": action.as_str(),
        "status": status,
        "workflow": workflow,
        "validation": validation,
        "items": items,
        "task": task,
        "next_actions": next_actions,
    }))
}

fn permission_mode(ctx: &ToolContext) -> crate::PermissionMode {
    ctx.child_config
        .as_ref()
        .map_or(crate::PermissionMode::Auto, |config| {
            config
                .live_permission_mode
                .read()
                .map_or(config.permission_mode, |mode| *mode)
        })
}

fn launch_intent(
    ctx: &ToolContext,
    definition: &ResolvedWorkflowDefinition,
    args: Value,
    action: WorkflowAction,
    session_identity: String,
    validate_args: bool,
) -> WorkflowLaunchIntent {
    let request = WorkflowLaunchRequest {
        name: definition.name.as_str().to_owned(),
        description: definition.description.clone(),
        phases: definition.phases.clone(),
        script: definition.lua_source.clone(),
        args,
        launch_source: format!("model:Workflow({})", action.as_str()),
        parent_run_id: None,
        output_schema: Some(definition.output_schema.clone()),
    };
    let mut intent = WorkflowLaunchIntent::from_parts(
        request,
        WorkflowLaunchBinding {
            session_identity,
            workspace_identity: ctx.cwd.display().to_string(),
            launch_nonce: String::new(),
            actor: WorkflowActor::Model,
            permission_mode: permission_mode(ctx),
            parent_lineage: None,
            compiled_input_schema: validate_args
                .then(|| definition.compiled_input_schema.clone())
                .flatten(),
            schema_sha256: canonical_input_hash(&definition.output_schema),
        },
    );
    intent.definition_revision = definition.revision.clone();
    intent
}

fn validate_definition(
    ctx: &ToolContext,
    definition: &ResolvedWorkflowDefinition,
    action: WorkflowAction,
) -> Result<(), WorkflowError> {
    let session_identity = ctx.session_directory.as_ref().map_or_else(
        || "workflow-validation".to_owned(),
        |path| path.display().to_string(),
    );
    let intent = launch_intent(ctx, definition, json!({}), action, session_identity, false);
    WorkflowLaunchCoordinator.preflight(&intent, &ctx.workflow_runtime)
}

async fn launch_definition(
    ctx: &ToolContext,
    definition: &ResolvedWorkflowDefinition,
    args: Value,
    action: WorkflowAction,
) -> ToolResult {
    let Some(session_dir) = ctx.session_directory.as_deref() else {
        return feature_error(action, "Workflow run requires a durable session directory");
    };
    let intent = launch_intent(
        ctx,
        definition,
        args,
        action,
        session_dir.display().to_string(),
        true,
    );
    if let Err(error) = WorkflowLaunchCoordinator.preflight(&intent, &ctx.workflow_runtime) {
        let field = (error.code() == WorkflowErrorCode::InvalidInput).then_some("args");
        return workflow_error_result_with_context(action, error, field, false, None);
    }
    let outcome = match WorkflowLaunchCoordinator
        .launch(
            &intent,
            WorkflowLaunchHosts {
                runtime: &ctx.workflow_runtime,
                capability: &ctx.workflow_capability,
                background_tasks: &ctx.background_tasks,
                session_dir,
            },
            LaunchAuthorizationMode::Headless,
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            let side_effect_occurred = error.code() == WorkflowErrorCode::LaunchFailedAfterCreate;
            return workflow_error_result_with_context(
                action,
                error,
                None,
                side_effect_occurred,
                None,
            );
        }
    };
    let task_id = outcome.task_id;
    result(
        action,
        "running",
        format!("Workflow started as task {task_id}. Use TaskOutput to inspect it."),
        Some(workflow_details(definition, false)),
        None,
        None,
        Some(json!({
            "task_id": task_id,
            "kind": "workflow",
            "status": "running",
            "automatic_notification": true,
        })),
        json!([{
            "tool": "TaskOutput",
            "arguments": {"task_id": task_id},
            "reason": "Inspect the running workflow and collect its final result."
        }]),
    )
}

pub struct WorkflowTool;

impl Tool for WorkflowTool {
    fn name(&self) -> &'static str {
        "Workflow"
    }

    fn description(&self) -> &'static str {
        "Canonical first-party tool to list, show, validate, save, run, use, test, or evaluate Neo workflows. Activate create-workflow before inline authoring unless it is already active. For assistant-native workflow use, do not inspect Neo source, run Cargo, or invoke `neo workflow` through Bash/Terminal. Saved and inline runs require no slash capability, return a task ID, and should be inspected with TaskOutput. Child tool effects remain independently authorized."
    }

    fn input_schema(&self) -> Value {
        schema::<WorkflowInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let prepared = match prepare_action(&input) {
                Ok(prepared) => prepared,
                Err(error) => return Ok(input_error_result(error)),
            };
            let action = prepared.action();
            let registry: &WorkflowDefinitionRegistry = &ctx.workflow_definitions;

            let result = match prepared {
                PreparedWorkflowAction::List {
                    scope,
                    offset,
                    limit,
                } => {
                    let summaries = match registry.list(scope) {
                        Ok(summaries) => summaries,
                        Err(error) => return Ok(workflow_error_result(action, error)),
                    };
                    if offset > summaries.len() {
                        return Ok(input_error_result(WorkflowInputError::input(
                            action,
                            Some("cursor"),
                            "cursor is beyond the current Workflow list result",
                        )));
                    }
                    let end = offset.saturating_add(limit).min(summaries.len());
                    let items = match serde_json::to_value(&summaries[offset..end]) {
                        Ok(Value::Array(items)) => items,
                        Ok(_) => {
                            return Ok(feature_error(
                                action,
                                "Workflow registry returned an invalid list projection",
                            ));
                        }
                        Err(error) => {
                            return Ok(feature_error(
                                action,
                                format!("Workflow list serialization failed: {error}"),
                            ));
                        }
                    };
                    let next_cursor =
                        (end < summaries.len()).then(|| format!("{LIST_CURSOR_PREFIX}{end}"));
                    let next_actions = items.first().map_or_else(
                        || json!([]),
                        |item| {
                            let name = &item["name"];
                            json!([
                                {
                                    "tool": "Workflow",
                                    "arguments": {"action": "show", "name": name},
                                    "reason": "Inspect this saved workflow."
                                },
                                {
                                    "tool": "Workflow",
                                    "arguments": {"action": "run_saved", "name": name},
                                    "reason": "Run this saved workflow."
                                }
                            ])
                        },
                    );
                    result(
                        action,
                        "listed",
                        format!("Listed {} workflow(s).", items.len()),
                        None,
                        None,
                        Some(json!({
                            "entries": items,
                            "cursor": next_cursor,
                            "total": summaries.len(),
                        })),
                        None,
                        next_actions,
                    )
                }
                PreparedWorkflowAction::Show { name } => {
                    let definition = match registry.resolve(&name) {
                        Ok(definition) => definition,
                        Err(error) => return Ok(workflow_error_result(action, error)),
                    };
                    result(
                        action,
                        "shown",
                        format!("Showing workflow `{}`.", definition.name.as_str()),
                        Some(workflow_details(&definition, true)),
                        None,
                        None,
                        None,
                        json!([
                            {
                                "tool": "Workflow",
                                "arguments": {"action": "validate_saved", "name": definition.name.as_str()},
                                "reason": "Validate this saved workflow without side effects."
                            },
                            {
                                "tool": "Workflow",
                                "arguments": {"action": "run_saved", "name": definition.name.as_str()},
                                "reason": "Run this saved workflow."
                            }
                        ]),
                    )
                }
                PreparedWorkflowAction::ValidateInline { definition } => {
                    let definition = match resolve_dynamic_definition(
                        definition,
                        &ctx.workflow_runtime.limits(),
                    ) {
                        Ok(definition) => definition,
                        Err(error) => return Ok(workflow_error_result(action, error)),
                    };
                    if let Err(error) = validate_definition(ctx, &definition, action) {
                        return Ok(workflow_error_result(action, error));
                    }
                    let mut save_arguments =
                        inline_action_arguments(&definition, WorkflowAction::Save);
                    save_arguments["scope"] = json!("user");
                    result(
                        action,
                        "valid",
                        format!("Workflow `{}` is valid.", definition.name.as_str()),
                        Some(workflow_details(&definition, false)),
                        Some(json!({"valid": true})),
                        None,
                        None,
                        json!([
                            {
                                "tool": "Workflow",
                                "arguments": inline_action_arguments(&definition, WorkflowAction::RunInline),
                                "reason": "Run the validated inline workflow."
                            },
                            {
                                "tool": "Workflow",
                                "arguments": save_arguments,
                                "reason": "Save the validated workflow in user scope."
                            }
                        ]),
                    )
                }
                PreparedWorkflowAction::ValidateSaved { name } => {
                    let definition = match registry.resolve(&name) {
                        Ok(definition) => definition,
                        Err(error) => return Ok(workflow_error_result(action, error)),
                    };
                    if let Err(error) = validate_definition(ctx, &definition, action) {
                        return Ok(workflow_error_result(action, error));
                    }
                    result(
                        action,
                        "valid",
                        format!("Workflow `{}` is valid.", definition.name.as_str()),
                        Some(workflow_details(&definition, false)),
                        Some(json!({"valid": true})),
                        None,
                        None,
                        json!([
                            {
                                "tool": "Workflow",
                                "arguments": {"action": "run_saved", "name": definition.name.as_str()},
                                "reason": "Run the validated saved workflow."
                            },
                            {
                                "tool": "Workflow",
                                "arguments": {"action": "show", "name": definition.name.as_str()},
                                "reason": "Inspect the saved definition."
                            }
                        ]),
                    )
                }
                PreparedWorkflowAction::Save {
                    request,
                    scope,
                    replace,
                } => {
                    let candidate = match resolve_dynamic_definition(
                        DynamicWorkflowDefinitionInput {
                            name: request.name.clone(),
                            display_name: Some(request.display_name.clone()),
                            description: request.description.clone(),
                            phases: request.phases.clone(),
                            script: request.lua_source.clone(),
                            input_schema: request.input_schema.clone(),
                            output_schema: request.output_schema.clone(),
                        },
                        &ctx.workflow_runtime.limits(),
                    ) {
                        Ok(definition) => definition,
                        Err(error) => return Ok(workflow_error_result(action, error)),
                    };
                    if let Err(error) = validate_definition(ctx, &candidate, action) {
                        return Ok(workflow_error_result(action, error));
                    }
                    let definition = match registry.save(scope, &request, replace) {
                        Ok(definition) => definition,
                        Err(error) => {
                            let next_actions = (error.code()
                                == WorkflowErrorCode::DefinitionConflict)
                                .then(|| {
                                    json!([{
                                        "tool": "Workflow",
                                        "arguments": {
                                            "action": "save",
                                            "name": request.name,
                                            "description": request.description,
                                            "phases": request.phases,
                                            "script": request.lua_source,
                                            "input_schema": request.input_schema,
                                            "output_schema": request.output_schema,
                                            "scope": match scope {
                                                WorkflowSaveScope::User => "user",
                                                WorkflowSaveScope::Project => "project",
                                            },
                                            "replace": true,
                                        },
                                        "reason": "Retry the same save with explicit replacement enabled."
                                    }])
                                });
                            return Ok(workflow_save_error_result(error, next_actions));
                        }
                    };
                    result(
                        action,
                        "saved",
                        format!("Saved workflow `{}`.", definition.name.as_str()),
                        Some(workflow_details(&definition, false)),
                        Some(json!({"valid": true})),
                        None,
                        None,
                        json!([
                            {
                                "tool": "Workflow",
                                "arguments": {"action": "run_saved", "name": definition.name.as_str()},
                                "reason": "Run the saved workflow."
                            },
                            {
                                "tool": "Workflow",
                                "arguments": {"action": "show", "name": definition.name.as_str()},
                                "reason": "Inspect the durable saved definition."
                            }
                        ]),
                    )
                }
                PreparedWorkflowAction::RunInline { definition, args } => {
                    let definition = match resolve_dynamic_definition(
                        definition,
                        &ctx.workflow_runtime.limits(),
                    ) {
                        Ok(definition) => definition,
                        Err(error) => return Ok(workflow_error_result(action, error)),
                    };
                    launch_definition(ctx, &definition, args, action).await
                }
                PreparedWorkflowAction::RunSaved { name, args } => {
                    let definition = match registry.resolve(&name) {
                        Ok(definition) => definition,
                        Err(error) => return Ok(workflow_error_result(action, error)),
                    };
                    launch_definition(ctx, &definition, args, action).await
                }
            };
            Ok(result)
        })
    }
}

// Temporary compile bridge for Task 2. The old permission branch still names
// these functions until it is replaced with action-aware Workflow preparation.
pub(crate) struct PreparedWorkflowLaunch {
    definition: DynamicWorkflowDefinitionInput,
    args: Value,
}

impl PreparedWorkflowLaunch {
    pub(crate) fn launch_request(
        &self,
        permission_mode: crate::PermissionMode,
    ) -> WorkflowLaunchRequest {
        WorkflowLaunchRequest {
            name: self.definition.name.clone(),
            description: self.definition.description.clone(),
            phases: self.definition.phases.clone(),
            script: self.definition.script.clone(),
            args: self.args.clone(),
            launch_source: format!("model:Workflow(run_inline; {})", permission_mode.label()),
            parent_run_id: None,
            output_schema: Some(self.definition.output_schema.clone()),
        }
    }
}

pub(crate) fn validated_input(value: &Value) -> Result<PreparedWorkflowLaunch, String> {
    match prepare_action(value).map_err(|error| error.message)? {
        PreparedWorkflowAction::RunInline { definition, args } => {
            Ok(PreparedWorkflowLaunch { definition, args })
        }
        _ => Err("typed workflow launch preparation requires action `run_inline`".to_owned()),
    }
}

pub(crate) fn approval_presentation(value: &Value) -> Result<WorkflowApprovalPresentation, String> {
    let PreparedWorkflowLaunch { definition, args } = validated_input(value)?;
    let args = serde_json::to_string_pretty(&args).map_err(|error| error.to_string())?;
    Ok(WorkflowApprovalPresentation {
        name: definition.name,
        description: definition.description,
        phases: definition
            .phases
            .into_iter()
            .map(|phase| format!("{}: {}", phase.id, phase.description))
            .collect(),
        args,
        line_count: definition.script.split('\n').count().max(1),
        byte_count: definition.script.len(),
        source: definition.script,
        warning: "Launch approval authorizes orchestration only; child tool effects remain independently authorized."
            .to_owned(),
    })
}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod workflow_tests;
