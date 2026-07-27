use thiserror::Error;

/// Stable workflow error categories (design §43). Control flow uses these codes,
/// never string parsing of human messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowErrorCode {
    InvalidDefinition,
    DefinitionConflict,
    DefinitionSavePartial,
    DefinitionNotFound,
    UntrustedProjectDefinition,
    InvalidManifest,
    LuaCompileFailed,
    InvalidSchema,
    InputSchemaInvalid,
    LaunchAuthorizationMissing,
    LaunchAuthorizationMismatch,
    StorageAdmissionDenied,
    JournalCorrupt,
    JournalTornTailRecovered,
    RecoveryConflict,
    InterruptedHostExit,
    ToolNotWorkflowEligible,
    PermissionDenied,
    InstructionReplanRequired,
    SchemaInvalid,
    SchemaRepairToolForbidden,
    AwaitingUser,
    InvalidUserAnswer,
    StaleUserRequest,
    LineageMismatch,
    ArtifactMissing,
    ArtifactCorrupt,
    ResourceLimited,
    WorkerPanicked,
    /// Generic invalid input / identity / name grammar failure.
    InvalidInput,
    InvalidOperation,
    NotFound,
    Cancelled,
    Paused,
    Failed,
    Journal,
    Host,
    Lua,
}

impl WorkflowErrorCode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidDefinition => "invalid_definition",
            Self::DefinitionConflict => "definition_conflict",
            Self::DefinitionSavePartial => "definition_save_partial",
            Self::DefinitionNotFound => "definition_not_found",
            Self::UntrustedProjectDefinition => "untrusted_project_definition",
            Self::InvalidManifest => "invalid_manifest",
            Self::LuaCompileFailed => "lua_compile_failed",
            Self::InvalidSchema => "invalid_schema",
            Self::InputSchemaInvalid => "input_schema_invalid",
            Self::LaunchAuthorizationMissing => "launch_authorization_missing",
            Self::LaunchAuthorizationMismatch => "launch_authorization_mismatch",
            Self::StorageAdmissionDenied => "storage_admission_denied",
            Self::JournalCorrupt => "journal_corrupt",
            Self::JournalTornTailRecovered => "journal_torn_tail_recovered",
            Self::RecoveryConflict => "recovery_conflict",
            Self::InterruptedHostExit => "interrupted_host_exit",
            Self::ToolNotWorkflowEligible => "tool_not_workflow_eligible",
            Self::PermissionDenied => "permission_denied",
            Self::InstructionReplanRequired => "instruction_replan_required",
            Self::SchemaInvalid => "schema_invalid",
            Self::SchemaRepairToolForbidden => "schema_repair_tool_forbidden",
            Self::AwaitingUser => "awaiting_user",
            Self::InvalidUserAnswer => "invalid_user_answer",
            Self::StaleUserRequest => "stale_user_request",
            Self::LineageMismatch => "lineage_mismatch",
            Self::ArtifactMissing => "artifact_missing",
            Self::ArtifactCorrupt => "artifact_corrupt",
            Self::ResourceLimited => "resource_limited",
            Self::WorkerPanicked => "worker_panicked",
            Self::InvalidInput => "invalid_input",
            Self::InvalidOperation => "invalid_workflow_operation",
            Self::NotFound => "not_found",
            Self::Cancelled => "cancelled",
            Self::Paused => "paused",
            Self::Failed => "failed",
            Self::Journal => "journal",
            Self::Host => "host",
            Self::Lua => "lua",
        }
    }
}

impl std::fmt::Display for WorkflowErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Error)]
pub enum WorkflowError {
    #[error("lua error: {0}")]
    Lua(String),
    #[error("workflow failed: {0}")]
    Failed(String),
    #[error("host API error: {0}")]
    Host(String),
    #[error("journal error: {0}")]
    Journal(String),
    #[error("journal record size {observed} exceeds limit {limit}")]
    JournalRecordLimitExceeded { observed: u64, limit: u64 },
    #[error("journal total size limit exceeded")]
    JournalTotalLimitExceeded,
    #[error("invalid workflow input: {0}")]
    InvalidInput(String),
    #[error("invalid_workflow_operation: {0}")]
    InvalidOperation(String),
    #[error("resource limited: {0}")]
    ResourceLimited(String),
    #[error("workflow paused: {0}")]
    Paused(String),
    #[error("workflow cancelled: {0}")]
    Cancelled(String),
    #[error("run not found: {0}")]
    NotFound(String),
    /// Typed stable-code error; preferred for new V2 control paths.
    #[error("{code}: {message}")]
    Coded {
        code: WorkflowErrorCode,
        message: String,
    },
}

impl WorkflowError {
    #[must_use]
    pub fn coded(code: WorkflowErrorCode, message: impl Into<String>) -> Self {
        Self::Coded {
            code,
            message: message.into(),
        }
    }

    /// Stable error code for routing; human messages are not parsed.
    #[must_use]
    pub fn code(&self) -> WorkflowErrorCode {
        match self {
            Self::Lua(_) => WorkflowErrorCode::Lua,
            Self::Failed(_) => WorkflowErrorCode::Failed,
            Self::Host(_) => WorkflowErrorCode::Host,
            Self::Journal(_)
            | Self::JournalRecordLimitExceeded { .. }
            | Self::JournalTotalLimitExceeded => WorkflowErrorCode::Journal,
            Self::InvalidInput(_) => WorkflowErrorCode::InvalidInput,
            Self::InvalidOperation(_) => WorkflowErrorCode::InvalidOperation,
            Self::ResourceLimited(_) => WorkflowErrorCode::ResourceLimited,
            Self::Paused(_) => WorkflowErrorCode::Paused,
            Self::Cancelled(_) => WorkflowErrorCode::Cancelled,
            Self::NotFound(_) => WorkflowErrorCode::NotFound,
            Self::Coded { code, .. } => *code,
        }
    }
}
