//! Workspace change entry (top bar) at the product boundary: a real
//! `neo webui --no-open` service against a real temporary repository. The
//! structured summary comes from the shared git collector (per-file numstat
//! counts and untracked line counts), and opaque change references are
//! validated host-side: forged, outside, absolute, shell-like or stale
//! references all get the same 404 without echoing paths.

use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine as _;
use serde_json::Value;

use super::http;
use super::session_env::start_env;

fn git(project: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Commit `tracked.txt` (two lines) and `unchanged.txt`, then modify
/// `tracked.txt` (+1 line) and add the untracked two-line `notes.txt`;
/// returns the current branch name.
fn prepare_repository(project: &Path) -> String {
    git(project, &["init", "-q"]);
    git(project, &["config", "user.email", "webui@test"]);
    git(project, &["config", "user.name", "webui"]);
    std::fs::write(project.join("tracked.txt"), "a\nb\n").expect("write tracked");
    std::fs::write(project.join("unchanged.txt"), "k\n").expect("write unchanged");
    git(project, &["add", "tracked.txt", "unchanged.txt"]);
    git(project, &["commit", "-qm", "init"]);
    std::fs::write(project.join("tracked.txt"), "a\nb\nc\n").expect("modify tracked");
    std::fs::write(project.join("notes.txt"), "n1\nn2\n").expect("write untracked");
    let branch = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .expect("git branch");
    String::from_utf8_lossy(&branch.stdout).trim().to_owned()
}

fn encode_reference(raw: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}

async fn changes_json(port: u16, cookie: &str) -> Value {
    let response = http::get(port, cookie, "/api/workspace/changes").await;
    assert_eq!(response.status, 200, "{}", response.body);
    serde_json::from_str(&response.body).expect("changes json")
}

fn change_id_of<'a>(changes: &'a Value, path: &str) -> &'a str {
    changes["changes"]
        .as_array()
        .expect("changes array")
        .iter()
        .find(|change| change["path"] == path)
        .unwrap_or_else(|| panic!("change for {path}: {changes}"))["change_id"]
        .as_str()
        .expect("change id")
}

