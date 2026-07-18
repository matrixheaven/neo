# Aegis Dual-Host Install Implementation Plan

> **For agentic workers:** Execute inline. Do not dispatch subagents and do not touch `docs/superpowers/**`.

**Goal:** Make `$NEO_HOME/skills` Neo's only implicit user skill root and replace both Superpowers installations with one complete shared Aegis Method Pack.

**Architecture:** Neo retains explicit `extra_skill_dirs` and `skill_path`, but removes `$NEO_HOME/.agents/skills` and the unused project `.agents/skills` trust surface. One checkout at `~/.codex/aegis` is exposed through updater-managed Codex and Neo directories whose child skills are symlinks.

**Tech Stack:** Rust 2024, Cargo, TOML, Python 3, Git, filesystem symlinks.

## Global Constraints

- Preserve `extra_skill_dirs`, `skill_path`, and `$NEO_HOME/skills`.
- Delete implicit `$NEO_HOME/.agents/skills` and project `.agents/skills` trust handling.
- Do not modify `docs/superpowers/**`.
- Do not retain Superpowers aliases, backups, or compatibility copies.
- Keep one editable Aegis checkout and two generated host views.
- Do not commit or perform unrelated git mutations.

---

### Task 1: Canonical Neo Skill Discovery

**Files:**
- Modify: `crates/neo-agent-core/src/skills/discovery.rs`
- Modify: `crates/neo-agent-core/tests/skills.rs`
- Modify: `docs/en/customization/skills.md`
- Modify: `docs/zh/customization/skills.md`

**Interfaces:**
- Consumes: `user_skill_dirs(user_dir: &Path) -> Vec<PathBuf>`
- Produces: `$NEO_HOME/skills` as the sole implicit root.

- [ ] Add `user_skill_dirs_contains_only_neo_skills`:

```rust
#[test]
fn user_skill_dirs_contains_only_neo_skills() {
    let home = Path::new("/home/alice/.neo");
    assert_eq!(user_skill_dirs(home), vec![home.join("skills")]);
}
```

- [ ] Run the test and confirm it fails because `.neo/.agents/skills` remains:

```bash
cargo test --package neo-agent-core --test skills -- user_skill_dirs_contains_only_neo_skills --exact --nocapture
```

- [ ] Replace the implementation with:

```rust
#[must_use]
pub fn user_skill_dirs(user_dir: &Path) -> Vec<PathBuf> {
    vec![user_dir.join("skills")]
}
```

- [ ] Remove `~/.neo/.agents/skills/` only from the en/zh user-tier rows. Keep both explicit configuration examples.

- [ ] Verify implicit and explicit roots:

```bash
cargo test --package neo-agent-core --test skills -- user_skill_dirs_contains_only_neo_skills --exact --nocapture
cargo test --package neo-agent --bin neo -- resources::tests::skill_path_tilde_expands_to_user_home --exact --nocapture
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_completion_refreshes_skills_from_disk --exact --nocapture
```

---

### Task 2: Retire Project `.agents/skills` Trust Surface

**Files:**
- Modify: `crates/neo-agent/src/trust.rs`
- Modify: `crates/neo-agent/src/trust_commands.rs`
- Modify: `crates/neo-tui/src/dialogs/trust.rs`
- Modify: `crates/neo-tui/tests/trust_dialog.rs`

**Interfaces:**
- Consumes: `TrustInputKind`, `TrustDialogInputKind`, `inputs_in_dir`.
- Produces: trust inputs for `AGENTS.md` and `.neo` only.

- [ ] Rename the existing trust test to `trust_inputs_detect_neo_directory_and_ignore_agents_skills`, create both directories, and assert:

```rust
assert_eq!(inputs.detected.len(), 1);
assert_eq!(inputs.detected[0].1, TrustInputKind::NeoDir);
```

- [ ] Confirm it fails:

```bash
cargo test --package neo-agent --bin neo -- trust::tests::trust_inputs_detect_neo_directory_and_ignore_agents_skills --exact --nocapture
```

- [ ] Remove `AgentsSkillsDir` from both enums, delete `.agents/skills` probing, dialog mapping, command label, and directory-rendering match arm.

