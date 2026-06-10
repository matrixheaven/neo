use std::{
    collections::BTreeSet,
    env, fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Component, Path, PathBuf},
    process::{Child, Command},
    thread,
    time::Duration,
};

use anyhow::{Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use clap::{Parser, Subcommand};
use ed25519_dalek::{Signer as _, SigningKey};
use regex::Regex;
use sha2::{Digest as _, Sha256};
use tar::{Builder, Header};

#[derive(Debug, Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<XtaskCommand>,
}

#[derive(Debug, Subcommand)]
enum XtaskCommand {
    Check(CheckOptions),
    /// Run the docs/examples parity gate without fmt, clippy, or tests.
    Parity,
    /// Run the release smoke gate against a self-hosted neo-cloud.
    ReleaseSmoke,
    /// Validate generated catalog artifacts.
    #[command(subcommand)]
    Catalog(CatalogCommand),
}

#[derive(Debug, Subcommand)]
enum CatalogCommand {
    /// Validate generated model catalog schema artifacts.
    Check(CatalogCheckOptions),
}

#[derive(Debug, Clone, Default, clap::Args)]
struct CheckOptions {
    /// Validate local links in docs and examples Markdown files.
    #[arg(long)]
    docs: bool,
    /// Run full workspace checks instead of the stable xtask gate.
    #[arg(long)]
    workspace: bool,
    /// Run only the xtask package checks.
    #[arg(long)]
    quick: bool,
}

#[derive(Debug, Clone, Default, clap::Args)]
struct CatalogCheckOptions {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandStep {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    current_dir: Option<PathBuf>,
}

impl CommandStep {
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_owned(),
            args: args.iter().map(ToString::to_string).collect(),
            env: Vec::new(),
            current_dir: None,
        }
    }

    fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    fn with_envs(mut self, env: &[(String, String)]) -> Self {
        self.env.extend(env.iter().cloned());
        self
    }

    fn display(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli
        .command
        .unwrap_or_else(|| XtaskCommand::Check(CheckOptions::default()))
    {
        XtaskCommand::Check(options) => check(&options),
        XtaskCommand::Parity => run_parity_gate(Path::new(".")),
        XtaskCommand::ReleaseSmoke => run_release_smoke(Path::new(".")),
        XtaskCommand::Catalog(CatalogCommand::Check(options)) => catalog_check(&options),
    }
}

fn check(options: &CheckOptions) -> Result<()> {
    for step in check_steps(options) {
        run(&step)?;
    }
    if options.docs {
        run_parity_gate(Path::new("."))?;
    }
    Ok(())
}

fn run_parity_gate(root: &Path) -> Result<()> {
    let errors = validate_parity_gate(root)?;
    if !errors.is_empty() {
        bail!(
            "production/docs/examples parity validation failed:\n{}",
            errors.join("\n")
        );
    }
    run_rust_examples_compile_gate(root)?;
    println!("production/docs/examples parity validation passed");
    Ok(())
}

fn catalog_check(_options: &CatalogCheckOptions) -> Result<()> {
    let report = validate_catalog_schemas(Path::new("."), CatalogRequirement::Required)?;
    if !report.errors.is_empty() {
        bail!(
            "catalog schema validation failed:\n{}",
            report.errors.join("\n")
        );
    }
    println!(
        "catalog schema validation passed ({} artifact{})",
        report.checked,
        if report.checked == 1 { "" } else { "s" }
    );
    Ok(())
}

fn run_release_smoke(root: &Path) -> Result<()> {
    run_parity_gate(root)?;

    let catalog_report = validate_catalog_schemas(root, CatalogRequirement::Optional)?;
    if !catalog_report.errors.is_empty() {
        bail!(
            "release smoke catalog validation failed:\n{}",
            catalog_report.errors.join("\n")
        );
    }

    let cloud_override = env::var("NEO_RELEASE_SMOKE_CLOUD_CMD").ok();
    let dependency_errors = if cloud_override
        .as_deref()
        .is_some_and(|command| !command.trim().is_empty())
    {
        release_smoke_dependency_errors_with_override(root, cloud_override.as_deref())?
    } else {
        release_smoke_dependency_errors(root)?
    };
    if !dependency_errors.is_empty() {
        bail!(
            "release smoke dependencies are not ready:\n{}",
            dependency_errors.join("\n")
        );
    }

    let fixture = ReleaseSmokeFixture::new()?;
    let port = random_local_port()?;
    let cloud_step = release_smoke_cloud_step(port, cloud_override.as_deref())
        .with_env("HOME", fixture.home_dir.display().to_string());
    let mut cloud = spawn_release_smoke_cloud(&cloud_step)?;
    thread::sleep(Duration::from_millis(500));
    if let Some(status) = cloud.try_wait()? {
        bail!(
            "self-hosted neo-cloud exited before CLI smoke flows could run with status {status}; command was `{}`",
            cloud_step.display()
        );
    }

    let result = run_release_smoke_cli_flows(port, &fixture);
    stop_release_smoke_cloud(&mut cloud);
    result?;

    println!("release smoke passed on http://127.0.0.1:{port}");
    Ok(())
}

fn validate_parity_gate(root: &Path) -> Result<Vec<String>> {
    let mut errors = validate_docs_links(root)?;
    errors.extend(validate_generated_cloud_api_schema_links(root)?);
    errors.extend(validate_docs_parity(root)?);
    errors.extend(validate_examples(root)?);
    errors.extend(validate_catalog_schemas(root, CatalogRequirement::Required)?.errors);
    errors.sort();
    Ok(errors)
}

fn check_steps(options: &CheckOptions) -> Vec<CommandStep> {
    if !options.workspace || options.quick {
        vec![
            CommandStep::new("cargo", &["fmt", "-p", "xtask", "--check"]),
            CommandStep::new(
                "cargo",
                &[
                    "clippy",
                    "-p",
                    "xtask",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
            ),
            CommandStep::new("cargo", &["test", "-p", "xtask"]),
            CommandStep::new("cargo", &["run", "-p", "xtask", "--", "catalog", "check"]),
        ]
    } else {
        vec![
            CommandStep::new("cargo", &["fmt", "--all", "--check"]),
            CommandStep::new(
                "cargo",
                &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--",
                    "-D",
                    "warnings",
                ],
            ),
            CommandStep::new("cargo", &["test", "--workspace", "--all-features"]),
            CommandStep::new("cargo", &["run", "-p", "xtask", "--", "catalog", "check"]),
        ]
    }
}

fn run(step: &CommandStep) -> Result<()> {
    println!("running: {}", step.display());
    let mut command = Command::new(&step.program);
    command.args(&step.args).envs(step.env.iter().cloned());
    if let Some(current_dir) = &step.current_dir {
        command.current_dir(current_dir);
    }
    let status = command.status()?;
    if !status.success() {
        bail!("{} failed", step.display());
    }
    Ok(())
}

fn run_capture(step: &CommandStep) -> Result<String> {
    println!("running: {}", step.display());
    let mut command = Command::new(&step.program);
    command.args(&step.args).envs(step.env.iter().cloned());
    if let Some(current_dir) = &step.current_dir {
        command.current_dir(current_dir);
    }
    let output = command.output()?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        bail!("{} failed", step.display());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run_release_smoke_cli_flows(port: u16, fixture: &ReleaseSmokeFixture) -> Result<()> {
    let marketplace = spawn_release_smoke_marketplace(fixture)?;
    let env = fixture.command_env(&marketplace.url);
    let api_base = format!("http://127.0.0.1:{port}");

    run(&release_smoke_neo_step(&["--help"], &env))?;
    run(&release_smoke_neo_step(
        &["models", "list", "--pricing"],
        &env,
    ))?;
    run(&release_smoke_neo_step(
        &["cloud", "status", "--api-base", &api_base],
        &env,
    ))?;
    run(&release_smoke_neo_step(
        &["login", "cloud", "--server", &api_base],
        &env,
    ))?;
    for step in release_smoke_profile_steps() {
        run(&release_smoke_prepare_step(step, &env))?;
    }
    run(&release_smoke_neo_step(
        &["sessions", "sync", "status"],
        &env,
    ))?;
    let share_output = run_capture(&release_smoke_neo_step(
        &["sessions", "share", "release-smoke", "--public"],
        &env,
    ))?;
    let share_id = output_value(&share_output, "share_id")?;
    let cloud_id = output_value(&share_output, "cloud_id")?;
    run(&release_smoke_neo_step(
        &["sessions", "import", &share_id],
        &env,
    ))?;
    run(&release_smoke_neo_step(&["resume", &cloud_id], &env))?;

    run(&release_smoke_neo_step(
        &[
            "trust",
            "publishers",
            "add",
            "neo-test",
            "--name",
            "Neo Test",
            "--root",
            "local-root",
            "--key-id",
            "ed25519:2026-a",
            "--public-key",
            &marketplace.public_key,
            "--account-id",
            "acct_neo_test",
        ],
        &env,
    ))?;
    for step in release_smoke_marketplace_steps() {
        run(&release_smoke_prepare_step(step, &env))?;
    }
    for step in release_smoke_mcp_steps() {
        run(&release_smoke_prepare_step(step, &env))?;
    }
    run(&CommandStep::new(
        "cargo",
        &["run", "-p", "xtask", "--", "catalog", "check"],
    ))?;
    Ok(())
}

fn release_smoke_neo_step(args: &[&str], env: &[(String, String)]) -> CommandStep {
    neo_agent_step(args).with_envs(env)
}

fn release_smoke_prepare_step(step: CommandStep, env: &[(String, String)]) -> CommandStep {
    step.with_envs(env)
}

#[cfg(test)]
fn release_smoke_cli_steps(port: u16) -> Vec<CommandStep> {
    let api_base = format!("http://127.0.0.1:{port}");
    let mut steps = vec![
        neo_agent_step(&["--help"]),
        neo_agent_step(&["models", "list", "--pricing"]),
        neo_agent_step(&["cloud", "status", "--api-base", &api_base]),
        neo_agent_step(&["login", "cloud", "--server", &api_base]),
    ];

    steps.extend(release_smoke_profile_steps());
    steps.extend(release_smoke_session_steps());
    steps.extend(release_smoke_marketplace_steps());
    steps.extend(release_smoke_mcp_steps());
    steps.push(CommandStep::new(
        "cargo",
        &["run", "-p", "xtask", "--", "catalog", "check"],
    ));

    steps
}

fn neo_agent_step(args: &[&str]) -> CommandStep {
    let mut cargo_args = vec!["run", "-p", "neo-agent", "--"];
    cargo_args.extend_from_slice(args);
    CommandStep::new("cargo", &cargo_args)
}

fn release_smoke_profile_steps() -> Vec<CommandStep> {
    vec![
        neo_agent_step(&["auth", "status"]),
        neo_agent_step(&["config", "sync", "status"]),
        neo_agent_step(&["config", "sync", "push"]),
        neo_agent_step(&["config", "sync", "pull"]),
    ]
}

#[cfg(test)]
fn release_smoke_session_steps() -> Vec<CommandStep> {
    vec![
        neo_agent_step(&["sessions", "sync", "status"]),
        neo_agent_step(&["sessions", "share", "release-smoke", "--public"]),
        neo_agent_step(&["sessions", "import", "sh_release_smoke"]),
        neo_agent_step(&["resume", "cs_release_smoke"]),
    ]
}

fn release_smoke_marketplace_steps() -> Vec<CommandStep> {
    vec![
        neo_agent_step(&["extensions", "search", "echo"]),
        neo_agent_step(&[
            "extensions",
            "install",
            "echo@0.1.0",
            "--from",
            "marketplace",
        ]),
    ]
}

fn release_smoke_mcp_steps() -> Vec<CommandStep> {
    vec![
        neo_agent_step(&["mcp", "servers", "health", "release-smoke"]),
        neo_agent_step(&["mcp", "servers", "start", "release-smoke"]),
        neo_agent_step(&["mcp", "servers", "stop", "release-smoke"]),
    ]
}

struct ReleaseSmokeFixture {
    _temp_dir: tempfile::TempDir,
    home_dir: PathBuf,
    config_path: PathBuf,
    sessions_dir: PathBuf,
    package_dir: PathBuf,
}

impl ReleaseSmokeFixture {
    fn new() -> Result<Self> {
        let temp_dir = tempfile::Builder::new()
            .prefix("neo-release-smoke-")
            .tempdir()?;
        let project_dir = temp_dir.path().join("project");
        let home_dir = temp_dir.path().join("home");
        let neo_dir = project_dir.join(".neo");
        let sessions_dir = neo_dir.join("sessions");
        let package_dir = temp_dir.path().join("marketplace");
        let mcp_script = temp_dir.path().join("release-smoke-mcp.py");
        let mcp_pid_file = temp_dir.path().join("release-smoke-mcp.pid");

        fs::create_dir_all(&sessions_dir)?;
        fs::create_dir_all(&home_dir)?;
        fs::create_dir_all(&package_dir)?;
        fs::write(&mcp_script, RELEASE_SMOKE_MCP_FIXTURE)?;
        fs::write(
            sessions_dir.join("release-smoke.jsonl"),
            format!(
                "{}\n",
                serde_json::json!({
                    "MessageAppended": {
                        "message": {
                            "User": {
                                "content": [{
                                    "Text": {
                                        "text": "release smoke self-hosted session"
                                    }
                                }]
                            }
                        }
                    }
                })
            ),
        )?;
        let config_path = neo_dir.join("config.toml");
        fs::write(
            &config_path,
            format!(
                r#"
sessions_dir = ".neo/sessions"

[cloud]
auth_file = ".neo/auth.json"

[[mcp.servers]]
id = "release-smoke"
enabled = true
transport = "stdio"
command = "python3"
args = ["-u", "{}"]

[mcp.servers.env]
MCP_PID_FILE = "{}"
"#,
                toml_escape(&mcp_script),
                toml_escape(&mcp_pid_file)
            ),
        )?;

        Ok(Self {
            _temp_dir: temp_dir,
            home_dir,
            config_path,
            sessions_dir,
            package_dir,
        })
    }

    fn command_env(&self, marketplace_url: &str) -> Vec<(String, String)> {
        vec![
            ("HOME".to_owned(), self.home_dir.display().to_string()),
            (
                "NEO_CONFIG".to_owned(),
                self.config_path.display().to_string(),
            ),
            (
                "NEO_SESSIONS_DIR".to_owned(),
                self.sessions_dir.display().to_string(),
            ),
            ("NEO_MARKETPLACE_URL".to_owned(), marketplace_url.to_owned()),
        ]
    }
}

struct ReleaseSmokeMarketplace {
    url: String,
    public_key: String,
}

fn spawn_release_smoke_marketplace(
    fixture: &ReleaseSmokeFixture,
) -> Result<ReleaseSmokeMarketplace> {
    let package = write_release_smoke_extension_package(&fixture.package_dir)?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let url = format!("http://{}", listener.local_addr()?);
    let responses = vec![
        http_json_response(&serde_json::json!({
            "packages": [{
                "kind": "extension",
                "id": "echo",
                "version": "0.1.0",
                "name": "Echo",
                "description": "Release smoke echo extension",
                "publisher": "neo-test"
            }]
        }))?,
        http_json_response(&serde_json::json!({
            "package": {
                "kind": "extension",
                "id": "echo",
                "version": "0.1.0",
                "manifest_url": "/api/v1/marketplace/packages/extension/echo/0.1.0/.neo-package.toml",
                "archive_url": "/api/v1/marketplace/packages/extension/echo/0.1.0/echo-0.1.0.tar"
            }
        }))?,
        http_response("application/toml", package.manifest.as_bytes()),
        http_response("application/x-tar", &package.archive),
    ];
    thread::spawn(move || {
        for response in responses {
            let Ok((mut socket, _)) = listener.accept() else {
                return;
            };
            let _ = read_http_headers(&mut socket);
            let _ = socket.write_all(&response);
        }
    });
    Ok(ReleaseSmokeMarketplace {
        url,
        public_key: package.public_key,
    })
}

struct ReleaseSmokePackage {
    manifest: String,
    archive: Vec<u8>,
    public_key: String,
}

fn write_release_smoke_extension_package(root: &Path) -> Result<ReleaseSmokePackage> {
    fs::create_dir_all(root)?;
    let archive_path = root.join("echo-0.1.0.tar");
    write_release_smoke_archive(&archive_path)?;
    let archive = fs::read(&archive_path)?;
    let digest = hex_sha256(&archive);
    let signing_key = SigningKey::from_bytes(&[23_u8; 32]);
    let verifying_key = signing_key.verifying_key();
    let signature = signing_key.sign(&archive);
    let public_key = STANDARD.encode(verifying_key.to_bytes());
    let manifest = format!(
        r#"
kind = "extension"
id = "echo"
version = "0.1.0"
entry = "neo-extension.toml"

[publisher]
id = "neo-test"
name = "Neo Test"
account_id = "acct_neo_test"

[archive]
path = "echo-0.1.0.tar"
sha256 = "{digest}"

[signature]
algorithm = "ed25519"
root = "local-root"
public_key_id = "ed25519:2026-a"
public_key = "{public_key}"
signature = "{}"
"#,
        STANDARD.encode(signature.to_bytes()),
    );
    Ok(ReleaseSmokePackage {
        manifest,
        archive,
        public_key,
    })
}

fn write_release_smoke_archive(path: &Path) -> Result<()> {
    let file = fs::File::create(path)?;
    let mut builder = Builder::new(file);
    let content = br#"
id = "echo"
name = "Echo"
version = "0.1.0"

[runner]
command = "python3"
"#;
    let mut header = Header::new_gnu();
    header.set_path("neo-extension.toml")?;
    header.set_size(content.len().try_into()?);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append(&header, &content[..])?;
    builder.finish()?;
    Ok(())
}

fn http_json_response(value: &serde_json::Value) -> Result<Vec<u8>> {
    Ok(http_response(
        "application/json",
        serde_json::to_vec(value)?.as_slice(),
    ))
}

fn http_response(content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn read_http_headers(stream: &mut impl Read) -> Result<()> {
    let mut buffer = [0_u8; 1024];
    let mut request = Vec::new();
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    Ok(())
}

fn output_value(output: &str, key: &str) -> Result<String> {
    for line in output.lines() {
        if let Some(value) = line.strip_prefix(&format!("{key}: ")) {
            return Ok(value.trim().to_owned());
        }
        if let Some(value) = line.strip_prefix(&format!("{key}=")) {
            return Ok(value.trim().to_owned());
        }
    }
    bail!("missing {key} in command output:\n{output}")
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = std::fmt::Write::write_fmt(&mut output, format_args!("{byte:02x}"));
    }
    output
}

fn toml_escape(path: &Path) -> String {
    path.display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn release_smoke_dependency_errors(root: &Path) -> Result<Vec<String>> {
    release_smoke_dependency_errors_with_override(root, None)
}

const RELEASE_SMOKE_MCP_FIXTURE: &str = r#"
import json
import os
import sys

pid_file = os.environ.get("MCP_PID_FILE")
if pid_file:
    with open(pid_file, "w", encoding="utf-8") as handle:
        handle.write(str(os.getpid()))

for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "initialize":
        result = {
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "release-smoke", "version": "0.1.0"},
            "capabilities": {"tools": {}},
        }
    elif method == "tools/list":
        result = {
            "tools": [{
                "name": "echo",
                "description": "Release smoke echo tool",
                "inputSchema": {"type": "object", "properties": {}},
            }]
        }
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request.get("id"), "result": result}), flush=True)
"#;

