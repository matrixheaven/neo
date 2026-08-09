//! Web UI host product boundary: `neo webui --no-open` driven through a PTY
//! (the only legal way for a subprocess test to read the loopback address and
//! its one-time token), then HTTP + WebSocket against the real service with a
//! programmable mock `openai_response` provider.

mod webui_behavior {
    #[path = "http.rs"]
    pub mod http;
    #[path = "provider.rs"]
    pub mod provider;
    #[path = "pty.rs"]
    pub mod pty;
    #[path = "session_controls.rs"]
    pub mod session_controls;
    #[path = "session_env.rs"]
    pub mod session_env;
    #[path = "session_reconnect.rs"]
    pub mod session_reconnect;
    #[path = "session_turns.rs"]
    pub mod session_turns;
    #[path = "workspace_changes.rs"]
    pub mod workspace_changes;
    #[path = "ws.rs"]
    pub mod ws;
}
