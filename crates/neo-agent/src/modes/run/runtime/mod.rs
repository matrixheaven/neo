mod agent;
mod model;

pub(crate) use agent::{agent_config_for_app, build_mcp_client, tool_registry_for_config};
pub(crate) use model::{
    model_config_matches_default, model_registry_for_config, resolve_model,
    resolve_model_client, select_config_model,
};
