/// Host-owned machine-safety limits for workflow occupancy and storage.
///
/// Scripts, model tool inputs, and project definitions cannot set or raise these
/// values. Limits describe actual physical capacity — not predictive token,
/// agent-count, or wall-clock governance.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowLimits {
    /// Maximum Lua source bytes for a single workflow definition/run.
    pub lua_source_bytes: u64,
    /// Maximum manifest (`.workflow.toml`) bytes.
    pub manifest_bytes: u64,
    /// Lua VM memory ceiling per active VM; must fit platform `usize`.
    pub lua_vm_memory_bytes: u64,
    /// Lua instructions between pause/stop/resource checks (`1..=u32::MAX`).
    pub pause_hook_interval: u64,
    /// Maximum Lua instructions without a durable child invocation.
    pub max_uninterrupted_instructions: u64,
    /// Maximum serialized journal record size.
    pub journal_record_bytes: u64,
    /// Maximum journal size per workflow run.
    pub journal_total_bytes: u64,
    /// Maximum single artifact payload size.
    pub artifact_record_bytes: u64,
    /// Maximum total artifact bytes per workflow run.
    pub artifact_total_bytes: u64,
    /// Global workflow storage ceiling (journals + artifacts + metadata).
    pub global_storage_bytes: u64,
    /// Global pending (not yet durable-synced) record byte ceiling.
    pub pending_record_bytes: u64,
    /// Maximum complete TaskOutput tool-result bytes.
    pub task_output_page_bytes: u64,
    /// Simultaneously active Lua VMs.
    pub max_active_vms: usize,
    /// Simultaneously active workflow worker tasks.
    pub max_active_workers: usize,
    /// Simultaneously active host executors (child tool / effect slots).
    pub max_active_executors: usize,
    /// Default concurrency for workflow-created swarms (not a total child cap).
    pub swarm_concurrency: usize,
}

impl Default for WorkflowLimits {
    fn default() -> Self {
        Self {
            lua_source_bytes: 1024 * 1024,          // 1 MiB
            manifest_bytes: 256 * 1024,             // 256 KiB
            lua_vm_memory_bytes: 256 * 1024 * 1024, // 256 MiB
            pause_hook_interval: 10_000,
            max_uninterrupted_instructions: 100_000_000,
            journal_record_bytes: 16 * 1024 * 1024, // 16 MiB
            journal_total_bytes: 4 * 1024 * 1024 * 1024, // 4 GiB
            artifact_record_bytes: 16 * 1024 * 1024, // 16 MiB
            artifact_total_bytes: 4 * 1024 * 1024 * 1024, // 4 GiB
            global_storage_bytes: 32 * 1024 * 1024 * 1024, // 32 GiB
            pending_record_bytes: 256 * 1024 * 1024, // 256 MiB
            task_output_page_bytes: 64 * 1024,      // 64 KiB
            max_active_vms: 8,
            max_active_workers: 8,
            max_active_executors: 32,
            swarm_concurrency: 4,
        }
    }
}

const TERMINAL_TAIL_RESERVE: u64 = 64 * 1024; // 64 KiB

impl WorkflowLimits {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.lua_source_bytes == 0 {
            return Err("runtime.workflow.lua_source_bytes must be greater than 0");
        }
        if self.manifest_bytes == 0 {
            return Err("runtime.workflow.manifest_bytes must be greater than 0");
        }
        if self.lua_vm_memory_bytes == 0 {
            return Err("runtime.workflow.lua_vm_memory_bytes must be greater than 0");
        }
        if usize::try_from(self.lua_vm_memory_bytes).is_err() {
            return Err("runtime.workflow.lua_vm_memory_bytes does not fit this platform");
        }
        if self.pause_hook_interval == 0 || self.pause_hook_interval > u64::from(u32::MAX) {
            return Err("runtime.workflow.pause_hook_interval must be between 1 and u32::MAX");
        }
        if self.max_uninterrupted_instructions == 0 {
            return Err("runtime.workflow.max_uninterrupted_instructions must be greater than 0");
        }
        if self.journal_record_bytes == 0 {
            return Err("runtime.workflow.journal_record_bytes must be greater than 0");
        }
        if self.journal_total_bytes == 0 {
            return Err("runtime.workflow.journal_total_bytes must be greater than 0");
        }
        if self.artifact_record_bytes == 0 {
            return Err("runtime.workflow.artifact_record_bytes must be greater than 0");
        }
        if self.artifact_total_bytes == 0 {
            return Err("runtime.workflow.artifact_total_bytes must be greater than 0");
        }
        if self.global_storage_bytes == 0 {
            return Err("runtime.workflow.global_storage_bytes must be greater than 0");
        }
        if self.pending_record_bytes == 0 {
            return Err("runtime.workflow.pending_record_bytes must be greater than 0");
        }
        if self.task_output_page_bytes == 0 {
            return Err("runtime.workflow.task_output_page_bytes must be greater than 0");
        }
        if self.max_active_vms == 0 {
            return Err("runtime.workflow.max_active_vms must be greater than 0");
        }
        if self.max_active_workers == 0 {
            return Err("runtime.workflow.max_active_workers must be greater than 0");
        }
        if self.max_active_executors == 0 {
            return Err("runtime.workflow.max_active_executors must be greater than 0");
        }
        if self.swarm_concurrency == 0 {
            return Err("runtime.workflow.swarm_concurrency must be greater than 0");
        }
        Ok(())
    }

    /// Bytes reserved up-front for a durable run create (metadata + journal headroom).
    #[must_use]
    pub fn run_storage_reservation_bytes(&self) -> u64 {
        self.journal_record_bytes
            .saturating_add(TERMINAL_TAIL_RESERVE)
            .max(64 * 1024)
    }

    #[must_use]
    pub fn invocation_reservation_bytes(&self, start_record_bytes: u64) -> Option<u64> {
        start_record_bytes
            .checked_add(self.journal_record_bytes)
            .and_then(|bytes| bytes.checked_add(TERMINAL_TAIL_RESERVE))
    }
}
