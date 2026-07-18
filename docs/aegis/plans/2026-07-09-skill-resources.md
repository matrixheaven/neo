# Skill Resources Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use aegis:subagent-driven-development (recommended) or aegis:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend Neo skills into light local-first packages with optional `references/`, `scripts/`, and `assets/` resources created through `CreateSkill`.

**Architecture:** Keep `SKILL.md` as the only automatically loaded skill entry point. Add optional resource metadata to `CreateSkill`, validate every resource path before writing, write resource files under the skill root, summarize resources in `ListSkills`, and make discovery skip resource directories inside a skill package. Update built-in authoring skills and docs to use the same resource contract.

**Tech Stack:** Rust 2024, `neo-agent-core` skill discovery and tool code, `schemars` tool schemas, `serde` JSON args, `tokio::fs` tests, existing safe path helpers in `skills_manager.rs`, Markdown docs in `docs/en` and `docs/zh`.

---

## Scope Check

The spec is coherent as one implementation plan. It covers one tool extension (`CreateSkill.resources`), one discovery rule, one display improvement, two built-in skill prompt updates, and documentation. It does not require a new runtime subsystem.

Spec review found no blockers. The spec resolves the key boundaries: resources are text-only in `CreateSkill` v1, `SKILL.md` remains the only automatic context entry point, resource dirs are non-discovery zones under a package root, nested skills use `skills/`, and unmentioned existing resources are preserved on overwrite.

Git policy for this repo is stricter than the generic planning skill: **do not run `git add`, `git commit`, branch, reset, checkout, stash, push, or any other git mutation unless the user explicitly authorizes that exact command. Subagents must never run git mutations.**

## File Structure

- Modify `crates/neo-agent-core/src/tools/skills_manager.rs`
  - Add `CreateSkillResource`.
  - Add resource constants and path/content validation helpers.
  - Extend `CreateSkillArgs` with `resources`.
  - Write resource files after `SKILL.md`.
  - Back up whole skill directories before overwrite.
  - Summarize non-empty resource dirs in `ListSkills`.
  - Add focused tests for tool schema, resource writes, invalid paths, backups, symlink safety, reload, and resource summaries.
- Modify `crates/neo-agent-core/src/skills/discovery.rs`
  - Skip `references/`, `scripts/`, and `assets/` when recursing inside a directory that has its own `SKILL.md`.
- Modify `crates/neo-agent-core/tests/skills.rs`
  - Add integration tests for resource-directory discovery skip and `skills/` child discovery.
- Modify `crates/neo-agent-core/src/skills/builtin/create-skill.md`
  - Replace the current follow-up resource wording with direct `CreateSkill.resources` guidance.
- Modify `crates/neo-agent-core/src/skills/builtin/self-evo.md`
  - Teach self-evo to create resource-backed skills when session history contains durable scripts, references, or assets.
- Modify `crates/neo-agent-core/src/tools/skills_manager.rs` built-in tests
  - Assert both authoring built-ins mention resource-backed creation, `CreateSkill.resources`, `${NEO_SKILL_DIR}`, and `## Verify`.
- Modify `docs/en/customization/skills.md`
  - Document package layout, resource semantics, `${NEO_SKILL_DIR}`, `CreateSkill.resources`, and nested skill/resource discovery rules.
- Modify `docs/zh/customization/skills.md`
  - Keep Chinese docs semantically aligned with English docs.

## Task 1: Add Failing `CreateSkill.resources` Tool Tests

**Files:**
- Modify: `crates/neo-agent-core/src/tools/skills_manager.rs`

- [ ] **Step 1: Add a failing test for creating all resource directories**

Add this test inside the existing `#[cfg(test)] mod tests`:

```rust
#[tokio::test]
async fn create_skill_writes_resource_files() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());

    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "resource-skill",
                "description": "Use when testing resource-backed skills.",
                "body": "# Resource Skill\n\nRead `${NEO_SKILL_DIR}/references/guide.md`.",
                "resources": [
                    {
                        "path": "references/guide.md",
                        "content": "# Guide\n\nUse this reference."
                    },
                    {
                        "path": "scripts/check.py",
                        "content": "print('ok')\n",
                        "executable": true
                    },
                    {
                        "path": "assets/template.md",
                        "content": "Name: {{name}}\n"
                    }
                ]
            }),
        )
        .await
        .expect("execute");

    assert!(!result.is_error);
    let skill_dir = temp.path().join("skills").join("resource-skill");
    assert_eq!(
        fs::read_to_string(skill_dir.join("references").join("guide.md"))
            .await
            .expect("read reference"),
        "# Guide\n\nUse this reference."
    );
    assert_eq!(
        fs::read_to_string(skill_dir.join("scripts").join("check.py"))
            .await
            .expect("read script"),
        "print('ok')\n"
    );
    assert_eq!(
        fs::read_to_string(skill_dir.join("assets").join("template.md"))
            .await
            .expect("read asset"),
        "Name: {{name}}\n"
    );
}
```