fn release_smoke_dependency_errors_with_override(
    root: &Path,
    cloud_override: Option<&str>,
) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let has_cloud_override = cloud_override.is_some_and(|command| !command.trim().is_empty());

    let cloud_manifest = root.join("crates").join("neo-cloud").join("Cargo.toml");
    if !cloud_manifest.exists() && !has_cloud_override {
        errors.push(
            "missing self-hosted neo-cloud package at crates/neo-cloud/Cargo.toml; land the cloud worker output or set NEO_RELEASE_SMOKE_CLOUD_CMD to an explicit start command"
                .to_owned(),
        );
    }
    if (cloud_manifest.exists() || has_cloud_override) && !release_smoke_cli_flow_exists(root)? {
        errors.push(
            "missing neo-agent cloud CLI smoke flow; expected crates/neo-agent/src/cli.rs to expose `cloud status --api-base <URL>` before release-smoke can exercise neo-cloud"
                .to_owned(),
        );
        return Ok(errors);
    }
    if cloud_manifest.exists() || has_cloud_override {
        errors.extend(release_smoke_cli_surface_errors(root)?);
    }

    Ok(errors)
}

fn release_smoke_cli_flow_exists(root: &Path) -> Result<bool> {
    let source = read_optional_source(&root.join("crates/neo-agent/src/cli.rs"))?;
    let normalized = source.to_lowercase().replace(['-', '_'], " ");
    Ok(normalized.contains("cloud")
        && normalized.contains("status")
        && normalized.contains("api base"))
}

fn release_smoke_cli_surface_errors(root: &Path) -> Result<Vec<String>> {
    let source = read_optional_source(&root.join("crates/neo-agent/src/cli.rs"))?;
    let normalized = source.to_lowercase().replace(['-', '_'], " ");
    let mut errors = Vec::new();

    for (ok, message) in [
        (
            normalized.contains("logincommand") && normalized.contains("cloud"),
            "missing neo-agent cloud login smoke flow; expected `login cloud --server <URL>` before release-smoke can exercise self-hosted profile sync",
        ),
        (
            normalized.contains("authcommand") && normalized.contains("status"),
            "missing neo-agent auth status smoke flow; expected `auth status` before release-smoke can verify self-hosted login state",
        ),
        (
            normalized.contains("configsynccommand")
                && normalized.contains("push")
                && normalized.contains("pull")
                && normalized.contains("status"),
            "missing neo-agent config sync smoke flow; expected `config sync status|push|pull` before release-smoke can exercise profile sync",
        ),
        (
            normalized.contains("sessioncommand")
                && normalized.contains("share")
                && normalized.contains("import")
                && normalized.contains("sync"),
            "missing neo-agent session cloud smoke flow; expected `sessions sync`, `sessions share`, and `sessions import` before release-smoke can exercise session share/import/resume",
        ),
        (
            normalized.contains("modelcommand")
                && normalized.contains("list")
                && normalized.contains("pricing"),
            "missing neo-agent model pricing smoke flow; expected `models list --pricing` before release-smoke can verify generated catalog pricing display",
        ),
        (
            normalized.contains("extensioncommand")
                && normalized.contains("search")
                && normalized.contains("install")
                && normalized.contains("packagesource")
                && normalized.contains("marketplace"),
            "missing neo-agent marketplace smoke flow; expected `extensions search` and `extensions install --from marketplace` before release-smoke can exercise local marketplace fixtures",
        ),
        (
            normalized.contains("mcpcommand")
                && normalized.contains("health")
                && normalized.contains("start")
                && normalized.contains("stop"),
            "missing neo-agent MCP lifecycle smoke flow; expected crates/neo-agent/src/cli.rs to expose `mcp servers health/start/stop <server-id>` before release-smoke can exercise local MCP lifecycle",
        ),
    ] {
        if !ok {
            errors.push(message.to_owned());
        }
    }

    Ok(errors)
}

fn release_smoke_cloud_step(port: u16, cloud_override: Option<&str>) -> CommandStep {
    if let Some(command) = cloud_override
        .map(str::trim)
        .filter(|command| !command.is_empty())
    {
        return override_command_step(command, port);
    }

    CommandStep::new(
        "cargo",
        &[
            "run",
            "-p",
            "neo-cloud",
            "--",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
        ],
    )
}

fn override_command_step(command: &str, port: u16) -> CommandStep {
    let substituted = command
        .replace("{host}", "127.0.0.1")
        .replace("{port}", &port.to_string());
    let mut parts = substituted.split_whitespace();
    let program = parts.next().unwrap_or("neo-cloud").to_owned();
    let args = parts.map(ToString::to_string).collect();
    CommandStep {
        program,
        args,
        env: Vec::new(),
        current_dir: None,
    }
}

fn spawn_release_smoke_cloud(step: &CommandStep) -> Result<Child> {
    println!("starting self-hosted cloud: {}", step.display());
    Command::new(&step.program)
        .args(&step.args)
        .spawn()
        .map_err(Into::into)
}

