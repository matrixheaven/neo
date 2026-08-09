//! Spawn `neo webui --no-open` under a PTY so stdout is an interactive
//! terminal and the full loopback address (with the one-time access token) is
//! printed. The address line is the only way a test may legally obtain the
//! token; the child is always killed on drop.

use std::io::Read as _;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

/// A running `neo webui` child plus its parsed access address.
pub(crate) struct NeoWebUi {
    pub(crate) port: u16,
    pub(crate) token: String,
    pub(crate) address: String,
    pub(crate) child: std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    pub(crate) captured: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// Kept alive so the PTY reader thread never sees a closed channel: a
    /// dropped receiver would end the thread, close the PTY master, and the
    /// kernel would SIGHUP the child (killing the web service).
    _lines: std::sync::mpsc::Receiver<String>,
}

impl Drop for NeoWebUi {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl NeoWebUi {
    /// Reap the child if it already exited and return its real exit status.
    pub(crate) fn wait_status(&self) -> Option<portable_pty::ExitStatus> {
        let mut child = self.child.lock().expect("child lock");
        child.wait().ok()
    }
}

/// Spawn `neo webui --no-open` with an isolated `NEO_HOME` and working
/// directory, and wait for the address line (bounded by `deadline`).
pub(crate) fn spawn_webui(project_dir: &Path, neo_home: &Path, deadline: Duration) -> NeoWebUi {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open pty");
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_neo"));
    command.arg("webui");
    command.arg("--no-open");
    command.env("NEO_HOME", neo_home);
    command.env("HOME", neo_home);
    command.env("OPENAI_API_KEY", "test-key");
    command.env("RUST_BACKTRACE", "1");
    command.cwd(project_dir);
    let mut child = pair.slave.spawn_command(command).expect("spawn neo webui");
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().expect("pty reader");
    let (line_tx, line_rx) = mpsc::channel();
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_thread = std::sync::Arc::clone(&captured);
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 1024];
        let mut pending = String::new();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    pending.push_str(&String::from_utf8_lossy(&buffer[..read]));
                    while let Some(position) = pending.find('\n') {
                        let line = pending[..position].to_string();
                        pending.drain(..=position);
                        if let Ok(mut lines) = captured_thread.lock() {
                            lines.push(line.clone());
                        }
                        if line_tx.send(line).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    });
    let address = loop {
        match line_rx.recv_timeout(deadline) {
            Ok(line) if line.starts_with("http://127.0.0.1:") => break line,
            Ok(_) => continue,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("neo webui did not print its loopback address within {deadline:?}");
            }
        }
    };
    let address = address.trim().to_owned();
    let rest = address
        .strip_prefix("http://127.0.0.1:")
        .expect("address prefix");
    let (port_text, fragment) = rest.split_once('/').expect("address port and fragment");
    let port = port_text.parse::<u16>().expect("address port");
    let token = fragment
        .strip_prefix("#access=")
        .expect("access token fragment")
        .to_owned();
    NeoWebUi {
        port,
        token,
        address,
        child: std::sync::Mutex::new(child),
        captured,
        _lines: line_rx,
    }
}