- [ ] **Step 2: Run the test and confirm it fails before implementation**

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_writes_resource_files --exact --nocapture
```

Expected: FAIL because `CreateSkillArgs` ignores or rejects the `resources` field and no resource files are written.

- [ ] **Step 3: Add failing resource path validation tests**

Add these tests:

```rust
#[tokio::test]
async fn create_skill_rejects_resource_path_escape() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());

    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "bad-resource",
                "description": "Bad resource",
                "body": "# Bad",
                "resources": [
                    {
                        "path": "references/../escaped.md",
                        "content": "escaped"
                    }
                ]
            }),
        )
        .await
        .expect_err("resource path escapes must fail");

    assert!(error.to_string().contains("invalid resource path"));
    assert!(!temp.path().join("skills").join("escaped.md").exists());
}

#[tokio::test]
async fn create_skill_rejects_resource_outside_canonical_dirs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());

    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "bad-resource",
                "description": "Bad resource",
                "body": "# Bad",
                "resources": [
                    {
                        "path": "docs/guide.md",
                        "content": "guide"
                    }
                ]
            }),
        )
        .await
        .expect_err("unsupported resource dir must fail");

    assert!(error.to_string().contains("references, scripts, or assets"));
}

#[tokio::test]
async fn create_skill_rejects_absolute_resource_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());

    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "bad-resource",
                "description": "Bad resource",
                "body": "# Bad",
                "resources": [
                    {
                        "path": "/tmp/guide.md",
                        "content": "guide"
                    }
                ]
            }),
        )
        .await
        .expect_err("absolute resource path must fail");

    assert!(error.to_string().contains("invalid resource path"));
}

#[tokio::test]
async fn create_skill_rejects_skill_md_as_resource() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());

    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "bad-resource",
                "description": "Bad resource",
                "body": "# Bad",
                "resources": [
                    {
                        "path": "references/SKILL.md",
                        "content": "not a nested skill"
                    }
                ]
            }),
        )
        .await
        .expect_err("SKILL.md resource path must fail");

    assert!(error.to_string().contains("SKILL.md"));
}
```

- [ ] **Step 4: Run one validation test and confirm it fails before implementation**

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_rejects_resource_path_escape --exact --nocapture
```

Expected: FAIL because resource validation does not exist.

## Task 2: Implement `CreateSkillResource` Types and Validation

**Files:**
- Modify: `crates/neo-agent-core/src/tools/skills_manager.rs`

- [ ] **Step 1: Add resource constants and schema type**

Near `CreateSkillArgs`, add:

```rust
const RESOURCE_DIRS: &[&str] = &["references", "scripts", "assets"];
const MAX_RESOURCE_BYTES: usize = 256 * 1024;
const MAX_TOTAL_RESOURCE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateSkillResource {
    /// Relative path under references/, scripts/, or assets/.
    #[schemars(
        description = "Relative resource path under references/, scripts/, or assets/. Absolute paths, '..', and SKILL.md are rejected."
    )]
    pub path: String,
    /// UTF-8 text content for the resource file.
    #[schemars(description = "UTF-8 text content for the resource file.")]
    pub content: String,
    /// Request executable permissions where supported. Intended for scripts/.
    #[serde(default)]
    #[schemars(
        description = "Request executable permissions where supported. Intended for scripts/."
    )]
    pub executable: bool,
}
```

- [ ] **Step 2: Extend `CreateSkillArgs`**

Add this field after `body`:

```rust
/// Optional text resources to write under references/, scripts/, or assets/.
#[serde(default)]
#[schemars(
    description = "Optional text resources to create under references/, scripts/, or assets/."
)]
pub resources: Vec<CreateSkillResource>,
```

