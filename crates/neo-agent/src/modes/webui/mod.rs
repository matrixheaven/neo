//! Local web interface mode: `neo webui [--no-open]`.
//!
//! Binds `127.0.0.1:0` and prints the full
//! `http://127.0.0.1:<port>/#access=<token>` address when and only when
//! stdout is an interactive terminal, and opens the browser by default
//! (`--no-open` only disables the open). The web service identity
//! (`stream_id`) is minted here where the relay is built; the one-time
//! access token is minted inside `neo-webui`'s `AuthState` at server start.
//! When stdout is redirected, neither stdout, stderr nor logs ever carry the
//! address, token, cookie or auth bodies.

pub mod host;
mod session;

use std::io::IsTerminal;
use std::sync::Arc;

use neo_webui::relay::Relay;

use crate::config::AppConfig;

use self::host::WebSessionHost;

/// Serve the local web interface until the process exits.
pub async fn execute(config: &AppConfig, no_open: bool) -> anyhow::Result<String> {
    let relay = Arc::new(Relay::new(format!(
        "webui_{}",
        uuid::Uuid::new_v4().simple()
    )));
    let host = Arc::new(WebSessionHost::new(config.clone(), Arc::clone(&relay)));
    let running = neo_webui::server::start(host, relay).await?;
    let url = running.access_url();
    let interactive = std::io::stdout().is_terminal();
    if interactive {
        println!("{url}");
    }
    if !no_open && interactive && webbrowser::open(&url).is_err() {
        // Generic hint only: never echo the address, token or the library's
        // raw error. With stdout redirected the address was never printed,
        // so no address hint is printed at all.
        eprintln!("failed to open the browser; open the printed address manually");
    }
    running.run().await?;
    Ok(String::new())
}