fn stop_release_smoke_cloud(child: &mut Child) {
    if child.try_wait().is_ok_and(|status| status.is_none()) {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn random_local_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

fn validate_docs_links(root: &Path) -> Result<Vec<String>> {
    let link_pattern = Regex::new(r"\[[^\]]+\]\(([^)]+)\)")?;
    let markdown_files = markdown_files(root)?;
    let mut errors = Vec::new();

    for file in markdown_files {
        let source = fs::read_to_string(&file)?;
        let relative_file = relative_path(root, &file);
        let Some(parent) = file.parent() else {
            continue;
        };

        for captures in link_pattern.captures_iter(&source) {
            let Some(raw_target) = captures.get(1).map(|target| target.as_str().trim()) else {
                continue;
            };
            if let Some(target) = local_link_target(raw_target) {
                let target_path = parent.join(target);
                if !target_path.exists() {
                    errors.push(format!(
                        "{} links to missing local file {}",
                        relative_file.display(),
                        relative_path(root, &target_path).display()
                    ));
                }
            }
        }
    }

    errors.sort();
    Ok(errors)
}

fn validate_docs_parity(root: &Path) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let code_truth = ParityCodeTruth::load(root)?;
    for file in parity_scan_files(root)? {
        let source = fs::read_to_string(&file)?;
        let relative_file = relative_path(root, &file);
        let explicit_fixture_path = is_explicit_fixture_path(&relative_file);
        let mut allow_next_line = false;

        for (index, line) in source.lines().enumerate() {
            let line_number = index + 1;
            if explicit_fixture_path && parity_allowlist_reason(line).is_some() {
                allow_next_line = true;
                continue;
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                allow_next_line = false;
                continue;
            }

            if allow_next_line {
                allow_next_line = false;
                continue;
            }

            if explicit_fixture_path && parity_line_is_fixture_safe(trimmed) {
                continue;
            }

            if let Some(reason) =
                parity_line_violation(&relative_file, trimmed, explicit_fixture_path, &code_truth)
            {
                errors.push(format!(
                    "{}:{line_number} contains {reason}: {trimmed}",
                    relative_file.display()
                ));
            }
        }
    }
    errors.sort();
    Ok(errors)
}

fn parity_scan_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = production_scan_files(root)?;
    out.extend(markdown_files(root)?);
    let examples = root.join("examples");
    if examples.is_dir() {
        collect_files_with_extensions(&examples, &["toml", "json", "yaml", "yml"], &mut out)?;
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn production_scan_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let crates = root.join("crates");
    if !crates.is_dir() {
        return Ok(out);
    }

    for entry in fs::read_dir(crates)? {
        let crate_dir = entry?.path();
        if crate_dir.is_dir() {
            let src = crate_dir.join("src");
            if src.is_dir() {
                collect_files_with_extensions(&src, &["rs", "toml", "json"], &mut out)?;
            }
        }
    }

    out.sort();
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ImplementedSurface {
    McpToolAdapterBoundary,
    StdioMcpProcessAdapter,
    HttpMcpJsonSubscribeEventReader,
    McpSubscribeEventStreamUrl,
    SelfHostedNeoCloud,
    ExtensionLifecycleCommands,
    SessionMetadataBranching,
    SessionExportJson,
    InteractiveSessionPicker,
    InteractiveModelPicker,
    InteractiveSessionFork,
    RuntimeHooksAndQueues,
    TuiUnifiedDiffRenderer,
    TuiPasteBuffering,
    TuiTranscriptSelectionCopy,
    TerminalImageProtocol,
    TuiSixelImageProtocol,
    AiAnthropicGoogleThinkingPayloads,
    AiReasoningReplayControl,
}

#[derive(Debug, Clone)]
struct ParityCodeTruth {
    implemented: BTreeSet<ImplementedSurface>,
}

#[derive(Debug, Clone)]
struct ParitySources {
    mcp: String,
    cli: String,
    session: String,
    runtime: String,
    interactive: String,
    input: String,
    tui_app: String,
    tui_components: String,
    tui_image: String,
    ai_options: String,
    anthropic: String,
    google: String,
    session_commands: String,
}

impl ParitySources {
    fn load(root: &Path) -> Result<Self> {
        Ok(Self {
            mcp: read_agent_core_source(root, &["tools", "mcp.rs"])?,
            cli: read_neo_agent_source(root, &["cli.rs"])?,
            session: read_agent_core_source(root, &["session", "mod.rs"])?,
            runtime: read_agent_core_source(root, &["runtime.rs"])?,
            interactive: read_neo_agent_source(root, &["modes", "interactive.rs"])?,
            input: read_tui_source(root, &["input.rs"])?,
            tui_app: read_tui_source(root, &["app.rs"])?,
            tui_components: read_tui_source(root, &["components.rs"])?,
            tui_image: read_tui_source(root, &["image.rs"])?,
            ai_options: read_crate_source(root, "ai", &["options.rs"])?,
            anthropic: read_ai_provider_source(root, "anthropic.rs")?,
            google: read_ai_provider_source(root, "google.rs")?,
            session_commands: read_neo_agent_source(root, &["session_commands.rs"])?,
        })
    }
}

impl ParityCodeTruth {
    fn load(root: &Path) -> Result<Self> {
        let sources = ParitySources::load(root)?;
        let mut implemented = BTreeSet::new();
        insert_backend_surfaces(&sources, &mut implemented);
        insert_interactive_surfaces(&sources, &mut implemented);
        insert_tui_surfaces(&sources, &mut implemented);
        insert_ai_surfaces(&sources, &mut implemented);
        if root.join("crates/neo-cloud/Cargo.toml").exists() {
            implemented.insert(ImplementedSurface::SelfHostedNeoCloud);
        }

        Ok(Self { implemented })
    }

    fn has(&self, surface: ImplementedSurface) -> bool {
        self.implemented.contains(&surface)
    }
}

fn insert_backend_surfaces(
    sources: &ParitySources,
    implemented: &mut BTreeSet<ImplementedSurface>,
) {
    if sources.mcp.contains("trait McpToolAdapter") && sources.mcp.contains("McpToolProvider") {
        implemented.insert(ImplementedSurface::McpToolAdapterBoundary);
    }
    if sources.mcp.contains("McpStdioToolAdapter")
        && sources.mcp.contains("tools/list")
        && sources.mcp.contains("tools/call")
    {
        implemented.insert(ImplementedSurface::StdioMcpProcessAdapter);
    }
    if sources.mcp.contains("start_resource_event_reader") {
        implemented.insert(ImplementedSurface::HttpMcpJsonSubscribeEventReader);
    }
    if sources.mcp.contains("resource_event_stream_url")
        && sources.mcp.contains("eventStreamUrl")
        && sources.mcp.contains("event_stream_url")
        && sources.mcp.contains("event_url")
    {
        implemented.insert(ImplementedSurface::McpSubscribeEventStreamUrl);
    }
    if sources.cli.contains("Status")
        && sources.cli.contains("Enable")
        && sources.cli.contains("Disable")
        && sources.cli.contains("ExtensionCommand")
    {
        implemented.insert(ImplementedSurface::ExtensionLifecycleCommands);
    }
    if sources.session.contains("SessionMetadataStore")
        && sources.session.contains("pub fn fork")
        && sources.session.contains("pub fn rename")
    {
        implemented.insert(ImplementedSurface::SessionMetadataBranching);
    }
    if sources.session_commands.contains("export_json")
        && sources.session_commands.contains("export_json_artifact")
        && sources.session_commands.contains("neo.session.export_json")
    {
        implemented.insert(ImplementedSurface::SessionExportJson);
    }
    if sources.runtime.contains("with_before_tool_call")
        && sources.runtime.contains("with_after_tool_call")
        && sources.runtime.contains("with_queue_modes")
        && sources.runtime.contains("queue_steering_message")
    {
        implemented.insert(ImplementedSurface::RuntimeHooksAndQueues);
    }
}

fn insert_interactive_surfaces(
    sources: &ParitySources,
    implemented: &mut BTreeSet<ImplementedSurface>,
) {
    if sources.interactive.contains("open_session_picker")
        && sources.interactive.contains("load_selected_session")
        && sources.interactive.contains("session_catalog_for_config")
        && sources.input.contains("SessionPickerOpen")
    {
        implemented.insert(ImplementedSurface::InteractiveSessionPicker);
    }
    if sources.interactive.contains("open_model_picker")
        && sources.interactive.contains("apply_selected_model")
        && sources.interactive.contains("model_catalog_for_config")
        && sources.input.contains("ModelPickerOpen")
    {
        implemented.insert(ImplementedSurface::InteractiveModelPicker);
    }
    if sources.interactive.contains("fork_selected_session")
        && sources.interactive.contains("fork_session_transcript")
        && sources.input.contains("SessionFork")
        && sources.input.contains("tui.session.fork")
    {
        implemented.insert(ImplementedSurface::InteractiveSessionFork);
    }
}

fn insert_tui_surfaces(sources: &ParitySources, implemented: &mut BTreeSet<ImplementedSurface>) {
    if sources.tui_app.contains("DiffAdded")
        && sources.tui_app.contains("DiffRemoved")
        && sources.tui_components.contains("transcript_line_style")
    {
        implemented.insert(ImplementedSurface::TuiUnifiedDiffRenderer);
    }
    if sources.input.contains("InputParser")
        && sources.input.contains("BRACKETED_PASTE_START")
        && sources.input.contains("BRACKETED_PASTE_END")
    {
        implemented.insert(ImplementedSurface::TuiPasteBuffering);
    }
    if sources.tui_app.contains("TranscriptSelection")
        && sources.tui_app.contains("copy_selection")
        && sources.tui_app.contains("copy_selected_transcript_text")
        && sources.tui_components.contains("with_selection")
        && sources.input.contains("TranscriptSelectionStart")
        && sources.input.contains("TranscriptCopySelection")
        && sources.input.contains("tui.transcript.copySelection")
        && sources
            .interactive
            .contains("copy_transcript_selection_to_clipboard")
    {
        implemented.insert(ImplementedSurface::TuiTranscriptSelectionCopy);
    }
    if terminal_image_protocol_symbols_exist(&[
        &sources.input,
        &sources.tui_app,
        &sources.tui_components,
        &sources.tui_image,
    ]) {
        implemented.insert(ImplementedSurface::TerminalImageProtocol);
    }
    if sources.tui_image.contains("encode_sixel_image")
        && sources.tui_image.contains("SixelImageOptions")
        && sources.tui_image.contains("SixelPaletteColor")
    {
        implemented.insert(ImplementedSurface::TuiSixelImageProtocol);
    }
}

fn insert_ai_surfaces(sources: &ParitySources, implemented: &mut BTreeSet<ImplementedSurface>) {
    if sources.anthropic.contains("thinking_budget_tokens")
        && sources.anthropic.contains("\"budget_tokens\"")
        && sources.google.contains("thinking_budget_tokens")
        && sources.google.contains("\"thinkingConfig\"")
    {
        implemented.insert(ImplementedSurface::AiAnthropicGoogleThinkingPayloads);
    }
    if sources.ai_options.contains("replay_reasoning") {
        implemented.insert(ImplementedSurface::AiReasoningReplayControl);
    }
}

fn terminal_image_protocol_symbols_exist(sources: &[&str]) -> bool {
    sources.iter().any(|source| {
        source.contains("ImageProtocolError")
            || source.contains("KittyGraphicsOptions")
            || source.contains("Iterm2InlineImageOptions")
            || source.contains("encode_kitty_graphics")
            || source.contains("encode_iterm2_inline_image")
            || source.contains("TerminalImageProtocol")
            || source.contains("InlineImageProtocol")
            || source.contains("KittyGraphicsProtocol")
            || source.contains("Sixel")
            || source.contains("Iterm2InlineImage")
            || source.contains("render_inline_image_protocol")
            || source.contains("terminal_image_protocol")
            || source.contains("kitty_graphics_protocol")
            || source.contains("sixel_protocol")
    })
}

fn read_agent_core_source(root: &Path, parts: &[&str]) -> Result<String> {
    read_crate_source(root, "agent-core", parts)
}

fn read_neo_agent_source(root: &Path, parts: &[&str]) -> Result<String> {
    read_crate_source(root, "neo-agent", parts)
}

fn read_tui_source(root: &Path, parts: &[&str]) -> Result<String> {
    read_crate_source(root, "tui", parts)
}

fn read_ai_provider_source(root: &Path, file: &str) -> Result<String> {
    read_crate_source(root, "ai", &["providers", file])
}

fn read_crate_source(root: &Path, crate_name: &str, parts: &[&str]) -> Result<String> {
    let path = parts.iter().fold(
        root.join("crates").join(crate_name).join("src"),
        |path, part| path.join(part),
    );
    read_optional_source(&path)
}

fn read_optional_source(path: &Path) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }
    Ok(fs::read_to_string(path)?)
}

fn collect_files_with_extensions(
    dir: &Path,
    extensions: &[&str],
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files_with_extensions(&path, extensions, out)?;
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extensions.contains(&extension))
        {
            out.push(path);
        }
    }
    Ok(())
}

fn parity_allowlist_reason(line: &str) -> Option<&str> {
    let (_prefix, reason) = line.split_once("xtask-parity: allow ")?;
    Some(reason.trim())
}

fn is_explicit_fixture_path(path: &Path) -> bool {
    let normalized = normalize_path(path);
    let text = normalized.to_string_lossy();
    text.contains("/tests/")
        || text.starts_with("examples/")
        || text.ends_with("/src/harness.rs")
        || text.ends_with("/src/providers/fake.rs")
}

fn is_explicit_fixture_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    let normalized = lower.replace(['-', '_'], " ");
    normalized.contains("for tests")
        || normalized.contains("test fake provider")
        || normalized.contains("fake test provider")
        || normalized.contains("fake provider")
        || normalized.contains("fake harness")
        || normalized.contains("fake model")
        || normalized.contains("fake_model")
        || normalized.contains("fakemodelclient")
        || normalized.contains("fakeharness")
        || normalized.contains("pub mod fake")
        || normalized.contains("deterministic fixture")
        || normalized.contains("not production")
        || normalized.contains("no production")
        || normalized.contains("not resolve")
}

fn is_gap_or_checker_description(line: &str) -> bool {
    let lower = line.to_lowercase();
    let normalized = lower.replace(['-', '_'], " ");
    normalized.contains("no longer placeholder level")
        || (normalized.contains("scans for production docs")
            && normalized.contains("fake or placeholder"))
        || normalized.contains("production fake/local/placeholder guidance")
        || normalized.contains("fake/local/placeholder production guidance")
        || normalized.contains("that point at fake or placeholder provider paths")
        || normalized.contains("compile time rust stub should only be added")
        || normalized.contains("rejects stale text")
        || normalized.contains("once symbols exist")
        || normalized.contains("once `mcptooladapter`")
        || normalized.contains("once status/enable/disable commands")
        || normalized.contains("session branching and naming are future work\" once")
        || normalized.contains("once `sessionmetadatastore::fork`")
        || normalized.contains("once lifecycle commands exist")
        || normalized.contains("once fork exists")
        || normalized.contains("parity gate")
}

fn parity_line_is_fixture_safe(line: &str) -> bool {
    let lower = line.to_lowercase();
    let normalized = lower.replace(['-', '_'], " ");
    if (normalized.contains("production") || normalized.contains("prod "))
        && !is_explicit_fixture_line(line)
    {
        return false;
    }

    contains_any_word(
        &normalized,
        &[
            "fake",
            "placeholder",
            "stub",
            "dummy",
            "mock",
            "todo",
            "tbd",
        ],
    ) || normalized.contains("127.0.0.1")
        || normalized.contains("localhost")
}

fn parity_line_violation(
    relative_file: &Path,
    line: &str,
    explicit_fixture_path: bool,
    code_truth: &ParityCodeTruth,
) -> Option<&'static str> {
    let lower = line.to_lowercase();
    let normalized = lower.replace(['-', '_'], " ");
    if is_gap_or_checker_description(line) {
        return None;
    }
    if normalized.contains("parity")
        && (normalized.contains("scan")
            || normalized.contains("gate")
            || normalized.contains("guidance")
            || normalized.contains("allowlist"))
    {
        return None;
    }

    if should_scan_auth_token_leaks(relative_file) && contains_auth_token_leak(line) {
        return Some("auth token leak");
    }

    if should_scan_package_signature_fixtures(relative_file)
        && contains_private_package_signature_material(line)
    {
        return Some("private package signature material");
    }

    let production_context = contains_any_word(
        &normalized,
        &["production", "prod", "default", "defaults", "deploy"],
    );
    let fake_context = contains_any_word(
        &normalized,
        &[
            "fake",
            "placeholder",
            "stub",
            "dummy",
            "mock",
            "fixture",
            "deterministic",
        ],
    ) || normalized.contains("localhost")
        || normalized.contains("127.0.0.1");
    let local_provider_context = normalized.contains("local provider")
        || normalized.contains("local providers")
        || normalized.contains("api kind::local")
        || normalized.contains("apikind::local");

    if is_explicit_fixture_line(line) {
        return None;
    }

    if let Some(reason) = stale_gap_claim_violation(&normalized, code_truth) {
        return Some(reason);
    }

    if hosted_or_oauth_overclaim(&normalized) {
        return Some("hosted/OAuth overclaim");
    }

    if self_hosted_or_local_first_overclaim(&normalized, code_truth) {
        return Some("unbacked self-hosted/local-first claim");
    }

    if package_trust_overclaim(relative_file, &normalized) {
        return Some("package trust overclaim");
    }

    if image_runtime_detection_overclaim(&normalized) {
        return Some("image runtime-detection overclaim");
    }

    if production_context && fake_context {
        return Some("production fake/default guidance");
    }

    if production_context && local_provider_context && !is_rejection_or_gap_statement(&normalized) {
        return Some("production local-provider guidance");
    }

    if explicit_fixture_path && parity_line_is_fixture_safe(line) {
        return None;
    }

    if contains_any_word(&normalized, &["todo", "tbd"]) {
        return Some("unresolved placeholder marker");
    }

    if contains_any_word(
        &normalized,
        &["fake", "placeholder", "stub", "dummy", "mock"],
    ) {
        return Some("production fake/placeholder marker");
    }

    if normalized.contains("local deterministic") || normalized.contains("deterministic guidance") {
        return Some("local deterministic guidance");
    }

    None
}