- [ ] **Step 3: Add a validated resource helper struct**

Below `CreateSkillFrontmatter`, add:

```rust
#[derive(Debug, Clone)]
struct ValidatedResource {
    relative_path: PathBuf,
    content: String,
    executable: bool,
}
```

- [ ] **Step 4: Add resource validation helpers**

Add these helpers near `validate_skill_name`:

```rust
fn validate_resources(resources: &[CreateSkillResource]) -> Result<Vec<ValidatedResource>, ToolError> {
    let mut total_bytes = 0usize;
    let mut validated = Vec::with_capacity(resources.len());
    for resource in resources {
        let content_bytes = resource.content.len();
        if content_bytes > MAX_RESOURCE_BYTES {
            return Err(invalid_create_skill_input(format!(
                "resource content too large for {:?}: {} bytes exceeds {} bytes",
                resource.path, content_bytes, MAX_RESOURCE_BYTES
            )));
        }
        total_bytes = total_bytes.checked_add(content_bytes).ok_or_else(|| {
            invalid_create_skill_input("total resource content is too large".to_owned())
        })?;
        if total_bytes > MAX_TOTAL_RESOURCE_BYTES {
            return Err(invalid_create_skill_input(format!(
                "total resource content too large: {total_bytes} bytes exceeds {MAX_TOTAL_RESOURCE_BYTES} bytes"
            )));
        }
        let relative_path = validate_resource_path(&resource.path)?;
        validated.push(ValidatedResource {
            relative_path,
            content: resource.content.clone(),
            executable: resource.executable,
        });
    }
    Ok(validated)
}

fn validate_resource_path(raw: &str) -> Result<PathBuf, ToolError> {
    if raw.is_empty() {
        return Err(invalid_resource_path(raw, "path must not be empty"));
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(invalid_resource_path(raw, "path must be relative"));
    }

    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let Some(part) = part.to_str() else {
                    return Err(invalid_resource_path(raw, "path must be valid UTF-8"));
                };
                if part.is_empty() || part == "." || part == ".." {
                    return Err(invalid_resource_path(raw, "path contains an unsafe component"));
                }
                if part.ends_with('.') {
                    return Err(invalid_resource_path(raw, "path component must not end with a dot"));
                }
                let reserved_prefix = part.split('.').next().unwrap_or(part);
                if is_windows_reserved_basename(reserved_prefix) {
                    return Err(invalid_resource_path(raw, "path contains a reserved Windows device name"));
                }
                components.push(part.to_owned());
            }
            Component::CurDir | Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(invalid_resource_path(raw, "path contains an unsafe component"));
            }
        }
    }

    if components.len() < 2 {
        return Err(invalid_resource_path(raw, "path must include a file under a resource directory"));
    }
    if !RESOURCE_DIRS.contains(&components[0].as_str()) {
        return Err(invalid_resource_path(raw, "path must start with references, scripts, or assets"));
    }
    if components.last().is_some_and(|name| name.eq_ignore_ascii_case("SKILL.md")) {
        return Err(invalid_resource_path(raw, "resource path must not target SKILL.md"));
    }

    Ok(components.iter().collect())
}

fn invalid_resource_path(raw: &str, reason: &str) -> ToolError {
    invalid_create_skill_input(format!("invalid resource path {raw:?}: {reason}"))
}
```

- [ ] **Step 5: Validate resources before any filesystem writes**

In `CreateSkillTool::execute`, after `parse_skill_type`, add:

```rust
let resources = validate_resources(&args.resources)?;
```

Keep this before `ensure_safe_directory(&skills_root)?;`.

