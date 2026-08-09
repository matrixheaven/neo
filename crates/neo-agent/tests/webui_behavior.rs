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
    #[path = "session_runtime.rs"]
    pub mod session_runtime;
    #[path = "ws.rs"]
    pub mod ws;
}