fn hosted_or_oauth_overclaim(normalized: &str) -> bool {
    (normalized.contains("oauth")
        || normalized.contains("hosted provider")
        || normalized.contains("managed hosted")
        || normalized.contains("hosted collaboration")
        || normalized.contains("hosted session")
        || normalized.contains("hosted share")
        || normalized.contains("package account"))
        && positive_claim_statement(normalized)
        && !honest_gap_or_rejection_statement(normalized)
}

fn self_hosted_or_local_first_overclaim(normalized: &str, code_truth: &ParityCodeTruth) -> bool {
    !code_truth.has(ImplementedSurface::SelfHostedNeoCloud)
        && (normalized.contains("self hosted")
            || normalized.contains("self-hosted")
            || normalized.contains("local first")
            || normalized.contains("local-first"))
        && positive_claim_statement(normalized)
        && !honest_gap_or_rejection_statement(normalized)
}

fn package_trust_overclaim(relative_file: &Path, normalized: &str) -> bool {
    let path = normalize_path(relative_file)
        .to_string_lossy()
        .to_lowercase();
    (path.contains("package") || normalized.contains("package"))
        && (normalized.contains("publisher/root")
            || (normalized.contains("publisher") && normalized.contains("root trust"))
            || normalized.contains("trust chain")
            || normalized.contains("root trust anchor"))
        && positive_claim_statement(normalized)
        && !honest_gap_or_rejection_statement(normalized)
}

fn image_runtime_detection_overclaim(normalized: &str) -> bool {
    (normalized.contains("image protocol")
        || normalized.contains("terminal image")
        || normalized.contains("kitty")
        || normalized.contains("sixel")
        || normalized.contains("iterm2"))
        && (normalized.contains("runtime detection")
            || normalized.contains("auto detect")
            || normalized.contains("auto-detect")
            || normalized.contains("detects")
            || normalized.contains("detection/negotiation")
            || normalized.contains("capability detection")
            || normalized.contains("capability negotiation"))
        && positive_claim_statement(normalized)
        && !honest_gap_or_rejection_statement(normalized)
}

fn positive_claim_statement(normalized: &str) -> bool {
    contains_any_word(
        normalized,
        &[
            "support",
            "supports",
            "supported",
            "available",
            "ready",
            "implemented",
            "exposes",
            "provides",
            "establish",
            "establishes",
            "backed",
        ],
    ) || normalized.contains("can ")
        || normalized.contains("is implemented")
}

fn honest_gap_or_rejection_statement(normalized: &str) -> bool {
    is_rejection_or_gap_statement(normalized)
        || normalized.contains("future work")
        || normalized.contains("out of scope")
        || normalized.contains("not yet")
        || normalized.contains("does not")
        || normalized.contains("do not")
        || normalized.contains("cannot")
        || normalized.contains("can't")
        || normalized.contains("remain")
        || normalized.contains("remaining")
        || normalized.contains("limitation")
        || normalized.contains("only when")
        || normalized.contains("not a ")
        || normalized.contains("not an ")
}

fn should_scan_auth_token_leaks(path: &Path) -> bool {
    let text = normalize_path(path).to_string_lossy().to_lowercase();
    text.starts_with("docs/") || text.starts_with("examples/")
}

fn should_scan_package_signature_fixtures(path: &Path) -> bool {
    let text = normalize_path(path).to_string_lossy().to_lowercase();
    (text.starts_with("examples/") || text.starts_with("docs/"))
        && (text.contains("signature") || text.contains("package"))
}

fn contains_auth_token_leak(line: &str) -> bool {
    contains_bearer_token_leak(line) || contains_inline_api_key_leak(line)
}

fn contains_bearer_token_leak(line: &str) -> bool {
    let lower = line.to_lowercase();
    let Some(bearer_index) = lower.find("bearer ") else {
        return false;
    };
    let token = line[bearer_index + "bearer ".len()..]
        .split(|character: char| character.is_whitespace() || matches!(character, '"' | '\'' | '`'))
        .next()
        .unwrap_or_default()
        .trim_matches(|character: char| {
            matches!(character, ',' | ';' | ')' | ']' | '}' | '"' | '\'' | '`')
        });
    looks_like_secret_token(token)
}

fn contains_inline_api_key_leak(line: &str) -> bool {
    let lower = line.to_lowercase();
    if lower.contains("api_key_env") || lower.contains("api key env") {
        return false;
    }

    for marker in ["api_key", "apikey", "api-key", "token", "auth_token"] {
        let Some(index) = lower.find(marker) else {
            continue;
        };
        let value = line[index + marker.len()..]
            .trim_start_matches(|character: char| {
                character.is_whitespace() || matches!(character, '=' | ':' | '"' | '\'' | '`')
            })
            .split(|character: char| {
                character.is_whitespace() || matches!(character, '"' | '\'' | '`' | ',' | ';')
            })
            .next()
            .unwrap_or_default();
        if looks_like_secret_token(value) {
            return true;
        }
    }
    false
}

fn looks_like_secret_token(value: &str) -> bool {
    let token = value.trim();
    if token.len() < 20 {
        return false;
    }
    if token.contains('$')
        || token.contains('<')
        || token.contains('>')
        || token.contains('{')
        || token.contains('}')
        || token.contains("...")
        || token.eq_ignore_ascii_case("redacted")
        || token.eq_ignore_ascii_case("example")
    {
        return false;
    }
    let lower = token.to_lowercase();
    lower.starts_with("sk-")
        || lower.starts_with("pk-")
        || lower.starts_with("ghp_")
        || lower.starts_with("github_pat_")
        || token.chars().filter(char::is_ascii_alphanumeric).count() >= 24
}

fn contains_private_package_signature_material(line: &str) -> bool {
    let lower = line.to_lowercase();
    (lower.contains("privatekey") || lower.contains("private_key") || lower.contains("private key"))
        && (lower.contains("begin private key")
            || lower.contains("-----begin")
            || lower.contains("\"signature\"")
            || lower.contains("'signature'"))
}

fn stale_gap_claim_violation(
    normalized: &str,
    code_truth: &ParityCodeTruth,
) -> Option<&'static str> {
    stale_cloud_gap_claim_violation(normalized, code_truth)
        .or_else(|| stale_backend_gap_claim_violation(normalized, code_truth))
        .or_else(|| stale_interactive_gap_claim_violation(normalized, code_truth))
        .or_else(|| stale_tui_gap_claim_violation(normalized, code_truth))
        .or_else(|| stale_ai_gap_claim_violation(normalized, code_truth))
}

fn stale_cloud_gap_claim_violation(
    normalized: &str,
    code_truth: &ParityCodeTruth,
) -> Option<&'static str> {
    if code_truth.has(ImplementedSurface::SelfHostedNeoCloud)
        && normalized.contains("neo cloud")
        && normalized.contains("smoke")
        && (normalized.contains("future work")
            || normalized.contains("missing")
            || normalized.contains("not implemented")
            || normalized.contains("remain"))
    {
        return Some("stale self-hosted cloud smoke gap claim");
    }

    None
}

fn stale_backend_gap_claim_violation(
    normalized: &str,
    code_truth: &ParityCodeTruth,
) -> Option<&'static str> {
    if code_truth.has(ImplementedSurface::McpToolAdapterBoundary)
        && normalized.contains("mcp")
        && normalized.contains("adapter")
        && (normalized.contains("not wired")
            || normalized.contains("no mcp")
            || normalized.contains("future work"))
    {
        return Some("stale MCP adapter gap claim");
    }

    if code_truth.has(ImplementedSurface::StdioMcpProcessAdapter)
        && normalized.contains("mcp")
        && normalized.contains("process")
        && (normalized.contains("does not yet spawn")
            || normalized.contains("not yet spawn")
            || normalized.contains("not implemented")
            || normalized.contains("future work"))
    {
        return Some("stale MCP process adapter gap claim");
    }

    if code_truth.has(ImplementedSurface::HttpMcpJsonSubscribeEventReader)
        && normalized.contains("mcp")
        && normalized.contains("json")
        && normalized.contains("subscribe")
        && (normalized.contains("ack") || normalized.contains("resource update"))
        && (normalized.contains("cannot receive")
            || normalized.contains("can't receive")
            || normalized.contains("does not receive")
            || normalized.contains("not receive")
            || normalized.contains("no update")
            || normalized.contains("missing")
            || normalized.contains("not implemented")
            || normalized.contains("future work"))
    {
        return Some("stale HTTP MCP JSON subscribe event gap claim");
    }

    if code_truth.has(ImplementedSurface::McpSubscribeEventStreamUrl)
        && normalized.contains("mcp")
        && normalized.contains("subscribe")
        && (normalized.contains("event stream url")
            || normalized.contains("event-stream url")
            || normalized.contains("event-channel url")
            || normalized.contains("event channel url")
            || normalized.contains("eventstreamurl")
            || normalized.contains("event_stream_url")
            || normalized.contains("event_url"))
        && (normalized.contains("cannot")
            || normalized.contains("can't")
            || normalized.contains("not implemented")
            || normalized.contains("future work")
            || normalized.contains("missing")
            || normalized.contains("yet"))
    {
        return Some("stale MCP subscribe event URL gap claim");
    }

    if code_truth.has(ImplementedSurface::ExtensionLifecycleCommands)
        && normalized.contains("extension lifecycle")
        && (normalized.contains("do not document")
            || normalized.contains("not available")
            || normalized.contains("unavailable")
            || normalized.contains("not implemented")
            || normalized.contains("future work"))
    {
        return Some("stale extension lifecycle gap claim");
    }

    if code_truth.has(ImplementedSurface::SessionMetadataBranching)
        && (normalized.contains("branching") || normalized.contains("fork"))
        && normalized.contains("naming")
        && (normalized.contains("future work")
            || normalized.contains("remain")
            || normalized.contains("missing")
            || normalized.contains("not implemented"))
    {
        return Some("stale session branching gap claim");
    }

    if code_truth.has(ImplementedSurface::SessionExportJson)
        && normalized.contains("session")
        && (normalized.contains("export-json") || normalized.contains("export json"))
        && (normalized.contains("future work")
            || normalized.contains("missing")
            || normalized.contains("not implemented")
            || normalized.contains("remain"))
    {
        return Some("stale session export-json gap claim");
    }

    if code_truth.has(ImplementedSurface::RuntimeHooksAndQueues)
        && normalized.contains("hook")
        && normalized.contains("steering")
        && normalized.contains("docs")
        && (normalized.contains("only when")
            || normalized.contains("not exposed")
            || normalized.contains("future work")
            || normalized.contains("missing"))
    {
        return Some("stale runtime hook/queue gap claim");
    }

    None
}

fn stale_interactive_gap_claim_violation(
    normalized: &str,
    code_truth: &ParityCodeTruth,
) -> Option<&'static str> {
    if code_truth.has(ImplementedSurface::InteractiveSessionPicker)
        && normalized.contains("session picker")
        && (normalized.contains("missing")
            || normalized.contains("not implemented")
            || normalized.contains("future work"))
    {
        return Some("stale live session picker gap claim");
    }

    if code_truth.has(ImplementedSurface::InteractiveModelPicker)
        && normalized.contains("model picker")
        && (normalized.contains("missing")
            || normalized.contains("not implemented")
            || normalized.contains("future work"))
    {
        return Some("stale live model picker gap claim");
    }

    if code_truth.has(ImplementedSurface::InteractiveSessionFork)
        && (normalized.contains("fork-before-continue")
            || normalized.contains("fork before continue"))
        && (normalized.contains("missing")
            || normalized.contains("not implemented")
            || normalized.contains("future work")
            || normalized.contains("gap")
            || normalized.contains("only when")
            || normalized.contains("beyond local jsonl append"))
    {
        return Some("stale interactive session fork gap claim");
    }

    None
}

fn stale_tui_gap_claim_violation(
    normalized: &str,
    code_truth: &ParityCodeTruth,
) -> Option<&'static str> {
    if code_truth.has(ImplementedSurface::TuiUnifiedDiffRenderer)
        && stale_tui_diff_renderer_claim(normalized)
    {
        return Some("stale TUI unified diff renderer gap claim");
    }

    if code_truth.has(ImplementedSurface::TuiPasteBuffering)
        && (normalized.contains("stdin buffering") || normalized.contains("paste buffering"))
        && (normalized.contains("not implement")
            || normalized.contains("not implemented")
            || normalized.contains("missing")
            || normalized.contains("future work")
            || normalized.contains("until")
            || normalized.contains("land"))
    {
        return Some("stale TUI paste buffering gap claim");
    }

    if code_truth.has(ImplementedSurface::TuiTranscriptSelectionCopy)
        && stale_tui_transcript_selection_copy_claim(normalized)
    {
        return Some("stale TUI transcript selection copy gap claim");
    }

    if code_truth.has(ImplementedSurface::TerminalImageProtocol)
        && stale_terminal_image_protocol_claim(normalized)
    {
        return Some("stale terminal image protocol gap claim");
    }

    if code_truth.has(ImplementedSurface::TuiSixelImageProtocol)
        && stale_sixel_image_protocol_claim(normalized)
    {
        return Some("stale Sixel image protocol gap claim");
    }

    None
}