- [ ] **Step 6: Run validation tests**

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_rejects_resource_path_escape --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_rejects_resource_outside_canonical_dirs --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_rejects_absolute_resource_path --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_rejects_skill_md_as_resource --exact --nocapture
```

Expected: each test passes.

## Task 3: Write Resource Files and Back Up Whole Skill Directories

**Files:**
- Modify: `crates/neo-agent-core/src/tools/skills_manager.rs`

- [ ] **Step 1: Add failing overwrite tests for full-directory backup and preserved resources**

Add:

```rust
#[tokio::test]
async fn create_skill_backs_up_existing_resource_directory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join("skills").join("existing-skill");
    fs::create_dir_all(skill_dir.join("references"))
        .await
        .expect("mkdir references");
    fs::write(skill_dir.join("SKILL.md"), "old skill")
        .await
        .expect("write old skill");
    fs::write(skill_dir.join("references").join("old.md"), "old reference")
        .await
        .expect("write old reference");
    let tool = CreateSkillTool::new(temp.path());

    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "existing-skill",
                "description": "Updated skill",
                "body": "# Updated",
                "resources": [
                    {
                        "path": "references/new.md",
                        "content": "new reference"
                    }
                ]
            }),
        )
        .await
        .expect("execute");

    assert!(!result.is_error);
    let backup_root = temp.path().join("backups").join("skills");
    let backup_skill = std::fs::read_dir(&backup_root)
        .expect("read backups")
        .map(|entry| entry.expect("backup entry").path().join("existing-skill"))
        .find(|path| path.join("SKILL.md").is_file())
        .expect("backup skill dir");
    assert_eq!(
        fs::read_to_string(backup_skill.join("SKILL.md"))
            .await
            .expect("read backup skill"),
        "old skill"
    );
    assert_eq!(
        fs::read_to_string(backup_skill.join("references").join("old.md"))
            .await
            .expect("read backup resource"),
        "old reference"
    );
}

#[tokio::test]
async fn create_skill_preserves_unmentioned_existing_resources() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join("skills").join("existing-skill");
    fs::create_dir_all(skill_dir.join("references"))
        .await
        .expect("mkdir references");
    fs::write(skill_dir.join("SKILL.md"), "old skill")
        .await
        .expect("write old skill");
    fs::write(skill_dir.join("references").join("keep.md"), "keep me")
        .await
        .expect("write kept reference");
    let tool = CreateSkillTool::new(temp.path());

    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "existing-skill",
                "description": "Updated skill",
                "body": "# Updated"
            }),
        )
        .await
        .expect("execute");

    assert!(!result.is_error);
    assert_eq!(
        fs::read_to_string(skill_dir.join("references").join("keep.md"))
            .await
            .expect("read kept reference"),
        "keep me"
    );
}
```

- [ ] **Step 2: Run one overwrite test and confirm it fails before implementation**

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_backs_up_existing_resource_directory --exact --nocapture
```

Expected: FAIL because only `SKILL.md` is backed up.

- [ ] **Step 3: Replace single-file backup with directory backup**

In `CreateSkillTool::execute`, replace the existing `if path.exists()` backup block with a skill-directory backup:

```rust
if skill_dir.exists() {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let backup_root = user_home.join("backups").join("skills");
    ensure_safe_directory(&backup_root)?;
    let timestamp_dir = ensure_safe_child_directory(&backup_root, Path::new(&format!("{timestamp}")))
        .map_err(ToolError::Io)?;
    let backup_dir = timestamp_dir.join(&args.name);
    if let Err(error) = copy_dir(&skill_dir, &backup_dir).await {
        let _ = fs::remove_dir_all(&backup_dir).await;
        return Err(ToolError::Io(error));
    }
}
```

Keep `reject_reparse_or_symlink_if_present(&path)` before writing `SKILL.md`.

- [ ] **Step 4: Add resource-writing helpers**

Add:

```rust
fn write_resource_file(skill_dir: &Path, resource: &ValidatedResource) -> io::Result<()> {
    let path = skill_dir.join(&resource.relative_path);
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("resource path has no parent directory: {}", path.display()),
        )
    })?;
    let relative_parent = parent.strip_prefix(skill_dir).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("resource parent escapes skill directory: {}", parent.display()),
        )
    })?;
    ensure_safe_child_directory(skill_dir, relative_parent)?;
    reject_reparse_or_symlink_if_present(&path)?;
    match stdfs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("resource target is a directory: {}", path.display()),
            ));
        }
        Ok(_) | Err(_) => {}
    }
    write_file_atomic(&path, resource.content.as_bytes())?;
    apply_resource_executable(&path, resource.executable)
}

#[cfg(unix)]
fn apply_resource_executable(path: &Path, executable: bool) -> io::Result<()> {
    if !executable {
        return Ok(());
    }
    use std::os::unix::fs::PermissionsExt;
    let metadata = stdfs::metadata(path)?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o100);
    stdfs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn apply_resource_executable(_path: &Path, _executable: bool) -> io::Result<()> {
    Ok(())
}
```

- [ ] **Step 5: Write resources after `SKILL.md`**

After:

