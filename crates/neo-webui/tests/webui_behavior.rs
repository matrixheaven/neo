//! Crate behavior tests for `neo-webui`: auth/guard boundaries and the
//! bounded relay. Top-level entry declares only the same-name behavior
//! submodules (test bodies live in `webui_behavior/<behavior>.rs`), and the
//! shared fixture is pulled in via an explicit `#[path]`. The wrapper module
//! keeps the canonical full test names (`webui_behavior::auth::…`,
//! `webui_behavior::relay::…`) stable.

mod webui_behavior {
    #[path = "assets.rs"]
    mod assets;
    #[path = "auth.rs"]
    mod auth;
    #[path = "relay.rs"]
    mod relay;

    #[path = "http_server.rs"]
    mod http_server;
}
