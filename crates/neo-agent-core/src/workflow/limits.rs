#[derive(Debug, Clone)]
pub struct WorkflowLimits {
    pub lua_source_bytes: u64,
    pub lua_vm_memory_bytes: u64,
    pub pause_hook_interval: u64,
    pub max_uninterrupted_instructions: u64,
    pub journal_record_bytes: u64,
    pub journal_total_bytes: u64,
    pub swarm_concurrency: usize,
    pub token_cap: Option<u64>,
}

impl Default for WorkflowLimits {
    fn default() -> Self {
        Self {
            lua_source_bytes: 1024 * 1024,          // 1 MiB
            lua_vm_memory_bytes: 256 * 1024 * 1024, // 256 MiB
            pause_hook_interval: 10_000,
            max_uninterrupted_instructions: 100_000_000,
            journal_record_bytes: 16 * 1024 * 1024, // 16 MiB
            journal_total_bytes: 4 * 1024 * 1024 * 1024, // 4 GiB
            swarm_concurrency: 4,
            token_cap: None,
        }
    }
}

const TERMINAL_TAIL_RESERVE: u64 = 64 * 1024; // 64 KiB

impl WorkflowLimits {
    #[must_use]
    pub fn invocation_reservation_bytes(&self) -> u64 {
        // Reserve: one start record (max record size is overkill; use a
        // reasonable upper bound for start) + one max-size finish + terminal tail.
        // Start records are small; use 1 MiB as generous upper bound.
        let start_reserve = 1024 * 1024;
        start_reserve + self.journal_record_bytes + TERMINAL_TAIL_RESERVE
    }
}