```rust
write_file_atomic(&path, content.as_bytes()).map_err(ToolError::Io)?;
```

add:

```rust
for resource in &resources {
    write_resource_file(&skill_dir, resource).map_err(ToolError::Io)?;
}
```

- [ ] **Step 6: Run resource write and backup tests**

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_writes_resource_files --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_backs_up_existing_resource_directory --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_preserves_unmentioned_existing_resources --exact --nocapture
```

Expected: all pass.

## Task 4: Add Resource Summary to `ListSkills`

**Files:**
- Modify: `crates/neo-agent-core/src/tools/skills_manager.rs`

- [ ] **Step 1: Add failing ListSkills summary test**

Add:

```rust
#[tokio::test]
async fn list_skills_reports_resource_summary() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join("skills").join("resource-skill");
    fs::create_dir_all(skill_dir.join("references"))
        .await
        .expect("mkdir references");
    fs::create_dir_all(skill_dir.join("scripts"))
        .await
        .expect("mkdir scripts");
    fs::create_dir_all(skill_dir.join("assets"))
        .await
        .expect("mkdir assets");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: resource-skill\ndescription: test\ntype: prompt\n---\n\nbody",
    )
    .await
    .expect("write skill");
    fs::write(skill_dir.join("references").join("guide.md"), "guide")
        .await
        .expect("write reference");
    fs::write(skill_dir.join("scripts").join("check.py"), "print('ok')\n")
        .await
        .expect("write script");

    let tool = ListSkillsTool::new(Some(temp.path().to_path_buf()), Vec::new());
    let result = tool.execute(&make_ctx(), json!({})).await.expect("execute");

    assert!(!result.is_error);
    assert!(
        result.content.contains("resource-skill:"),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("[references,scripts]"),
        "{}",
        result.content
    );
    assert!(
        !result.content.contains("assets"),
        "empty resource dirs should be omitted: {}",
        result.content
    );
}
```

- [ ] **Step 2: Run the test and confirm it fails before implementation**

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::list_skills_reports_resource_summary --exact --nocapture
```

Expected: FAIL because `ListSkills` does not summarize resources.

- [ ] **Step 3: Add resource summary helpers**

Add near `ListSkillsTool`:

```rust
fn format_skill_resource_summary(root: &Path) -> String {
    let mut present = Vec::new();
    for dir in RESOURCE_DIRS {
        let path = root.join(dir);
        if resource_dir_has_entries(&path) {
            present.push(*dir);
        }
    }
    if present.is_empty() {
        String::new()
    } else {
        format!(" [{}]", present.join(","))
    }
}

fn resource_dir_has_entries(path: &Path) -> bool {
    let Ok(metadata) = stdfs::symlink_metadata(path) else {
        return false;
    };
    if is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
        return false;
    }
    stdfs::read_dir(path)
        .ok()
        .and_then(|mut entries| entries.next())
        .is_some()
}
```

- [ ] **Step 4: Append the summary in `ListSkills` output**

Replace:

```rust
lines.push(format!("  {}: {}", skill.name, skill.root.display()));
```

with:

```rust
let resource_summary = format_skill_resource_summary(&skill.root);
lines.push(format!(
    "  {}: {}{}",
    skill.name,
    skill.root.display(),
    resource_summary
));
```

- [ ] **Step 5: Run ListSkills tests**

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::list_skills_reports_resource_summary --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::list_skills_discovers_user_skills --exact --nocapture
```

Expected: both pass.

## Task 5: Add Discovery Skip Tests and Implementation

**Files:**
- Modify: `crates/neo-agent-core/tests/skills.rs`
- Modify: `crates/neo-agent-core/src/skills/discovery.rs`

- [ ] **Step 1: Add failing discovery test for resource dirs**

In `crates/neo-agent-core/tests/skills.rs`, add:

```rust
#[test]
fn discover_skills_ignores_resource_dirs_under_skill_package() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path().join("parent").join("SKILL.md"),
        r"---
name: parent
description: Parent skill
---
Parent.
",
    );
    for resource_dir in ["references", "scripts", "assets"] {
        write(
            dir.path()
                .join("parent")
                .join(resource_dir)
                .join("SKILL.md"),
            r"---
name: accidental
description: Accidental resource skill
---
This file is an example resource, not a skill.
",
        );
    }

    let skills = discover_skills(dir.path(), SkillSource::default()).unwrap();
    let names: Vec<_> = skills.iter().map(|skill| skill.name.as_str()).collect();

    assert_eq!(names, vec!["parent"]);
}
```

- [ ] **Step 2: Add passing guard test for canonical child skills**

Add:

```rust
#[test]
fn discover_skills_still_finds_children_under_skills_dir() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path().join("parent").join("SKILL.md"),
        r"---
