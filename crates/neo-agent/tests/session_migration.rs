use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const TRANSCRIPT: &str = "{\"kind\":\"neo.session.metadata\",\"format\":\"neo.session.jsonl\",\"schema_version\":1,\"created_at\":\"0\"}\n";

fn python_command() -> Command {
    if let Some(python) = env::var_os("PYTHON").filter(|python| !python.is_empty()) {
        assert!(
            command_succeeds(&python),
            "PYTHON is set but is not runnable: {}",
            python.to_string_lossy()
        );
        return Command::new(python);
    }

    for candidate in ["python3", "python"] {
        if command_succeeds(OsStr::new(candidate)) {
            return Command::new(candidate);
        }
    }

    panic!("no Python interpreter found; set PYTHON or install python3/python");
}

fn command_succeeds(program: &OsStr) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn script_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("scripts")
        .join("migrate_sessions_to_agent_layout.py")
}

fn create_session(neo_home: &Path, id: &str) -> PathBuf {
    let session_dir = neo_home
        .join("sessions")
        .join("wd_neo_000000000000")
        .join(id);
    fs::create_dir_all(&session_dir).expect("mkdir session");
    fs::write(session_dir.join("transcript.jsonl"), TRANSCRIPT).expect("write transcript");
    session_dir
}

fn main_wire_path(session_dir: &Path) -> PathBuf {
    session_dir.join("agents").join("main").join("wire.jsonl")
}

fn migration_command(neo_home: &Path) -> Command {
    let mut command = python_command();
    command.arg(script_path()).arg("--neo-home").arg(neo_home);
    command
}

#[test]
fn migration_script_moves_transcript_to_main_wire_and_writes_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let neo_home = temp.path().join(".neo");
    let session_dir = create_session(&neo_home, "session_00000000-0000-0000-0000-000000000001");

    let output = migration_command(&neo_home)
        .arg("--apply")
        .arg("--no-backup")
        .output()
        .expect("run migration");

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let wire_path = main_wire_path(&session_dir);
    assert!(wire_path.is_file());
    assert_eq!(
        fs::read_to_string(wire_path).expect("read wire"),
        TRANSCRIPT
    );
    let state: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(session_dir.join("state.json")).expect("read state"),
    )
    .expect("parse state");
    assert_eq!(state["schema_version"], 1);
    assert_eq!(state["agents"]["main"]["kind"], "main");
    assert_eq!(state["agents"]["main"]["record_dir"], "agents/main");
    assert!(!session_dir.join("transcript.jsonl").exists());
}

#[test]
fn migration_script_dry_run_leaves_transcript_in_place() {
    let temp = tempfile::tempdir().expect("tempdir");
    let neo_home = temp.path().join(".neo");
    let session_dir = create_session(&neo_home, "session_00000000-0000-0000-0000-000000000002");

    let output = migration_command(&neo_home)
        .arg("--dry-run")
        .output()
        .expect("run migration");

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("would-migrate"),
        "stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(session_dir.join("transcript.jsonl").is_file());
    assert!(!main_wire_path(&session_dir).exists());
}

#[test]
fn migration_script_preserves_existing_backup_and_writes_unique_backup() {
    let temp = tempfile::tempdir().expect("tempdir");
    let neo_home = temp.path().join(".neo");
    let session_dir = create_session(&neo_home, "session_00000000-0000-0000-0000-000000000003");
    let existing_backup = session_dir.with_file_name(format!(
        "{}.pre-agent-layout-backup",
        session_dir
            .file_name()
            .expect("session name")
            .to_string_lossy()
    ));
    fs::create_dir_all(&existing_backup).expect("mkdir backup");
    fs::write(existing_backup.join("sentinel"), "keep").expect("write sentinel");

    let output = migration_command(&neo_home)
        .arg("--apply")
        .output()
        .expect("run migration");

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(existing_backup.join("sentinel")).expect("read sentinel"),
        "keep"
    );
    assert!(
        existing_backup
            .with_extension("pre-agent-layout-backup.1")
            .is_dir()
    );
}

#[test]
fn migration_script_repairs_half_migrated_session_when_transcript_remains() {
    let temp = tempfile::tempdir().expect("tempdir");
    let neo_home = temp.path().join(".neo");
    let session_dir = create_session(&neo_home, "session_00000000-0000-0000-0000-000000000004");
    let wire_path = main_wire_path(&session_dir);
    fs::create_dir_all(wire_path.parent().expect("wire parent")).expect("mkdir agent dir");
    fs::write(&wire_path, TRANSCRIPT).expect("write existing wire");

    let output = migration_command(&neo_home)
        .arg("--apply")
        .arg("--no-backup")
        .output()
        .expect("run migration");

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(session_dir.join("state.json").is_file());
    assert!(!session_dir.join("transcript.jsonl").exists());
}

#[test]
fn migration_script_fails_unrecoverable_half_migrated_session() {
    let temp = tempfile::tempdir().expect("tempdir");
    let neo_home = temp.path().join(".neo");
    let session_dir = neo_home
        .join("sessions")
        .join("wd_neo_000000000000")
        .join("session_00000000-0000-0000-0000-000000000005");
    let wire_path = main_wire_path(&session_dir);
    fs::create_dir_all(wire_path.parent().expect("wire parent")).expect("mkdir agent dir");
    fs::write(&wire_path, TRANSCRIPT).expect("write existing wire");

    let output = migration_command(&neo_home)
        .arg("--apply")
        .arg("--no-backup")
        .output()
        .expect("run migration");

    assert!(
        !output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("failed"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!session_dir.join("state.json").exists());
}
