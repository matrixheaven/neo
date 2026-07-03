use std::path::{Path, PathBuf};

pub const MAIN_AGENT_ID: &str = "main";
pub const SESSION_STATE_FILE: &str = "state.json";
pub const AGENTS_DIR: &str = "agents";
pub const WIRE_FILE: &str = "wire.jsonl";
pub const TASKS_DIR: &str = "tasks";
pub const PLANS_DIR: &str = "plans";

#[must_use]
pub fn session_state_path(session_dir: &Path) -> PathBuf {
    session_dir.join(SESSION_STATE_FILE)
}

#[must_use]
pub fn agents_dir(session_dir: &Path) -> PathBuf {
    session_dir.join(AGENTS_DIR)
}

#[must_use]
pub fn agent_record_dir(session_dir: &Path, agent_id: &str) -> PathBuf {
    agents_dir(session_dir).join(agent_id)
}

#[must_use]
pub fn agent_wire_path(session_dir: &Path, agent_id: &str) -> PathBuf {
    agent_record_dir(session_dir, agent_id).join(WIRE_FILE)
}

#[must_use]
pub fn main_agent_wire_path(session_dir: &Path) -> PathBuf {
    agent_wire_path(session_dir, MAIN_AGENT_ID)
}

#[must_use]
pub fn agent_tasks_dir(session_dir: &Path, agent_id: &str) -> PathBuf {
    agent_record_dir(session_dir, agent_id).join(TASKS_DIR)
}

#[must_use]
pub fn agent_plans_dir(session_dir: &Path, agent_id: &str) -> PathBuf {
    agent_record_dir(session_dir, agent_id).join(PLANS_DIR)
}

#[must_use]
pub fn main_agent_plans_dir(session_dir: &Path) -> PathBuf {
    agent_plans_dir(session_dir, MAIN_AGENT_ID)
}

#[must_use]
pub fn relative_agent_record_dir(agent_id: &str) -> PathBuf {
    PathBuf::from(AGENTS_DIR).join(agent_id)
}
