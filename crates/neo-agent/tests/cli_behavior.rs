//! CLI behavior: root/run commands, session picker and resume,
//! config/MCP wiring, mock-provider run output, and the fullscreen
//! boundary for static modes.

#[path = "cli_behavior/commands.rs"]
mod commands;
#[path = "cli_behavior/config.rs"]
mod config;
#[path = "cli_behavior/fullscreen_output.rs"]
mod fullscreen_output;
#[path = "cli_behavior/http_server.rs"]
mod http_server;
#[path = "cli_behavior/mock_provider.rs"]
mod mock_provider;
#[path = "cli_behavior/sessions.rs"]
mod sessions;