name: parent
description: Parent skill
---
Parent.
",
    );
    write(
        dir.path()
            .join("parent")
            .join("skills")
            .join("child")
            .join("SKILL.md"),
        r"---
name: child
description: Child skill
---
Child.
",
    );

    let skills = discover_skills(dir.path(), SkillSource::default()).unwrap();
    let names: Vec<_> = skills.iter().map(|skill| skill.name.as_str()).collect();

    assert!(names.contains(&"parent"));
    assert!(names.contains(&"parent/child"));
}
```

- [ ] **Step 3: Run the failing discovery test**

Run:

```bash
cargo test --package neo-agent-core --test skills -- discover_skills_ignores_resource_dirs_under_skill_package --exact --nocapture
```

Expected: FAIL because `parent/references/SKILL.md` is currently discovered.

- [ ] **Step 4: Implement resource-dir skip**

In `discovery.rs`, add:

```rust
const RESOURCE_DIRS: &[&str] = &["references", "scripts", "assets"];

fn is_resource_dir_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| RESOURCE_DIRS.contains(&name))
}
```

Then change the recursion loop in `discover_recursive`:

```rust
for entry in entries {
    let path = entry.path();
    if path.is_dir() {
        if own_skill_file.is_file() && is_resource_dir_name(&path) {
            continue;
        }
        skills.extend(discover_recursive(&path, source, &own_prefix)?);
    }
}
```

- [ ] **Step 5: Run discovery tests**

Run:

```bash
cargo test --package neo-agent-core --test skills -- discover_skills_ignores_resource_dirs_under_skill_package --exact --nocapture
cargo test --package neo-agent-core --test skills -- discover_skills_still_finds_children_under_skills_dir --exact --nocapture
cargo test --package neo-agent-core --test skills -- discover_skills_finds_subskills --exact --nocapture
```

Expected: all pass.

## Task 6: Update Built-in Authoring Skills

**Files:**
- Modify: `crates/neo-agent-core/src/skills/builtin/create-skill.md`
- Modify: `crates/neo-agent-core/src/skills/builtin/self-evo.md`
- Modify: `crates/neo-agent-core/src/tools/skills_manager.rs`

- [ ] **Step 1: Strengthen built-in prompt tests first**

Update `create_skill_builtin_requires_verify_and_create_skill_tool` so it asserts:

```rust
assert!(skill.body.contains("CreateSkill.resources"), "{}", skill.body);
assert!(skill.body.contains("${NEO_SKILL_DIR}"), "{}", skill.body);
assert!(skill.body.contains("references/"), "{}", skill.body);
assert!(skill.body.contains("scripts/"), "{}", skill.body);
assert!(skill.body.contains("assets/"), "{}", skill.body);
```

Update `self_evo_builtin_requires_scope_and_verify_section` so it asserts:

```rust
assert!(skill.body.contains("CreateSkill.resources"), "{}", skill.body);
assert!(skill.body.contains("references/"), "{}", skill.body);
assert!(skill.body.contains("scripts/"), "{}", skill.body);
assert!(skill.body.contains("assets/"), "{}", skill.body);
```

- [ ] **Step 2: Run one built-in prompt test and confirm it fails**

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::self_evo_builtin_requires_scope_and_verify_section --exact --nocapture
```

Expected: FAIL because `self-evo` does not mention resource-backed creation yet.

- [ ] **Step 3: Update `create-skill.md` for direct resource creation**

Edit `create-skill.md` to replace the current "Current `CreateSkill` creates only `SKILL.md`" wording with direct resource guidance:

```markdown
4. Design resources before writing the body:
   - Keep everything in `SKILL.md` when the workflow is concise and self-contained.
   - Use `references/` for heavy API docs, schemas, policies, or examples that should load only when needed.
   - Use `scripts/` only when repeated code or fragile operations need deterministic execution.
   - Use `assets/` for templates, boilerplate, or text fixtures used in final outputs.
   - When resources are useful, pass them through `CreateSkill.resources`; do not describe resource files that you fail to create.
   - In the skill body, route future agents with concrete paths such as `${NEO_SKILL_DIR}/references/schema.md`, `${NEO_SKILL_DIR}/scripts/check_schema.py`, or `${NEO_SKILL_DIR}/assets/template.md`.
```