fn stale_tui_diff_renderer_claim(normalized: &str) -> bool {
    (normalized.contains("diff renderer")
        || normalized.contains("diff rendering")
        || normalized.contains("unified diff"))
        && (normalized.contains("not implement")
            || normalized.contains("not implemented")
            || normalized.contains("missing")
            || normalized.contains("future work")
            || normalized.contains("until diff rendering"))
}

fn stale_tui_transcript_selection_copy_claim(normalized: &str) -> bool {
    let Some(copy_index) = normalized.find("copy") else {
        return false;
    };
    let context = {
        let prefix = normalized[..copy_index]
            .chars()
            .rev()
            .take(80)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>();
        let suffix = normalized[copy_index..]
            .chars()
            .take(120)
            .collect::<String>();
        format!("{prefix}{suffix}")
    };

    (context.contains("selected transcript")
        || normalized.contains("transcript region")
        || normalized.contains("transcript-region")
        || context.contains("transcript selection"))
        && (context.contains("not implement")
            || context.contains("not implemented")
            || context.contains("missing")
            || context.contains("future work")
            || context.contains("remain")
            || context.contains("gap"))
}

fn stale_terminal_image_protocol_claim(normalized: &str) -> bool {
    if normalized.contains("runtime detection")
        || normalized.contains("runtime image-protocol detection")
        || normalized.contains("detection/negotiation")
        || normalized.contains("detection and negotiation")
        || normalized.contains("capability detection")
        || normalized.contains("capability negotiation")
    {
        return false;
    }

    (normalized.contains("terminal image protocol")
        || normalized.contains("terminal image protocols")
        || (normalized.contains("image protocol")
            && (normalized.contains("no ")
                || normalized.contains("without ")
                || normalized.contains("not implement")
                || normalized.contains("not implemented")))
        || (normalized.contains("image protocols")
            && (normalized.contains("no ")
                || normalized.contains("without ")
                || normalized.contains("not implement")
                || normalized.contains("not implemented"))))
        && (normalized.contains("not implement")
            || normalized.contains("not implemented")
            || normalized.contains("missing")
            || normalized.contains("future work")
            || normalized.contains("gap")
            || normalized.contains("unsupported"))
}

fn stale_sixel_image_protocol_claim(normalized: &str) -> bool {
    normalized.contains("sixel")
        && (normalized.contains("not implement")
            || normalized.contains("not implemented")
            || normalized.contains("missing")
            || normalized.contains("future work")
            || normalized.contains("remain"))
        && !(normalized.contains("runtime")
            || normalized.contains("detection")
            || normalized.contains("negotiation")
            || normalized.contains("integration")
            || normalized.contains("renderer"))
}

fn stale_ai_gap_claim_violation(
    normalized: &str,
    code_truth: &ParityCodeTruth,
) -> Option<&'static str> {
    if code_truth.has(ImplementedSurface::AiAnthropicGoogleThinkingPayloads)
        && normalized.contains("anthropic")
        && normalized.contains("google")
        && normalized.contains("thinking")
        && (normalized.contains("only after")
            || normalized.contains("future work")
            || normalized.contains("missing")
            || normalized.contains("not implemented")
            || normalized.contains("does not translate")
            || normalized.contains("not translate"))
    {
        return Some("stale Anthropic/Google thinking payload gap claim");
    }

    if code_truth.has(ImplementedSurface::AiReasoningReplayControl)
        && (normalized.contains("reasoning replay")
            || normalized.contains("replay signed reasoning")
            || normalized.contains("signed reasoning replay")
            || normalized.contains("thinking off"))
        && (normalized.contains("cannot")
            || normalized.contains("can't")
            || normalized.contains("not implemented")
            || normalized.contains("future work")
            || normalized.contains("missing")
            || normalized.contains("yet"))
    {
        return Some("stale reasoning replay-control gap claim");
    }

    None
}

fn is_rejection_or_gap_statement(normalized: &str) -> bool {
    contains_any_word(
        normalized,
        &[
            "reject",
            "rejects",
            "rejected",
            "unsupported",
            "never",
            "missing",
            "gap",
            "gaps",
        ],
    )
}

fn contains_any_word(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| contains_word(haystack, needle))
}

fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle
        .chars()
        .all(|character| character.is_ascii_alphanumeric())
    {
        haystack
            .split(|character: char| {
                !character.is_ascii_alphanumeric() && character != '.' && character != ':'
            })
            .any(|word| word == needle)
    } else {
        haystack.contains(needle)
    }
}

fn validate_examples(root: &Path) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let minimal_config = root.join("examples/config/minimal.toml");
    if minimal_config.exists() {
        errors.extend(validate_minimal_config(root, &minimal_config)?);
    }

    let mcp_config = root.join("examples/config/mcp-server.toml");
    if mcp_config.exists() {
        errors.extend(validate_mcp_config(root, &mcp_config)?);
    }

    let read_schema = root.join("examples/tools/read-file-schema.json");
    if read_schema.exists() {
        errors.extend(validate_read_file_schema(root, &read_schema)?);
    }

    errors.extend(validate_rust_examples(root)?);
    errors.sort();
    Ok(errors)
}

fn run_rust_examples_compile_gate(root: &Path) -> Result<()> {
    let manifest = root.join("examples/rust/Cargo.toml");
    if !manifest.exists() {
        return Ok(());
    }

    run(&CommandStep {
        program: "cargo".to_owned(),
        args: vec![
            "check".to_owned(),
            "--manifest-path".to_owned(),
            manifest.display().to_string(),
            "--examples".to_owned(),
        ],
        env: Vec::new(),
        current_dir: None,
    })
}

fn validate_rust_examples(root: &Path) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let example_dir = root.join("examples/rust");
    if !example_dir.is_dir() {
        return Ok(errors);
    }

    let mut example_files = Vec::new();
    collect_files_with_extensions(&example_dir, &["rs"], &mut example_files)?;
    example_files.sort();
    if example_files.is_empty() {
        return Ok(errors);
    }

    let manifest = example_dir.join("Cargo.toml");
    let relative_manifest = relative_path(root, &manifest);
    if !manifest.exists() {
        errors.push("examples/rust contains Rust examples but is missing Cargo.toml".to_owned());
        return Ok(errors);
    }

    let source = fs::read_to_string(&manifest)?;
    let value = match toml::from_str::<toml::Value>(&source) {
        Ok(value) => value,
        Err(error) => {
            errors.push(format!(
                "{} is invalid TOML: {error}",
                relative_manifest.display()
            ));
            return Ok(errors);
        }
    };

    let declared_paths = value
        .get("example")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|target| target.get("path"))
        .filter_map(toml::Value::as_str)
        .map(|path| normalize_path(Path::new(path)))
        .collect::<BTreeSet<_>>();

    if declared_paths.is_empty() {
        errors.push(format!(
            "{} must declare at least one [[example]] target",
            relative_manifest.display()
        ));
        return Ok(errors);
    }

    for example_file in example_files {
        let relative_to_manifest = example_file
            .strip_prefix(&example_dir)
            .map_or_else(|_| normalize_path(&example_file), normalize_path);
        if !declared_paths.contains(&relative_to_manifest) {
            errors.push(format!(
                "{} does not declare example target for {}",
                relative_manifest.display(),
                relative_path(root, &example_file).display()
            ));
        }
    }

    Ok(errors)
}

fn validate_minimal_config(root: &Path, path: &Path) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let relative = relative_path(root, path);
    let source = fs::read_to_string(path)?;
    let keys = top_level_toml_keys(&source);

    for key in [
        "default_provider",
        "default_model",
        "sessions_dir",
        "permissions",
        "defaults",
    ] {
        if !keys.contains(&key) {
            errors.push(format!("{} is missing `{key}`", relative.display()));
        }
    }

    if keys.contains(&"api_base") {
        errors.push(format!(
            "{} must not set api_base in the minimal development fixture",
            relative.display()
        ));
    }

    for key in keys {
        let allowed = [
            "default_provider",
            "default_model",
            "sessions_dir",
            "permissions",
            "defaults",
        ];
        if !allowed.contains(&key) {
            errors.push(format!(
                "{} contains unsupported minimal config key `{key}`",
                relative.display()
            ));
        }
    }

    Ok(errors)
}

fn validate_mcp_config(root: &Path, path: &Path) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let relative = relative_path(root, path);
    let source = fs::read_to_string(path)?;
    let Some(server_body) = first_toml_array_table(&source, "mcp.servers") else {
        errors.push(format!(
            "{} must define [[mcp.servers]]",
            relative.display()
        ));
        return Ok(errors);
    };

    for key in ["id", "enabled", "transport", "command"] {
        if !toml_body_has_key(server_body, key) {
            errors.push(format!(
                "{} mcp server #0 is missing `{key}`",
                relative.display()
            ));
        }
    }

    Ok(errors)
}

fn validate_read_file_schema(root: &Path, path: &Path) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let relative = relative_path(root, path);
    let source = fs::read_to_string(path)?;

    if !source.contains("\"properties\"") {
        errors.push(format!(
            "{} must define object properties",
            relative.display()
        ));
        return Ok(errors);
    }
    if !source.contains("\"path\"") {
        errors.push(format!(
            "{} must include a `path` property",
            relative.display()
        ));
    }
    if !source.contains("\"required\"") || !source.contains("\"path\"") {
        errors.push(format!("{} must require `path`", relative.display()));
    }

    Ok(errors)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogRequirement {
    Optional,
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogValidationReport {
    checked: usize,
    errors: Vec<String>,
}

fn validate_catalog_schemas(
    root: &Path,
    requirement: CatalogRequirement,
) -> Result<CatalogValidationReport> {
    let mut checked = 0;
    let mut errors = Vec::new();
    for path in catalog_schema_candidate_paths(root) {
        if !path.exists() {
            continue;
        }
        checked += 1;
        let source = fs::read_to_string(&path)?;
        if !looks_like_json_object(&source) {
            errors.push(format!(
                "{} must be a JSON object schema",
                relative_path(root, &path).display()
            ));
            continue;
        }
        if !json_field_equals(&source, "type", "object") || !source.contains("\"properties\"") {
            errors.push(format!(
                "{} must be an object schema with `properties`",
                relative_path(root, &path).display()
            ));
        }
    }

    if checked == 0 && requirement == CatalogRequirement::Required {
        errors.push(format!(
            "missing generated model catalog schema; expected one of {}",
            human_join(&catalog_schema_candidate_labels())
        ));
    }

    errors.sort();
    Ok(CatalogValidationReport { checked, errors })
}

fn catalog_schema_candidate_paths(root: &Path) -> Vec<PathBuf> {
    catalog_schema_candidate_labels()
        .into_iter()
        .map(|path| root.join(path))
        .collect()
}

fn catalog_schema_candidate_labels() -> Vec<&'static str> {
    vec![
        "docs/generated/model-catalog.schema.json",
        "docs/generated/models.schema.json",
        "docs/reference/model-catalog.schema.json",
        "examples/catalog/model-catalog.schema.json",
    ]
}

fn human_join(values: &[&str]) -> String {
    match values {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [first, second] => format!("{first} or {second}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}

fn validate_generated_cloud_api_schema_links(root: &Path) -> Result<Vec<String>> {
    let link_pattern = Regex::new(r"\[[^\]]+\]\(([^)]+)\)")?;
    let mut errors = Vec::new();
    for file in markdown_files(root)? {
        let source = fs::read_to_string(&file)?;
        let relative_file = relative_path(root, &file);
        let Some(parent) = file.parent() else {
            continue;
        };

        for (index, line) in source.lines().enumerate() {
            for captures in link_pattern.captures_iter(line) {
                let Some(raw_target) = captures.get(1).map(|target| target.as_str().trim()) else {
                    continue;
                };
                let Some(target) = local_link_target(raw_target) else {
                    continue;
                };
                let normalized_target = normalize_path(Path::new(target));
                if !is_generated_cloud_api_schema_path(&normalized_target) {
                    continue;
                }
                let target_path = parent.join(&normalized_target);
                if !target_path.exists() {
                    errors.push(format!(
                        "{}:{} links to missing generated cloud API schema {}",
                        relative_file.display(),
                        index + 1,
                        relative_path(root, &target_path).display()
                    ));
                }
            }
        }
    }

    errors.sort();
    Ok(errors)
}

fn is_generated_cloud_api_schema_path(path: &Path) -> bool {
    let text = normalize_path(path).to_string_lossy().to_lowercase();
    text.contains("generated")
        && text.contains("cloud")
        && (text.contains("openapi") || text.contains("schema"))
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

fn looks_like_json_object(source: &str) -> bool {
    let trimmed = source.trim();
    trimmed.starts_with('{') && trimmed.ends_with('}')
}

fn json_field_equals(source: &str, field: &str, expected: &str) -> bool {
    let pattern = format!(
        r#""{}"\s*:\s*"{}""#,
        regex::escape(field),
        regex::escape(expected)
    );
    Regex::new(&pattern).is_ok_and(|regex| regex.is_match(source))
}

fn top_level_toml_keys(source: &str) -> Vec<&str> {
    let mut keys = Vec::new();
    let mut in_top_level = true;

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(section) = trimmed
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .filter(|section| !section.starts_with('['))
        {
            keys.push(section.trim());
            in_top_level = false;
            continue;
        }
        if in_top_level && let Some((key, _value)) = trimmed.split_once('=') {
            let key = key.trim();
            if !key.is_empty() {
                keys.push(key);
            }
        }
    }

    keys
}

fn first_toml_array_table<'a>(source: &'a str, table: &str) -> Option<&'a str> {
    let header = format!("[[{table}]]");
    let start = source.find(&header)? + header.len();
    let rest = &source[start..];
    let end = rest.find("\n[[").unwrap_or(rest.len());
    Some(&rest[..end])
}

fn toml_body_has_key(body: &str, expected: &str) -> bool {
    body.lines().any(|line| {
        let trimmed = line.trim();
        trimmed
            .split_once('=')
            .is_some_and(|(key, _value)| key.trim() == expected)
    })
}

fn markdown_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in ["README.md", "docs", "examples"] {
        let path = root.join(entry);
        if path.is_file() && path.extension().is_some_and(|extension| extension == "md") {
            out.push(path);
        } else if path.is_dir() {
            collect_markdown_files(&path, &mut out)?;
        }
    }
    out.sort();
    Ok(out)
}