#[tokio::test]
async fn workspace_status_reuses_the_shared_git_collector() {
    let project = tempfile::tempdir().expect("project tempdir");
    let branch = prepare_repository(project.path());
    let project_path: PathBuf = project.path().to_path_buf();
    let (test_env, _provider) = start_env(project, Vec::new()).await;

    let response = http::get(
        test_env.webui.port,
        &test_env.cookie,
        "/api/workspace/changes",
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body);
    // The change surface never leaks the absolute workspace path.
    assert!(
        !response.body.contains(&*project_path.to_string_lossy()),
        "{}",
        response.body
    );
    let body: Value = serde_json::from_str(&response.body).expect("changes json");
    assert_eq!(body["branch"].as_str(), Some(branch.as_str()));
    assert_eq!(body["dirty"].as_bool(), Some(true));

    let changes = body["changes"].as_array().expect("changes array");
    // Per-file counts come from the collector's numstat mapping; the
    // committed-but-unchanged file is absent.
    let tracked = changes
        .iter()
        .find(|change| change["path"] == "tracked.txt")
        .expect("tracked change");
    assert_eq!(tracked["status"], "modified");
    assert_eq!(tracked["added"], 1);
    assert_eq!(tracked["deleted"], 0);
    assert!(
        tracked["change_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    // Untracked counts come from the collector's safe line counting.
    let notes = changes
        .iter()
        .find(|change| change["path"] == "notes.txt")
        .expect("untracked change");
    assert_eq!(notes["status"], "untracked");
    assert_eq!(notes["added"], 2);
    assert_eq!(notes["deleted"], 0);
    assert!(
        changes
            .iter()
            .all(|change| change["path"] != "unchanged.txt")
    );

    // Query parameters are a fixed whitelist (empty here): misspelled keys
    // are rejected instead of silently ignored.
    let rejected = http::get(
        test_env.webui.port,
        &test_env.cookie,
        "/api/workspace/changes?bogus=1",
    )
    .await;
    assert_eq!(rejected.status, 400, "{}", rejected.body);
    assert!(
        rejected.body.contains("invalid_request"),
        "{}",
        rejected.body
    );
}

#[tokio::test]
async fn workspace_change_detail_rejects_forged_or_outside_reference() {
    let project = tempfile::tempdir().expect("project tempdir");
    prepare_repository(project.path());
    let project_path: PathBuf = project.path().to_path_buf();
    let (test_env, _provider) = start_env(project, Vec::new()).await;
    let port = test_env.webui.port;
    let cookie = &test_env.cookie;

    let summary = changes_json(port, cookie).await;

    // The legitimate reference returns the bounded unified-diff preview.
    let change_id = change_id_of(&summary, "tracked.txt");
    let detail = http::get(port, cookie, &format!("/api/workspace/changes/{change_id}")).await;
    assert_eq!(detail.status, 200, "{}", detail.body);
    let detail_json: Value = serde_json::from_str(&detail.body).expect("detail json");
    assert_eq!(detail_json["change_id"], change_id);
    assert_eq!(detail_json["path"], "tracked.txt");
    assert_eq!(detail_json["status"], "modified");
    assert_eq!(detail_json["truncated"].as_bool(), Some(false));
    let diff = detail_json["diff"].as_str().expect("diff text");
    assert!(diff.contains("@@"), "{diff}");
    assert!(diff.contains("+c"), "{diff}");

    // Untracked content gets the synthesized new-file preview.
    let notes_id = change_id_of(&summary, "notes.txt");
    let notes = http::get(port, cookie, &format!("/api/workspace/changes/{notes_id}")).await;
    assert_eq!(notes.status, 200, "{}", notes.body);
    let notes_json: Value = serde_json::from_str(&notes.body).expect("notes detail json");
    let notes_diff = notes_json["diff"].as_str().expect("notes diff");
    assert!(notes_diff.contains("+n1"), "{notes_diff}");

    // Forged, outside, absolute, shell-like and stale-but-valid references
    // all get one uniform 404 that never echoes paths.
    let absolute = encode_reference(
        project_path
            .join("tracked.txt")
            .to_string_lossy()
            .as_bytes(),
    );
    for reference in [
        absolute,
        encode_reference(b"../outside.txt"),
        encode_reference(b"tracked.txt; rm -rf x"),
        encode_reference(b"unchanged.txt"),
        "!!!not-base64!!!".to_string(),
        String::new(),
    ] {
        let response =
            http::get(port, cookie, &format!("/api/workspace/changes/{reference}")).await;
        assert_eq!(response.status, 404, "{reference}: {}", response.body);
        assert!(
            response.body.contains("not_found"),
            "{reference}: {}",
            response.body
        );
        assert!(
            !response.body.contains(&*project_path.to_string_lossy()),
            "{reference}: {}",
            response.body
        );
    }
}

/// The workspace/branch picker API at the product boundary: branches list the
/// real repository refs, checkout switches (create + checkout for new
/// branches), and the reply carries the refreshed workspace group so the
/// browser updates the branch label in place.
#[tokio::test]
async fn branch_picker_lists_and_checks_out_workspace_branches() {
    let project = tempfile::tempdir().expect("project tempdir");
    prepare_repository(project.path());
    git(project.path(), &["branch", "feature"]);
    let project_path: PathBuf = project.path().to_path_buf();
    let (test_env, _provider) = start_env(project, Vec::new()).await;
    let port = test_env.webui.port;
    let cookie = &test_env.cookie;

    // The current workspace is known to the service; discover its id through
    // the add route (re-adding a known directory is idempotent and returns
    // the group with its id).
    let added = http::post_json(
        port,
        cookie,
        "/api/workspaces",
        &serde_json::json!({
            "path": project_path.to_string_lossy()
        }),
    )
    .await;
    assert_eq!(added.status, 201, "{}", added.body);
    let workspace_id = serde_json::from_str::<Value>(&added.body).expect("group json")["id"]
        .as_str()
        .expect("workspace id")
        .to_owned();

    let listed = http::get(
        port,
        cookie,
        &format!("/api/workspaces/{workspace_id}/branches"),
    )
    .await;
    assert_eq!(listed.status, 200, "{}", listed.body);
    let branches: Value = serde_json::from_str(&listed.body).expect("branches json");
    let names: Vec<String> = branches["branches"]
        .as_array()
        .expect("branch array")
        .iter()
        .map(|name| name.as_str().expect("branch name").to_owned())
        .collect();
    assert!(names.contains(&"feature".to_owned()), "{names:?}");
    assert!(
        branches["current"]
            .as_str()
            .is_some_and(|current| names.contains(&current.to_owned())),
        "{}",
        listed.body
    );

    // Check out the existing feature branch; the reply is the refreshed group.
    let checked_out = http::post_json(
        port,
        cookie,
        &format!("/api/workspaces/{workspace_id}/checkout"),
        &serde_json::json!({ "name": "feature", "create": false }),
    )
    .await;
    assert_eq!(checked_out.status, 200, "{}", checked_out.body);
    let group: Value = serde_json::from_str(&checked_out.body).expect("group json");
    assert_eq!(group["id"], workspace_id.as_str());
    assert_eq!(group["branch"], "feature");

    // Create + check out a brand-new branch from the picker footer.
    let created = http::post_json(
        port,
        cookie,
        &format!("/api/workspaces/{workspace_id}/checkout"),
        &serde_json::json!({ "name": "topic/new-work", "create": true }),
    )
    .await;
    assert_eq!(created.status, 200, "{}", created.body);
    assert_eq!(
        serde_json::from_str::<Value>(&created.body).expect("group json")["branch"],
        "topic/new-work"
    );

    // Option-like and traversal names never reach git.
    for hostile in ["-evil", "../escape", "a b"] {
        let rejected = http::post_json(
            port,
            cookie,
            &format!("/api/workspaces/{workspace_id}/checkout"),
            &serde_json::json!({ "name": hostile, "create": true }),
        )
        .await;
        assert_eq!(rejected.status, 400, "{hostile}: {}", rejected.body);
    }
}