Also update the `CreateSkill` call step to:

```markdown
10. Call `CreateSkill` with `name`, `description`, `skill_type: "prompt"`, `body`, and `resources` when resource files are part of the design.
```

Update the report step to include:

```markdown
12. Report the created path, whether a backup was made, the reload result, and every resource file created.
```

- [ ] **Step 4: Update `self-evo.md` for resource-backed distillation**

Insert after the step that identifies reusable patterns:

```markdown
4. Decide whether each reusable pattern needs resources:
   - Put long API notes, schemas, command references, or examples in `references/`.
   - Put reusable deterministic helpers in `scripts/`.
   - Put reusable text templates or boilerplate in `assets/`.
   - Keep one-off logs, transient transcript snippets, and project-only facts out of resources.
   - The generated skill body must explicitly route future agents with concrete paths such as `${NEO_SKILL_DIR}/references/schema.md`, `${NEO_SKILL_DIR}/scripts/check_schema.py`, or `${NEO_SKILL_DIR}/assets/template.md`.
```

Renumber the following steps. Change the create step to:

```markdown
8. Call `CreateSkill` with `resources` when the skill needs `references/`, `scripts/`, or `assets/`.
```

Update `## Verify` to include:

```markdown
A successful self-evo run creates only focused skills, each generated skill body includes its own `## Verify` section, resource-backed skills route to existing files through `${NEO_SKILL_DIR}`, and `ListSkills` shows the created names without restarting Neo.
```

- [ ] **Step 5: Run built-in prompt tests**

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_builtin_requires_verify_and_create_skill_tool --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::self_evo_builtin_requires_scope_and_verify_section --exact --nocapture
```

Expected: both pass.

## Task 7: Update Skill Docs in English and Chinese

**Files:**
- Modify: `docs/en/customization/skills.md`
- Modify: `docs/zh/customization/skills.md`

- [ ] **Step 1: Add a package layout section to English docs**

After "What Is a Skill", add:

```markdown
## Skill Package Layout

A skill package may contain optional local resources:

```text
skill-name/
  SKILL.md
  references/   # read-on-demand documentation
  scripts/      # deterministic helpers
  assets/       # templates and output assets
```

`SKILL.md` is the only file Neo loads automatically. Resource files stay local under the skill root and must be referenced explicitly from the skill body. Use `${NEO_SKILL_DIR}` to point future agents at resource files, for example `${NEO_SKILL_DIR}/references/schema.md`.

Use `references/` for long docs, schemas, API notes, and examples. Use `scripts/` for reusable helpers that should be run through normal tool approval. Use `assets/` for text templates, boilerplate, and output materials. `CreateSkill` v1 supports text resources; binary assets should be added manually.

Under a skill package, `references/`, `scripts/`, and `assets/` are not scanned for nested skills. Put child skills under `skills/`.
```
```

- [ ] **Step 2: Update the English `CreateSkill` example**

Replace the JSON example with:

```jsonc
{
  "name": "schema-review",
  "description": "Use when reviewing generated JSON against the team's schema rules.",
  "skill_type": "prompt",
  "body": "# Schema Review\n\n## Workflow\nRead `${NEO_SKILL_DIR}/references/schema-rules.md`, then run `python ${NEO_SKILL_DIR}/scripts/check_schema.py <file>`.\n\n## Verify\nThe script exits successfully and the reviewed file follows every required schema rule.",
  "resources": [
    {
      "path": "references/schema-rules.md",
      "content": "# Schema Rules\n\n- Required fields must be present.\n"
    },
    {
      "path": "scripts/check_schema.py",
      "content": "import sys\nprint(f'checked {sys.argv[1]}')\n",
      "executable": true
    }
  ]
}
```

Then change the explanatory paragraph to:

```markdown
The tool auto-generates frontmatter, writes `SKILL.md`, writes optional text resources under the skill directory, backs up an existing skill package before overwrite, and reloads the active skill store when available.
```

- [ ] **Step 3: Mirror the same section in Chinese docs**

Add a Chinese section with equivalent meaning:

```markdown
## Skill 包结构