- [ ] Delete the `.agents/skills` TUI fixture and rendered-text assertion.

- [ ] Verify trust behavior and UI:

```bash
cargo test --package neo-agent --bin neo -- trust::tests::trust_inputs_detect_neo_directory_and_ignore_agents_skills --exact --nocapture
cargo test --package neo-tui --test trust_dialog -- trust_dialog_renders_detected_inputs_without_file_contents --exact --nocapture
```

---

### Task 3: Build and Install Neo

Before building, add Codex-compatible frontmatter repair in
`crates/neo-agent-core/src/skills/mod.rs` with a regression test in
`crates/neo-agent-core/tests/skills.rs` using Aegis's unquoted
`TDD Route: strict` description. Verify the new test fails before the repair,
then passes together with `load_skill_rejects_invalid_yaml` after the repair.

**Files:**
- Build input: `crates/neo-agent/Cargo.toml`
- Install target: `~/.cargo/bin/neo`

- [ ] Check formatting and the protected documentation boundary:

```bash
cargo fmt --all --check
git diff --check
git diff --exit-code -- docs/superpowers
```

- [ ] Build and install:

```bash
cargo build --package neo-agent --bin neo
cargo install --path crates/neo-agent --locked --force
neo --version
```

Expected: all commands succeed and the active executable is `~/.cargo/bin/neo`.

---

### Task 4: Replace Both Superpowers Installations

**Files:**
- Create: `~/.codex/aegis/`
- Delete: `~/.agents/skills/superpowers/`
- Delete: `~/.neo/skills/superpowers/`
- Create: `~/.agents/skills/aegis/` with generated child symlinks
- Create: `~/.neo/skills/aegis/` with generated child symlinks
- Create/update: `~/.config/aegis/config.toml`
- Create/update: `~/.config/aegis/installations.json`

- [ ] Clone and validate before removing Superpowers:

```bash
git clone https://github.com/GanyuanRan/Aegis.git ~/.codex/aegis
cd ~/.codex/aegis
python scripts/aegis-doctor.py --json
```

Expected: `ok: true` and `workspaceSupport: available`.

- [ ] Delete only the two Superpowers directories and create empty real `aegis` discovery-root directories.

- [ ] Configure and verify both discovery views:

```bash
python scripts/aegis-doctor.py --write-config --json
```

Expected: the canonical method pack reports `ok: true`, `workspaceSupport: available`, and `configStatus: configured`.

- [ ] Register both host views:

```bash
python scripts/aegis-update.py register --host codex --sync-mode symlink --discovery-shape direct-child --discovery-root ~/.agents/skills/aegis --reload-hint "restart Codex"
python scripts/aegis-update.py register --host neo --sync-mode symlink --discovery-shape direct-child --discovery-root ~/.neo/skills/aegis --reload-hint "restart Neo"
python scripts/aegis-update.py status --json
```

Expected: register-time sync creates current child links, doctor verifies each
distinct root, and `codex:default` plus `neo:default` share one method-pack root.

---

### Task 5: Full Installation Smoke

- [ ] Confirm every child link in both discovery roots resolves to the canonical skills tree and both Superpowers paths are absent.

- [ ] Initialize and check an external temporary Aegis workspace:

```bash
python ~/.codex/aegis/scripts/aegis-workspace.py init --root <temp-dir>
python ~/.codex/aegis/scripts/aegis-workspace.py check --root <temp-dir>
```

- [ ] Run fresh Codex and Neo explicit `using-aegis` activation smokes; output must not reference Superpowers.

- [ ] Run final checks:

```bash
git diff --check
git diff --exit-code -- docs/superpowers
python ~/.codex/aegis/scripts/aegis-doctor.py --write-config --json
python ~/.codex/aegis/scripts/aegis-update.py status --json
```

## Self-Review

- Every approved requirement maps to a task.
- Explicit extra roots are tested, not merely left in the schema.
- `.agents/skills` is removed from implicit discovery and unused trust UI.
- Aegis validates before Superpowers is deleted.
- Full method-pack scripts and workspace support use one canonical checkout.
- No task modifies `docs/superpowers/**`.