fn collect_markdown_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_markdown_files(&path, out)?;
        } else if path.extension().is_some_and(|extension| extension == "md") {
            out.push(path);
        }
    }
    Ok(())
}

fn local_link_target(target: &str) -> Option<&str> {
    if target.is_empty()
        || target.starts_with('#')
        || target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
    {
        return None;
    }

    let without_anchor = target
        .split_once('#')
        .map_or(target, |(path, _anchor)| path);
    let without_query = without_anchor
        .split_once('?')
        .map_or(without_anchor, |(path, _query)| path);
    (!without_query.is_empty()).then_some(without_query)
}

fn relative_path(root: &Path, path: &Path) -> PathBuf {
    let relative = path.strip_prefix(root).map_or(path, |stripped| stripped);
    normalize_path(relative)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(component.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_check_runs_xtask_gate() {
        let steps = check_steps(&CheckOptions::default());

        assert_eq!(
            steps,
            vec![
                CommandStep::new("cargo", &["fmt", "-p", "xtask", "--check"]),
                CommandStep::new(
                    "cargo",
                    &[
                        "clippy",
                        "-p",
                        "xtask",
                        "--all-targets",
                        "--",
                        "-D",
                        "warnings"
                    ]
                ),
                CommandStep::new("cargo", &["test", "-p", "xtask"]),
                CommandStep::new("cargo", &["run", "-p", "xtask", "--", "catalog", "check"]),
            ]
        );
    }

    #[test]
    fn workspace_check_opts_into_workspace_gate() {
        let steps = check_steps(&CheckOptions {
            workspace: true,
            ..CheckOptions::default()
        });

        assert_eq!(
            steps,
            vec![
                CommandStep::new("cargo", &["fmt", "--all", "--check"]),
                CommandStep::new(
                    "cargo",
                    &[
                        "clippy",
                        "--workspace",
                        "--all-targets",
                        "--all-features",
                        "--",
                        "-D",
                        "warnings"
                    ]
                ),
                CommandStep::new("cargo", &["test", "--workspace", "--all-features"]),
                CommandStep::new("cargo", &["run", "-p", "xtask", "--", "catalog", "check"]),
            ]
        );
    }

    #[test]
    fn quick_check_limits_execution_to_xtask_package() {
        let steps = check_steps(&CheckOptions {
            quick: true,
            ..CheckOptions::default()
        });

        assert_eq!(
            steps,
            vec![
                CommandStep::new("cargo", &["fmt", "-p", "xtask", "--check"]),
                CommandStep::new(
                    "cargo",
                    &[
                        "clippy",
                        "-p",
                        "xtask",
                        "--all-targets",
                        "--",
                        "-D",
                        "warnings"
                    ]
                ),
                CommandStep::new("cargo", &["test", "-p", "xtask"]),
                CommandStep::new("cargo", &["run", "-p", "xtask", "--", "catalog", "check"]),
            ]
        );
    }

    #[test]
    fn parity_gate_requires_generated_catalog_schema_artifact() {
        let dir = tempfile::tempdir().expect("tempdir");

        let errors = validate_parity_gate(dir.path()).expect("parity gate");

        assert_eq!(
            errors,
            vec![
                "missing generated model catalog schema; expected one of docs/generated/model-catalog.schema.json, docs/generated/models.schema.json, docs/reference/model-catalog.schema.json, or examples/catalog/model-catalog.schema.json".to_string()
            ]
        );
    }

    #[test]
    fn docs_validation_fails_missing_local_markdown_links() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("index.md"),
            "[Missing](./missing.md)\n[Anchor](./index.md#heading)\n[Web](https://example.com)\n",
        )
        .expect("write docs");

        let errors = validate_docs_links(dir.path()).expect("docs validation should run");

        assert_eq!(
            errors,
            vec!["docs/index.md links to missing local file docs/missing.md".to_string()]
        );
    }

    #[test]
    fn parity_validation_rejects_fake_production_defaults_without_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("config.md"),
            "Production defaults may use provider = \"fake\" and http://127.0.0.1:11434/v1.\n",
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/config.md:1 contains production fake/default guidance: Production defaults may use provider = \"fake\" and http://127.0.0.1:11434/v1.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_accepts_explicit_fake_fixture_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("examples").join("config")).expect("examples dir");
        std::fs::write(
            dir.path().join("examples").join("config").join("minimal.toml"),
            "# xtask-parity: allow fake-provider-example - deterministic development fixture.\ndefault_provider = \"fake\"\n",
        )
        .expect("write example");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parity_validation_rejects_allowlisted_fake_production_source() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("crates").join("neo-agent").join("src");
        std::fs::create_dir_all(&src).expect("crate src dir");
        std::fs::write(
            src.join("config.rs"),
            concat!(
                "// xtask-parity: allow fake-provider-example - fixture only.\n",
                "const DEFAULT_PROVIDER: &str = \"fake\";\n",
            ),
        )
        .expect("write production source");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "crates/neo-agent/src/config.rs:2 contains production fake/default guidance: const DEFAULT_PROVIDER: &str = \"fake\";".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_local_production_guidance() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("providers.md"),
            "Production deployments can use the local provider by default.\n",
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/providers.md:1 contains production local-provider guidance: Production deployments can use the local provider by default.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_softened_local_production_guidance() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("providers.md"),
            concat!(
                "Production local provider is not risky.\n",
                "Future production deployments can use the local provider.\n",
            ),
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/providers.md:1 contains production local-provider guidance: Production local provider is not risky.".to_string(),
                "docs/providers.md:2 contains production local-provider guidance: Future production deployments can use the local provider.".to_string(),
            ]
        );
    }

    #[test]
    fn parity_validation_allows_local_provider_rejection_docs() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("providers.md"),
            "ProviderResolver rejects test-only/local providers in production resolution.\n",
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parity_validation_scans_production_crate_sources() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("crates").join("neo-agent").join("src");
        std::fs::create_dir_all(&src).expect("crate src dir");
        std::fs::write(
            src.join("config.rs"),
            "const DEFAULT_PROVIDER: &str = \"fake\";\n",
        )
        .expect("write production source");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "crates/neo-agent/src/config.rs:1 contains production fake/default guidance: const DEFAULT_PROVIDER: &str = \"fake\";".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_allows_explicit_source_fixtures() {
        let dir = tempfile::tempdir().expect("tempdir");
        let provider_src = dir
            .path()
            .join("crates")
            .join("ai")
            .join("src")
            .join("providers");
        let core_src = dir.path().join("crates").join("agent-core").join("src");
        std::fs::create_dir_all(&provider_src).expect("provider src dir");
        std::fs::create_dir_all(&core_src).expect("core src dir");
        std::fs::write(
            provider_src.join("fake.rs"),
            "pub struct FakeModelClient;\n",
        )
        .expect("write fake provider");
        std::fs::write(core_src.join("harness.rs"), "pub fn fake_model() {}\n")
            .expect("write harness");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parity_validation_allows_gap_and_checker_descriptions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mcp_source = dir
            .path()
            .join("crates")
            .join("agent-core")
            .join("src")
            .join("tools");
        let session_source = dir
            .path()
            .join("crates")
            .join("agent-core")
            .join("src")
            .join("session");
        let agent_source = dir.path().join("crates").join("neo-agent").join("src");
        std::fs::create_dir_all(&mcp_source).expect("mcp source dir");
        std::fs::create_dir_all(&session_source).expect("session source dir");
        std::fs::create_dir_all(&agent_source).expect("agent source dir");
        std::fs::write(
            mcp_source.join("mcp.rs"),
            "pub trait McpToolAdapter {}\npub struct McpToolProvider;\n",
        )
        .expect("write mcp source");
        std::fs::write(
            session_source.join("mod.rs"),
            "impl SessionMetadataStore { pub fn fork(&self) {} pub fn rename(&self) {} }\n",
        )
        .expect("write session source");
        std::fs::write(
            agent_source.join("cli.rs"),
            "pub enum ExtensionCommand { Status, Enable, Disable }\n",
        )
        .expect("write cli source");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            dir.path().join("docs").join("gap").join("neo-agent.md"),
            "Keep quickstart scoped until interactive mode is no longer placeholder-level.\n",
        )
        .expect("write gap doc");
        std::fs::write(
            dir.path().join("docs").join("quickstart.md"),
            concat!(
                "`--docs` scans for production docs\n",
                "that point at fake or placeholder provider paths.\n",
                "It also scans for fake/local/placeholder production guidance.\n",
                "It rejects stale text such as \"no MCP adapter is wired\" once symbols exist.\n",
                "It rejects \"extension lifecycle unavailable\" once lifecycle commands exist.\n",
                "It rejects \"session branching and naming are future work\" once fork exists.\n",
            ),
        )
        .expect("write quickstart");
        std::fs::write(
            dir.path().join("docs").join("mcp.md"),
            "A compile-time Rust stub should only be added after adjacent modules are stable.\n",
        )
        .expect("write mcp");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parity_validation_rejects_oauth_and_hosted_feature_claims() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("quickstart.md"),
            concat!(
                "Neo supports OAuth browser login for hosted provider accounts.\n",
                "Managed hosted collaboration is available through Pi-compatible share rooms.\n",
            ),
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/quickstart.md:1 contains hosted/OAuth overclaim: Neo supports OAuth browser login for hosted provider accounts.".to_string(),
                "docs/quickstart.md:2 contains hosted/OAuth overclaim: Managed hosted collaboration is available through Pi-compatible share rooms.".to_string(),
            ]
        );
    }

    #[test]
    fn parity_validation_allows_oauth_and_hosted_gap_language() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("providers.md"),
            "OAuth browser login and managed hosted collaboration remain gaps; use a user-run self-hosted neo-cloud instead.\n",
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parity_validation_rejects_self_hosted_claims_without_cloud_code() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("sessions.md"),
            "Self-hosted cloud session sync is implemented and ready to use.\n",
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/sessions.md:1 contains unbacked self-hosted/local-first claim: Self-hosted cloud session sync is implemented and ready to use.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_allows_self_hosted_claims_when_cloud_code_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("crates").join("neo-cloud")).expect("cloud dir");
        std::fs::write(
            dir.path()
                .join("crates")
                .join("neo-cloud")
                .join("Cargo.toml"),
            "[package]\nname = \"neo-cloud\"\n",
        )
        .expect("cloud manifest");
        std::fs::create_dir_all(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("sessions.md"),
            "Self-hosted cloud session sync is implemented against user-run neo-cloud.\n",
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parity_validation_rejects_package_root_trust_claims_for_self_signing() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("packages.md"),
            "Package signatures establish publisher/root trust for marketplace installs.\n",
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/packages.md:1 contains package trust overclaim: Package signatures establish publisher/root trust for marketplace installs.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_allows_manifest_self_sign_trust_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("packages.md"),
            "Current package trust is manifest self-sign only, not a publisher/root trust chain.\n",
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parity_validation_rejects_image_runtime_detection_claims_from_encoder_symbols() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tui_src = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&tui_src).expect("tui source dir");
        std::fs::write(
            tui_src.join("image.rs"),
            "pub fn encode_kitty_graphics() {} pub fn encode_iterm2_inline_image() {}\n",
        )
        .expect("write image source");
        std::fs::create_dir_all(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("quickstart.md"),
            "Neo auto-detects terminal image protocol support at runtime.\n",
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/quickstart.md:1 contains image runtime-detection overclaim: Neo auto-detects terminal image protocol support at runtime.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_allows_conditional_image_runtime_detection_guardrail() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("plan.md"),
            "Do not claim terminal image auto-detection unless runtime protocol negotiation is implemented and tested.\n",
        )
        .expect("write docs");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parity_validation_rejects_stale_mcp_adapter_gap_after_adapter_symbol_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_dir = dir
            .path()
            .join("crates")
            .join("agent-core")
            .join("src")
            .join("tools");
        std::fs::create_dir_all(&source_dir).expect("mcp source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            source_dir.join("mcp.rs"),
            "pub trait McpToolAdapter {}\npub struct McpToolProvider;\n",
        )
        .expect("write mcp source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("INDEX.md"),
            "No MCP client adapter is wired into neo-agent-core yet.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/INDEX.md:1 contains stale MCP adapter gap claim: No MCP client adapter is wired into neo-agent-core yet.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_mcp_process_gap_after_stdio_adapter_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_dir = dir
            .path()
            .join("crates")
            .join("agent-core")
            .join("src")
            .join("tools");
        std::fs::create_dir_all(&source_dir).expect("mcp source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            source_dir.join("mcp.rs"),
            concat!(
                "pub trait McpToolAdapter {}\n",
                "pub struct McpToolProvider;\n",
                "pub struct McpStdioToolAdapter;\n",
                "const METHODS: &[&str] = &[\"tools/list\", \"tools/call\"];\n",
            ),
        )
        .expect("write mcp source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("INDEX.md"),
            "Neo still does not yet spawn external MCP server processes.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/INDEX.md:1 contains stale MCP process adapter gap claim: Neo still does not yet spawn external MCP server processes.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_http_mcp_json_subscribe_gap_after_event_reader_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_dir = dir
            .path()
            .join("crates")
            .join("agent-core")
            .join("src")
            .join("tools");
        std::fs::create_dir_all(&source_dir).expect("mcp source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            source_dir.join("mcp.rs"),
            concat!(
                "impl McpHttpToolAdapter {\n",
                "  async fn start_resource_event_reader(&self) {}\n",
                "}\n",
            ),
        )
        .expect("write mcp source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("INDEX.md"),
            "HTTP MCP JSON subscribe ACK cannot receive resource updates yet.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/INDEX.md:1 contains stale HTTP MCP JSON subscribe event gap claim: HTTP MCP JSON subscribe ACK cannot receive resource updates yet.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_mcp_event_stream_url_gap_after_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_dir = dir
            .path()
            .join("crates")
            .join("agent-core")
            .join("src")
            .join("tools");
        std::fs::create_dir_all(&source_dir).expect("mcp source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            source_dir.join("mcp.rs"),
            concat!(
                "fn start_resource_event_reader() {}\n",
                "fn resource_event_stream_url() {",
                " let _ = \"eventStreamUrl\";",
                " let _ = \"event_stream_url\";",
                " let _ = \"event_url\";",
                " }\n",
            ),
        )
        .expect("write mcp source");
        std::fs::write(
            dir.path()
                .join("docs")
                .join("gap")
                .join("neo-agent-core.md"),
            "MCP JSON subscribe ACK cannot provide alternate event-channel URLs yet.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/neo-agent-core.md:1 contains stale MCP subscribe event URL gap claim: MCP JSON subscribe ACK cannot provide alternate event-channel URLs yet.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_extension_lifecycle_gap_after_commands_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_dir = dir.path().join("crates").join("neo-agent").join("src");
        std::fs::create_dir_all(&source_dir).expect("agent source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            source_dir.join("cli.rs"),
            "pub enum ExtensionCommand { Status, Enable, Disable }\n",
        )
        .expect("write cli source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("neo-agent.md"),
            "Do not document extension lifecycle management as available Neo features yet.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/neo-agent.md:1 contains stale extension lifecycle gap claim: Do not document extension lifecycle management as available Neo features yet.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_session_branching_gap_after_fork_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_dir = dir
            .path()
            .join("crates")
            .join("agent-core")
            .join("src")
            .join("session");
        std::fs::create_dir_all(&source_dir).expect("session source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            source_dir.join("mod.rs"),
            "impl SessionMetadataStore { pub fn fork(&self) {} pub fn rename(&self) {} }\n",
        )
        .expect("write session source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("INDEX.md"),
            "Session tree branching and naming remain pi-inspired future work.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/INDEX.md:1 contains stale session branching gap claim: Session tree branching and naming remain pi-inspired future work.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_live_session_picker_gap_after_interactive_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let interactive_dir = dir
            .path()
            .join("crates")
            .join("neo-agent")
            .join("src")
            .join("modes");
        let input_dir = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&interactive_dir).expect("interactive source dir");
        std::fs::create_dir_all(&input_dir).expect("input source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            interactive_dir.join("interactive.rs"),
            "fn open_session_picker() {} fn load_selected_session() {} fn session_catalog_for_config() {}\n",
        )
        .expect("write interactive source");
        std::fs::write(
            input_dir.join("input.rs"),
            "enum Key { SessionPickerOpen }\n",
        )
        .expect("write input source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("neo-agent.md"),
            "The live session picker remains future work.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/neo-agent.md:1 contains stale live session picker gap claim: The live session picker remains future work.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_live_model_picker_gap_after_interactive_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let interactive_dir = dir
            .path()
            .join("crates")
            .join("neo-agent")
            .join("src")
            .join("modes");
        let input_dir = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&interactive_dir).expect("interactive source dir");
        std::fs::create_dir_all(&input_dir).expect("input source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            interactive_dir.join("interactive.rs"),
            "fn open_model_picker() {} fn apply_selected_model() {} fn model_catalog_for_config() {}\n",
        )
        .expect("write interactive source");
        std::fs::write(input_dir.join("input.rs"), "enum Key { ModelPickerOpen }\n")
            .expect("write input source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("neo-agent.md"),
            "The live model picker is still missing.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/neo-agent.md:1 contains stale live model picker gap claim: The live model picker is still missing.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_fork_before_continue_gap_after_session_fork_ui_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let interactive_dir = dir
            .path()
            .join("crates")
            .join("neo-agent")
            .join("src")
            .join("modes");
        let input_dir = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&interactive_dir).expect("interactive source dir");
        std::fs::create_dir_all(&input_dir).expect("input source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            interactive_dir.join("interactive.rs"),
            "fn fork_selected_session() {} fn fork_session_transcript() {}\n",
        )
        .expect("write interactive source");
        std::fs::write(
            input_dir.join("input.rs"),
            "enum Action { SessionFork } const ID: &str = \"tui.session.fork\";\n",
        )
        .expect("write input source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("neo-agent.md"),
            "Explicit fork-before-continue controls are still a gap beyond local JSONL append.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/neo-agent.md:1 contains stale interactive session fork gap claim: Explicit fork-before-continue controls are still a gap beyond local JSONL append.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_runtime_hook_queue_gap_after_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_dir = dir.path().join("crates").join("agent-core").join("src");
        std::fs::create_dir_all(&source_dir).expect("agent-core source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            source_dir.join("runtime.rs"),
            concat!(
                "impl AgentConfig {\n",
                "  pub fn with_before_tool_call(&self) {}\n",
                "  pub fn with_after_tool_call(&self) {}\n",
                "  pub fn with_queue_modes(&self) {}\n",
                "}\n",
                "impl AgentContext { pub fn queue_steering_message(&self) {} }\n",
            ),
        )
        .expect("write runtime source");
        std::fs::write(
            dir.path()
                .join("docs")
                .join("gap")
                .join("neo-agent-core.md"),
            "Add hook/steering docs only when the runtime exposes those APIs.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/neo-agent-core.md:1 contains stale runtime hook/queue gap claim: Add hook/steering docs only when the runtime exposes those APIs.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_tui_diff_gap_after_renderer_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tui_src = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&tui_src).expect("tui source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            tui_src.join("app.rs"),
            "enum TranscriptLine { DiffAdded, DiffRemoved }\n",
        )
        .expect("write tui app source");
        std::fs::write(
            tui_src.join("components.rs"),
            "fn transcript_line_style() {}\n",
        )
        .expect("write tui components source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("tui.md"),
            "Keep TUI docs scoped until diff rendering lands.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/tui.md:1 contains stale TUI unified diff renderer gap claim: Keep TUI docs scoped until diff rendering lands.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_allows_advanced_diff_affordance_gaps_after_basic_renderer_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tui_src = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&tui_src).expect("tui source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            tui_src.join("app.rs"),
            "enum TranscriptLine { DiffAdded, DiffRemoved }\n",
        )
        .expect("write tui app source");
        std::fs::write(
            tui_src.join("components.rs"),
            "fn transcript_line_style() {}\n",
        )
        .expect("write tui components source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("tui.md"),
            "Advanced diff affordances remain not implemented.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(errors, Vec::<String>::new());
    }

    #[test]
    fn parity_validation_rejects_stale_tui_paste_buffering_gap_after_input_parser_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tui_src = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&tui_src).expect("tui source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            tui_src.join("input.rs"),
            concat!(
                "struct InputParser;\n",
                "const BRACKETED_PASTE_START: &[u8] = b\"\\x1b[200~\";\n",
                "const BRACKETED_PASTE_END: &[u8] = b\"\\x1b[201~\";\n",
            ),
        )
        .expect("write tui input source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("tui.md"),
            "Keep TUI docs scoped until stdin buffering lands.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/tui.md:1 contains stale TUI paste buffering gap claim: Keep TUI docs scoped until stdin buffering lands.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_tui_transcript_selection_copy_gap_after_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tui_src = dir.path().join("crates").join("tui").join("src");
        let interactive_dir = dir
            .path()
            .join("crates")
            .join("neo-agent")
            .join("src")
            .join("modes");
        std::fs::create_dir_all(&tui_src).expect("tui source dir");
        std::fs::create_dir_all(&interactive_dir).expect("interactive source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            tui_src.join("app.rs"),
            concat!(
                "struct TranscriptSelection;\n",
                "impl ChatTranscript { fn copy_selection(&self) {} }\n",
                "impl NeoTuiApp { fn copy_selected_transcript_text(&self) {} }\n",
            ),
        )
        .expect("write tui app source");
        std::fs::write(
            tui_src.join("components.rs"),
            "impl TranscriptWidget<'_> { fn with_selection(&self) {} }\n",
        )
        .expect("write tui components source");
        std::fs::write(
            tui_src.join("input.rs"),
            concat!(
                "enum Action { TranscriptSelectionStart, TranscriptCopySelection }\n",
                "const ID: &str = \"tui.transcript.copySelection\";\n",
            ),
        )
        .expect("write tui input source");
        std::fs::write(
            interactive_dir.join("interactive.rs"),
            "fn copy_transcript_selection_to_clipboard() {}\n",
        )
        .expect("write interactive source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("tui.md"),
            "Selected transcript-region copy remains future work.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/tui.md:1 contains stale TUI transcript selection copy gap claim: Selected transcript-region copy remains future work.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_terminal_image_protocol_gap_after_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tui_src = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&tui_src).expect("tui source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            tui_src.join("image.rs"),
            concat!(
                "pub enum ImageProtocolError {}\n",
                "pub struct KittyGraphicsOptions;\n",
                "pub fn encode_kitty_graphics() {}\n",
                "pub fn encode_iterm2_inline_image() {}\n",
            ),
        )
        .expect("write tui image source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("tui.md"),
            "Terminal image protocols remain not implemented.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/tui.md:1 contains stale terminal image protocol gap claim: Terminal image protocols remain not implemented.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_allows_specific_unimplemented_image_protocol_gaps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tui_src = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&tui_src).expect("tui source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            tui_src.join("image.rs"),
            concat!(
                "pub enum ImageProtocolError {}\n",
                "pub struct KittyGraphicsOptions;\n",
                "pub fn encode_kitty_graphics() {}\n",
            ),
        )
        .expect("write tui image source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("tui.md"),
            "Sixel and full renderer integration remain explicit image protocol gaps.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(errors, Vec::<String>::new());
    }

    #[test]
    fn parity_validation_rejects_stale_sixel_gap_after_encoder_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tui_src = dir.path().join("crates").join("tui").join("src");
        std::fs::create_dir_all(&tui_src).expect("tui source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            tui_src.join("image.rs"),
            concat!(
                "pub struct SixelImageOptions;\n",
                "pub struct SixelPaletteColor;\n",
                "pub fn encode_sixel_image() {}\n",
            ),
        )
        .expect("write tui image source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("tui.md"),
            "Sixel output remains not implemented.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/tui.md:1 contains stale Sixel image protocol gap claim: Sixel output remains not implemented.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_session_export_json_gap_after_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent_src = dir.path().join("crates").join("neo-agent").join("src");
        std::fs::create_dir_all(&agent_src).expect("neo-agent source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            agent_src.join("session_commands.rs"),
            concat!(
                "pub async fn export_json() {}\n",
                "pub async fn export_json_artifact() {}\n",
                "const FORMAT: &str = \"neo.session.export_json\";\n",
            ),
        )
        .expect("write session commands source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("neo-agent.md"),
            "Local session export-json remains future work.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/neo-agent.md:1 contains stale session export-json gap claim: Local session export-json remains future work.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_reasoning_replay_control_gap_after_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ai_src = dir.path().join("crates").join("ai").join("src");
        std::fs::create_dir_all(&ai_src).expect("ai source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            ai_src.join("options.rs"),
            "pub struct RequestOptions { pub replay_reasoning: bool }\n",
        )
        .expect("write ai options source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("neo-ai.md"),
            "Thinking off cannot suppress signed reasoning replay yet.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/neo-ai.md:1 contains stale reasoning replay-control gap claim: Thinking off cannot suppress signed reasoning replay yet.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_ai_thinking_gap_after_payload_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let providers_dir = dir
            .path()
            .join("crates")
            .join("ai")
            .join("src")
            .join("providers");
        std::fs::create_dir_all(&providers_dir).expect("ai provider source dir");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("docs gap dir");
        std::fs::write(
            providers_dir.join("anthropic.rs"),
            "fn thinking_budget_tokens() { let _ = \"budget_tokens\"; }\n",
        )
        .expect("write anthropic source");
        std::fs::write(
            providers_dir.join("google.rs"),
            "fn thinking_budget_tokens() { let _ = \"thinkingConfig\"; }\n",
        )
        .expect("write google source");
        std::fs::write(
            dir.path().join("docs").join("gap").join("neo-ai.md"),
            "Add Anthropic and Google thinking controls only after Neo has explicit budget contracts.\n",
        )
        .expect("write gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/neo-ai.md:1 contains stale Anthropic/Google thinking payload gap claim: Add Anthropic and Google thinking controls only after Neo has explicit budget contracts.".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_ai_thinking_translation_gap_after_payload_symbols_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let providers_dir = dir
            .path()
            .join("crates")
            .join("ai")
            .join("src")
            .join("providers");
        std::fs::create_dir_all(&providers_dir).expect("ai provider source dir");
        std::fs::create_dir_all(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            providers_dir.join("anthropic.rs"),
            "fn thinking_budget_tokens() { let _ = \"budget_tokens\"; }\n",
        )
        .expect("write anthropic source");
        std::fs::write(
            providers_dir.join("google.rs"),
            "fn thinking_budget_tokens() { let _ = \"thinkingConfig\"; }\n",
        )
        .expect("write google source");
        std::fs::write(
            dir.path().join("docs").join("providers.md"),
            "Neo intentionally does not translate reasoning effort into Anthropic or Google thinking payloads yet.\n",
        )
        .expect("write provider doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/providers.md:1 contains stale Anthropic/Google thinking payload gap claim: Neo intentionally does not translate reasoning effort into Anthropic or Google thinking payloads yet.".to_string()
            ]
        );
    }

    #[test]
    fn examples_validation_requires_rust_example_harness_for_rust_sources() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rust_examples = dir.path().join("examples").join("rust");
        std::fs::create_dir_all(&rust_examples).expect("rust examples dir");
        std::fs::write(rust_examples.join("tool_schema.rs"), "fn main() {}\n")
            .expect("write rust example");

        let errors = validate_examples(dir.path()).expect("examples validation should run");

        assert_eq!(
            errors,
            vec!["examples/rust contains Rust examples but is missing Cargo.toml".to_string()]
        );
    }

    #[test]
    fn examples_validation_requires_rust_harness_to_cover_each_example_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rust_examples = dir.path().join("examples").join("rust");
        std::fs::create_dir_all(&rust_examples).expect("rust examples dir");
        std::fs::write(rust_examples.join("provider_registry.rs"), "fn main() {}\n")
            .expect("write provider example");
        std::fs::write(rust_examples.join("tool_schema.rs"), "fn main() {}\n")
            .expect("write tool example");
        std::fs::write(
            rust_examples.join("Cargo.toml"),
            concat!(
                "[package]\n",
                "name = \"neo-rust-examples\"\n",
                "version = \"0.0.0\"\n",
                "edition = \"2024\"\n",
                "\n",
                "[[example]]\n",
                "name = \"provider_registry\"\n",
                "path = \"provider_registry.rs\"\n",
            ),
        )
        .expect("write harness manifest");

        let errors = validate_examples(dir.path()).expect("examples validation should run");

        assert_eq!(
            errors,
            vec![
                "examples/rust/Cargo.toml does not declare example target for examples/rust/tool_schema.rs".to_string()
            ]
        );
    }

    #[test]
    fn cli_accepts_release_smoke_and_catalog_check_commands() {
        assert!(matches!(
            Cli::try_parse_from(["xtask", "release-smoke"])
                .expect("release-smoke command should parse")
                .command,
            Some(XtaskCommand::ReleaseSmoke)
        ));

        assert!(matches!(
            Cli::try_parse_from(["xtask", "catalog", "check"])
                .expect("catalog check command should parse")
                .command,
            Some(XtaskCommand::Catalog(CatalogCommand::Check(_)))
        ));
    }

    #[test]
    fn release_smoke_reports_missing_self_hosted_cloud_package() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("crates").join("neo-agent"))
            .expect("neo-agent dir");
        std::fs::write(
            dir.path()
                .join("crates")
                .join("neo-agent")
                .join("Cargo.toml"),
            "[package]\nname = \"neo-agent\"\n",
        )
        .expect("neo-agent manifest");

        let errors = release_smoke_dependency_errors(dir.path()).expect("dependency scan");

        assert_eq!(
            errors,
            vec![
                "missing self-hosted neo-cloud package at crates/neo-cloud/Cargo.toml; land the cloud worker output or set NEO_RELEASE_SMOKE_CLOUD_CMD to an explicit start command".to_string()
            ]
        );
    }

    #[test]
    fn release_smoke_reports_missing_cloud_cli_flow_after_cloud_package_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("crates").join("neo-cloud")).expect("cloud dir");
        std::fs::write(
            dir.path()
                .join("crates")
                .join("neo-cloud")
                .join("Cargo.toml"),
            "[package]\nname = \"neo-cloud\"\n",
        )
        .expect("cloud manifest");
        let cli_dir = dir.path().join("crates").join("neo-agent").join("src");
        std::fs::create_dir_all(&cli_dir).expect("cli dir");
        std::fs::write(cli_dir.join("cli.rs"), "pub enum Command { Models }\n")
            .expect("cli source");

        let errors = release_smoke_dependency_errors(dir.path()).expect("dependency scan");

        assert_eq!(
            errors,
            vec![
                "missing neo-agent cloud CLI smoke flow; expected crates/neo-agent/src/cli.rs to expose `cloud status --api-base <URL>` before release-smoke can exercise neo-cloud".to_string()
            ]
        );
    }

    #[test]
    fn release_smoke_builds_cloud_start_step_with_random_port() {
        let step = release_smoke_cloud_step(49152, None);

        assert_eq!(
            step,
            CommandStep::new(
                "cargo",
                &[
                    "run",
                    "-p",
                    "neo-cloud",
                    "--",
                    "--host",
                    "127.0.0.1",
                    "--port",
                    "49152"
                ]
            )
        );
    }

    #[test]
    fn release_smoke_cli_steps_cover_local_first_release_surface() {
        let steps = release_smoke_cli_steps(49152)
            .into_iter()
            .map(|step| step.display())
            .collect::<Vec<_>>();

        for expected in [
            "cargo run -p neo-agent -- --help",
            "cargo run -p neo-agent -- models list --pricing",
            "cargo run -p neo-agent -- cloud status --api-base http://127.0.0.1:49152",
            "cargo run -p neo-agent -- login cloud --server http://127.0.0.1:49152",
            "cargo run -p neo-agent -- auth status",
            "cargo run -p neo-agent -- config sync status",
            "cargo run -p neo-agent -- config sync push",
            "cargo run -p neo-agent -- config sync pull",
            "cargo run -p neo-agent -- sessions sync status",
            "cargo run -p neo-agent -- sessions share release-smoke --public",
            "cargo run -p neo-agent -- sessions import sh_release_smoke",
            "cargo run -p neo-agent -- resume cs_release_smoke",
            "cargo run -p neo-agent -- extensions search echo",
            "cargo run -p neo-agent -- extensions install echo@0.1.0 --from marketplace",
            "cargo run -p neo-agent -- mcp servers health release-smoke",
            "cargo run -p neo-agent -- mcp servers start release-smoke",
            "cargo run -p neo-agent -- mcp servers stop release-smoke",
            "cargo run -p xtask -- catalog check",
        ] {
            assert!(steps.iter().any(|step| step == expected), "{expected}");
        }
    }

    #[test]
    fn release_smoke_reports_missing_mcp_lifecycle_cli_flow() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("crates").join("neo-cloud")).expect("cloud dir");
        std::fs::write(
            dir.path()
                .join("crates")
                .join("neo-cloud")
                .join("Cargo.toml"),
            "[package]\nname = \"neo-cloud\"\n",
        )
        .expect("cloud manifest");
        let cli_dir = dir.path().join("crates").join("neo-agent").join("src");
        std::fs::create_dir_all(&cli_dir).expect("cli dir");
        std::fs::write(
            cli_dir.join("cli.rs"),
            concat!(
                "pub enum CloudCommand { Status { api_base: String } }\n",
                "pub enum LoginCommand { Cloud }\n",
                "pub enum AuthCommand { Status }\n",
                "pub enum ConfigSyncCommand { Push, Pull, Status }\n",
                "pub enum SessionCommand { Share, Import, Sync }\n",
                "pub enum ModelCommand { List { pricing: bool } }\n",
                "pub enum ExtensionCommand { Search, Install }\n",
                "pub enum PackageSource { Marketplace }\n",
                "pub enum McpCommand { List }\n",
            ),
        )
        .expect("cli source");

        let errors = release_smoke_dependency_errors(dir.path()).expect("dependency scan");

        assert_eq!(
            errors,
            vec![
                "missing neo-agent MCP lifecycle smoke flow; expected crates/neo-agent/src/cli.rs to expose `mcp servers health/start/stop <server-id>` before release-smoke can exercise local MCP lifecycle".to_string()
            ]
        );
    }

    #[test]
    fn catalog_check_validates_generated_catalog_schema_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let generated = dir.path().join("docs").join("generated");
        std::fs::create_dir_all(&generated).expect("generated docs dir");
        std::fs::write(
            generated.join("model-catalog.schema.json"),
            r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"models":{"type":"array"}}}"#,
        )
        .expect("schema");

        let report = validate_catalog_schemas(dir.path(), CatalogRequirement::Optional)
            .expect("catalog check");

        assert_eq!(report.checked, 1);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
    }

    #[test]
    fn catalog_check_rejects_invalid_generated_catalog_schema_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let generated = dir.path().join("docs").join("generated");
        std::fs::create_dir_all(&generated).expect("generated docs dir");
        std::fs::write(
            generated.join("model-catalog.schema.json"),
            r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"array"}"#,
        )
        .expect("schema");

        let report = validate_catalog_schemas(dir.path(), CatalogRequirement::Optional)
            .expect("catalog check");

        assert_eq!(
            report.errors,
            vec![
                "docs/generated/model-catalog.schema.json must be an object schema with `properties`".to_string()
            ]
        );
    }

    #[test]
    fn required_catalog_check_reports_missing_generated_schema() {
        let dir = tempfile::tempdir().expect("tempdir");

        let report = validate_catalog_schemas(dir.path(), CatalogRequirement::Required)
            .expect("catalog check");

        assert_eq!(
            report.errors,
            vec![
                "missing generated model catalog schema; expected one of docs/generated/model-catalog.schema.json, docs/generated/models.schema.json, docs/reference/model-catalog.schema.json, or examples/catalog/model-catalog.schema.json".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_auth_token_leaks_in_docs() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_catalog_schema_fixture(dir.path());
        std::fs::create_dir_all(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("export.md"),
            "Authorization: Bearer sk-live-abcdefghijklmnopqrstuvwxyz123456\n",
        )
        .expect("write docs");

        let errors = validate_parity_gate(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/export.md:1 contains auth token leak: Authorization: Bearer sk-live-abcdefghijklmnopqrstuvwxyz123456".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_allows_auth_token_placeholders_in_docs() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_catalog_schema_fixture(dir.path());
        std::fs::create_dir_all(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("providers.md"),
            "Authorization: Bearer $NEO_API_KEY\napi_key_env = \"OPENAI_API_KEY\"\n",
        )
        .expect("write docs");

        let errors = validate_parity_gate(dir.path()).expect("parity validation should run");

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parity_validation_does_not_treat_source_identifiers_as_auth_token_leaks() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_catalog_schema_fixture(dir.path());
        let source_dir = dir.path().join("crates").join("neo-agent").join("src");
        std::fs::create_dir_all(&source_dir).expect("source dir");
        std::fs::write(
            source_dir.join("main.rs"),
            concat!(
                "let api_key = api_key_from_provider(provider, &env);\n",
                "let captured_token = StdArc::new(std::sync::Mutex::new(None));\n",
            ),
        )
        .expect("source");

        let errors = validate_parity_gate(dir.path()).expect("parity validation should run");

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parity_validation_rejects_cloud_api_schema_links_when_generated_target_is_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_catalog_schema_fixture(dir.path());
        std::fs::create_dir_all(dir.path().join("docs")).expect("docs dir");
        std::fs::write(
            dir.path().join("docs").join("cloud.md"),
            "[Cloud OpenAPI schema](./generated/cloud-api.openapi.json)\n",
        )
        .expect("write docs");

        let errors = validate_parity_gate(dir.path()).expect("parity validation should run");

        assert!(errors.iter().any(|error| error
            == "docs/cloud.md links to missing local file docs/generated/cloud-api.openapi.json"));
        assert!(errors.iter().any(|error| error
            == "docs/cloud.md:1 links to missing generated cloud API schema docs/generated/cloud-api.openapi.json"));
    }

    #[test]
    fn parity_validation_rejects_private_package_signature_fixture_material() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_catalog_schema_fixture(dir.path());
        let fixtures = dir.path().join("examples").join("packages");
        std::fs::create_dir_all(&fixtures).expect("fixtures dir");
        std::fs::write(
            fixtures.join("signature-fixture.json"),
            r#"{"privateKey":"-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----"}"#,
        )
        .expect("fixture");

        let errors = validate_parity_gate(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "examples/packages/signature-fixture.json:1 contains private package signature material: {\"privateKey\":\"-----BEGIN PRIVATE KEY-----\\nabc\\n-----END PRIVATE KEY-----\"}".to_string()
            ]
        );
    }

    #[test]
    fn parity_validation_rejects_stale_cloud_gap_after_cloud_package_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cloud_dir = dir.path().join("crates").join("neo-cloud").join("src");
        std::fs::create_dir_all(&cloud_dir).expect("cloud source dir");
        std::fs::write(
            cloud_dir.join("main.rs"),
            "fn main() { let _ = \"neo_cloud_self_hosted\"; }\n",
        )
        .expect("cloud main");
        std::fs::write(
            dir.path()
                .join("crates")
                .join("neo-cloud")
                .join("Cargo.toml"),
            "[package]\nname = \"neo-cloud\"\n",
        )
        .expect("cloud manifest");
        std::fs::create_dir_all(dir.path().join("docs").join("gap")).expect("gap docs dir");
        std::fs::write(
            dir.path().join("docs").join("gap").join("xtask.md"),
            "Self-hosted neo-cloud smoke remains future work.\n",
        )
        .expect("gap doc");

        let errors = validate_docs_parity(dir.path()).expect("parity validation should run");

        assert_eq!(
            errors,
            vec![
                "docs/gap/xtask.md:1 contains stale self-hosted cloud smoke gap claim: Self-hosted neo-cloud smoke remains future work.".to_string()
            ]
        );
    }

    fn write_catalog_schema_fixture(root: &Path) {
        let generated = root.join("docs").join("generated");
        std::fs::create_dir_all(&generated).expect("generated docs dir");
        std::fs::write(
            generated.join("model-catalog.schema.json"),
            r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"models":{"type":"array"}}}"#,
        )
        .expect("schema");
    }
}
