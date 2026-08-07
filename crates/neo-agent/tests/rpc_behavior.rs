//! RPC mode behavior: state, session methods, commands, streaming,
//! and failure recovery.

#[path = "rpc_behavior/commands.rs"]
mod commands;
#[path = "cli_behavior/http_server.rs"]
mod http_server;
#[path = "rpc_behavior/recovery.rs"]
mod recovery;
#[path = "rpc_behavior/sessions.rs"]
mod sessions;
#[path = "rpc_behavior/state.rs"]
mod state;
#[path = "rpc_behavior/streaming.rs"]
mod streaming;