一个 skill 包可以包含可选本地资源：

```text
skill-name/
  SKILL.md
  references/   # 按需读取的文档
  scripts/      # 确定性辅助脚本
  assets/       # 模板和输出素材
```

`SKILL.md` 是 Neo 唯一自动加载的入口。资源文件保留在 skill 根目录下，必须由正文显式引用。用 `${NEO_SKILL_DIR}` 指向资源，例如 `${NEO_SKILL_DIR}/references/schema.md`。

`references/` 放长文档、schema、API 说明和示例；`scripts/` 放需要通过正常工具审批运行的可复用辅助脚本；`assets/` 放文本模板、样板和输出素材。`CreateSkill` v1 支持文本资源；二进制素材需要手动添加。

在 skill 包内，`references/`、`scripts/` 和 `assets/` 不会被扫描成子 skill。子 skill 应放在 `skills/` 下。
```
```

- [ ] **Step 4: Mirror the `CreateSkill.resources` JSON example in Chinese docs**

Use the same JSON as English docs. Keep comments and surrounding prose in Chinese.

- [ ] **Step 5: Run docs diff check**

Run:

```bash
git diff --check -- docs/en/customization/skills.md docs/zh/customization/skills.md
```

Expected: no output.

## Task 8: Final Focused Verification

**Files:**
- Verify all touched implementation and docs files.

- [ ] **Step 1: Run exact tool tests**

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_writes_resource_files --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_backs_up_existing_resource_directory --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_preserves_unmentioned_existing_resources --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::list_skills_reports_resource_summary --exact --nocapture
```

Expected: all pass.

- [ ] **Step 2: Run exact discovery tests**

Run:

```bash
cargo test --package neo-agent-core --test skills -- discover_skills_ignores_resource_dirs_under_skill_package --exact --nocapture
cargo test --package neo-agent-core --test skills -- discover_skills_still_finds_children_under_skills_dir --exact --nocapture
```

Expected: both pass.

- [ ] **Step 3: Run exact built-in prompt tests**

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_builtin_requires_verify_and_create_skill_tool --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::self_evo_builtin_requires_scope_and_verify_section --exact --nocapture
```

Expected: both pass.

- [ ] **Step 4: Run formatting and diff checks**

Run:

```bash
cargo fmt --all --check
git diff --check -- crates/neo-agent-core/src/tools/skills_manager.rs crates/neo-agent-core/src/skills/discovery.rs crates/neo-agent-core/tests/skills.rs crates/neo-agent-core/src/skills/builtin/create-skill.md crates/neo-agent-core/src/skills/builtin/self-evo.md docs/en/customization/skills.md docs/zh/customization/skills.md docs/aegis/specs/2026-07-09-skill-resources-design.md docs/aegis/plans/2026-07-09-skill-resources.md
```

Expected: `cargo fmt --all --check` exits 0 and `git diff --check` prints no output.

- [ ] **Step 5: Inspect scoped diff**

Run:

```bash
git diff -- crates/neo-agent-core/src/tools/skills_manager.rs crates/neo-agent-core/src/skills/discovery.rs crates/neo-agent-core/tests/skills.rs crates/neo-agent-core/src/skills/builtin/create-skill.md crates/neo-agent-core/src/skills/builtin/self-evo.md docs/en/customization/skills.md docs/zh/customization/skills.md docs/aegis/specs/2026-07-09-skill-resources-design.md docs/aegis/plans/2026-07-09-skill-resources.md
```

Expected: diff contains only the resource contract implementation, built-in prompt updates, docs, spec, and this plan.

## Self-Review

Spec coverage: every spec section maps to a task. `CreateSkill.resources`, path validation, content limits, write semantics, full-directory backup, symlink safety, executable flag, `ListSkills` resource summary, discovery skip, built-in skill updates, docs, error handling, and migration are covered.

Placeholder scan: this plan contains no unresolved placeholder markers or vague edge-case instructions. Test names, file paths, commands, and expected results are concrete.

Type consistency: `CreateSkillResource`, `resources`, `ValidatedResource`, `validate_resources`, `validate_resource_path`, `write_resource_file`, and `format_skill_resource_summary` are introduced before later tasks use them.

Git policy consistency: the plan intentionally omits commit steps because this Neo thread has a stricter no-git-mutation rule without explicit user authorization.
