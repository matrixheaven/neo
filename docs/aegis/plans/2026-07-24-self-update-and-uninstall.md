# Neo Self-Update and Uninstall Implementation Plan

> Executor note: implement the approved lifecycle contract exactly. Do not
> restart product discovery, rename the flags, add package-manager behavior,
> add a background updater, invent a version database, retain multiple backups,
> use `self_replace::self_delete` for uninstall, or add a Neo-authored Windows
> helper. Read this plan, the approved spec, `AGENTS.md`, `CX.md`, and `RTK.md`,
> then work task by task. Stop and return to the spec if a pinned dependency
> cannot satisfy a stated invariant.

## Goal

Align Neo's banner and `neo --version` with its release tag, reject mismatched
release tags before building assets, and add these lifecycle commands:

```text
neo update
neo update --unstable
neo update --stable
neo update --rollback
neo uninstall
neo uninstall -y
neo uninstall --yes
```

Update must select exact GitHub Release assets for macOS, Linux, and Windows on
x86_64 and ARM64; verify the GitHub SHA-256 digest and staged binary version;
create one adjacent `.bak`; restore it when replacement fails; and consume it
once on explicit rollback. Uninstall must remove the exact running binary, then
its `.bak`, and delete the guarded active Neo home only after Y/Yes or
`-y`/`--yes` and only after both binary artifacts are absent.

## Architecture

```text
Cli::parse
    |
    +-> lifecycle command? -- yes --> modes::lifecycle (before AppConfig::load)
    |                                  |
    |                                  +-> update channel selection
    |                                  |     -> exact GitHub tag/asset
    |                                  |     -> digest + staged --version
    |                                  |     -> atomic .bak promotion
    |                                  |     -> self_replace
    |                                  |     -> exact-path recovery on error
    |                                  |
    |                                  +-> offline rollback
    |                                  |     -> verify .bak
    |                                  |     -> guarded self_replace
    |                                  |     -> consume .bak once
    |                                  |
    |                                  +-> uninstall
    |                                        -> prompt/guard home
    |                                        -> current exe -> .bak -> home
    |
    +-> no --> existing config/session/provider/TUI dispatch unchanged

Cargo package version --+-> clap `neo --version`
                        +-> every interactive banner call site
                        +-> release validation expects tag `v{version}`
```

`modes::lifecycle` is the only new behavior owner. `cli.rs` parses arguments,
`main.rs` performs early wiring, `config::neo_home()` remains the only Neo-home
resolver, Cargo package metadata remains the only version source, and the
release workflow remains the only asset publisher.

## Tech Stack

- Rust 2024, minimum Rust 1.96.1
- existing `tokio`, `anyhow`, `clap`, and `tempfile`
- `self_update = "=1.0.0-rc.6"` with only `reqwest`, `rustls`, `github`,
  `async`, `archive-tar`, `compression-tar-gz`, `archive-zip`,
  `compression-zip-deflate`, and `checksums`
- direct `self-replace = "=1.5.0"`, because `self_update` 1.0 no longer
  re-exports the replacement primitive used by offline rollback/recovery
- `semver = "1.0.28"`, using `Version::cmp_precedence` so build metadata does
  not order releases
- GitHub Actions, `cargo metadata --no-deps --format-version 1`, and `jq`
- `cargo-nextest`, `cargo fmt`, targeted Clippy, `git diff --check`

## Baseline And Authority Refs

- `AGENTS.md`
- `CX.md`
- `RTK.md`
- `docs/aegis/specs/2026-07-24-self-update-and-uninstall-design.md`
  (approved product and architecture contract)
- `.github/workflows/release.yml` (six existing release targets and archive
  names)
- `Cargo.toml` and the four workspace crate manifests (version authority)
- `crates/neo-agent/src/cli.rs` (CLI owner)
- `crates/neo-agent/src/main.rs` (dispatch owner)
- `crates/neo-agent/src/config/paths.rs` (`neo_home()` and `user_home()` owner)
- `crates/neo-agent/src/modes/interactive/mod.rs` and
  `crates/neo-agent/src/modes/interactive/sessions.rs` (banner evidence only;
  no code change expected)
- `docs/en/quickstart.md`, `docs/zh/quickstart.md`, and `README.md`
- `self_update 1.0.0-rc.6` public API and `self-replace 1.5.0` platform
  implementations, verified during planning

## Compatibility Boundary

Preserve:

- all existing commands, global flags, TUI/session behavior, config precedence,
  and provider/runtime behavior;
- `config::neo_home()` semantics: `NEO_HOME`, otherwise platform home + `.neo`;
- the six current archive names and archive contents;
- immutable `v0.1.0` plain Unix assets as the only historical packaging
  exception;
- Windows update through the dependency's replacement implementation;
- Windows uninstall as an explicit occupied-file error with no data deletion.

Do not add:

- `--rc`, arbitrary `--version`, nightly, channel persistence, update checks,
  daemon/TUI notification, package-manager fallback, privilege escalation, or
  shell/PowerShell scripts;
- a second version file, Git-derived runtime version, duplicate Neo-home
  resolver, backend trait with one implementation, backup manifest, timestamped
  backups, swap-style rollback, or automatic cleanup scan;
- `self_replace::self_delete()` for uninstall. That API schedules Windows
  deletion and would violate the explicit occupied-file contract.

The dependency's internal temporary Windows replacement process is allowed for
update only. Neo must not expose or reuse that deletion path for uninstall.

## TDD Route

- Mode: off.
- Decision: skipped.
- Strict authority: not applicable; neither the user nor project requested
  strict test-first TDD.
- Test posture: implement the minimum approved owner, then add focused
  post-change regression tests for each policy and filesystem boundary.
- Reason: the work is an approved feature contract with deterministic policy
  helpers and dependency-owned platform replacement, not a strict-TDD request.
- Verification: every automated test command names `neo-agent`, the `neo`
  binary target, and one exact lifecycle test filter.

## Verification

Use focused commands while implementing:

```bash
rtk cargo nextest run -p neo-agent --bin neo lifecycle::tests::release_selection_enforces_channel_and_downgrade_policy
rtk cargo nextest run -p neo-agent --bin neo lifecycle::tests::platform_assets_cover_six_targets_and_v0_1_0
rtk cargo nextest run -p neo-agent --bin neo lifecycle::tests::backup_promotion_and_failed_replace_preserve_recovery
rtk cargo nextest run -p neo-agent --bin neo lifecycle::tests::rollback_is_offline_and_consumes_one_backup
rtk cargo nextest run -p neo-agent --bin neo lifecycle::tests::uninstall_confirmation_and_partial_order_are_safe
rtk cargo nextest run -p neo-agent --bin neo lifecycle::tests::cli_lifecycle_contract_is_exact
rtk cargo fmt --all --check
rtk cargo clippy -p neo-agent --bin neo -- -D clippy::all
rtk git diff --check
```

Do not use broad `cargo test`, package-wide unfiltered `cargo nextest`, or
workspace-wide Clippy as completion evidence. Cross-platform acceptance is a
separate final task and must distinguish baseline/environment failures from
patch regressions.

## Aegis Visibility

Planning is required because release identity, network selection, executable
replacement, persistent user-data deletion, Windows file locking, and one-slot
recovery cross several owners. The plan prevents a locally convenient updater
from bypassing version authority, config-independent dispatch, or the deletion
order that protects `~/.neo`.

## Plan Basis

### Facts

- All four Neo crate manifests currently say `0.1.1`.
- Banner call sites and clap already use `CARGO_PKG_VERSION`; they do not need a
  new version lookup.
- The published complete release is
  `v0.1.1-rc.2+20260721.0634` with six archive assets and GitHub digests.
- `dispatch()` currently calls `AppConfig::load()` before subcommand dispatch.
- `config::neo_home()` and `config::user_home()` already implement the required
  platform home semantics.
- `self_update` can fetch all GitHub releases asynchronously, pin an exact tag,
  match an exact asset, verify the release digest, extract tar.gz/zip/plain
  assets, run a staged-binary verification hook, and replace the running binary.
- `self_update` does not reject a missing GitHub digest by default; Neo must
  require `ReleaseAsset::digest().is_some()` in exact asset matching.
- `self_update` no longer re-exports `self-replace`; offline rollback needs the
  direct dependency.
- On Windows, `self_replace()` may rename the running executable before a later
  copy/rename error. A second `self_replace()` call can then fail because
  `current_exe().canonicalize()` points at the now-absent original path.
- `tempfile::NamedTempFile::persist()` atomically replaces an existing path on
  Unix and Windows when the temp file is created in the destination directory.

### Assumptions

- GitHub public release listings omit inaccessible drafts; Neo still ignores
  every non-SemVer tag.
- The release tag contract remains `v{bare Cargo SemVer}`. The updater may
  reconstruct the exact tag from the selected version because release CI now
  enforces that shape.
- Users invoking self-update installed a standalone Neo binary whose directory
  is writable by that user. Permission failure remains an explicit error.

### Unknowns

- Hosted GitHub runner availability and public API rate limits are external.
- Filesystem/endpoint security products can impose Windows sharing rules beyond
  the standard platform behavior. Exact OS errors must be retained.
- Signed publisher assets have no approved key contract; GitHub digest
  verification provides integrity, not independent authenticity.

## Baseline Usage Draft

- Required baseline refs: approved spec, current version/CLI/dispatch/path
  owners, release workflow, quickstarts, dependency replacement APIs.
- Acknowledged before plan refs: all required refs above.
- Cited in plan refs: all required refs above.
- Missing refs: none blocking.
- Decision: continue.

## Requirement Ready Check

- Requirement source refs: approved self-update/uninstall design plus the
  user's explicit one-time rollback decision.
- Goals and scope refs: design sections 3-5 and this plan Goal.
- User/scenario refs: standalone Neo users on macOS/Linux/Windows update between
  stable/prerelease releases, recover failed replacement, rollback once, or
  uninstall with an optional data deletion confirmation.
- Requirement item refs: exact CLI matrix, six assets, digest/version gates,
  `.bak` transaction, Windows locking behavior, guarded home deletion.
- Acceptance/verification criteria refs: design sections 12 and 16.
- Open blocker questions: none.
- Decision: ready.

## Ripple Signal Triage

- Distribution/release surface: yes; Cargo version and GitHub workflow change.
- Persistent-state deletion: yes; guarded active Neo home only.
- Shared/core runtime: no; lifecycle stays in the binary crate and bypasses
  provider/session startup.
- Downstream consumers: clap help/version output, banner metadata, release
  assets, English/Chinese docs.
- Compatibility expansion: six OS/architecture targets plus immutable `v0.1.0`
  packaging.
- Required verification expansion: binary unit tests, six target builds, three
  native OS acceptance runs, and public-release metadata inspection.
- Decision: proceed as one lifecycle workstream with task-level commits.

## Change Necessity

- User-visible need: Neo cannot currently update, rollback, or uninstall itself,
  and release identity can diverge from the banner.
- No-change/non-code option: docs and release process notes cannot expose the
  commands, perform verified replacement, or protect deletion order.
- Why code change is necessary: clap, early dispatch, release selection,
  filesystem transaction, and uninstall prompting are runtime behavior.
- Minimum change boundary: crate metadata/lockfile, release workflow,
  `cli.rs`, `main.rs`, `modes/mod.rs`, one `modes/lifecycle.rs`, current docs,
  and a post-verification ADR.
- Decision: code-change.

## Existence Check

- Proposed new surface: one lifecycle mode module.
- Existing owner/reuse candidate: CLI, main dispatch, Cargo version,
  `config::neo_home()`, GitHub workflow, `self_update`, `self-replace`, and
  `tempfile` are reused.
- Why existing surface is insufficient: no current owner coordinates release
  selection, executable backup/replacement/recovery, rollback, or uninstall.
- Creation proof: two user-facing lifecycle commands need one cohesive owner and
  must run without `AppConfig`.
- Entropy/retirement impact: one module and one adjacent backup slot; no
  persistent channel, adapter, fallback owner, or history store.
- Decision: add-with-proof.

## Architecture Integrity Lens

- Invariant: one version owner, one Neo-home owner, exact release asset, verified
  successor, one backup slot, and no home deletion while any binary removal
  failed.
- Canonical owner/contract: Cargo package version; release workflow;
  `config::neo_home()`; `modes::lifecycle`.
- Responsibility overlap: `main.rs` only wires early dispatch; it must not own
  selection, filesystem, prompt, or recovery logic.
- Higher-level simplification: use `self_update` for network/archive/digest and
  `self-replace` for normal cross-platform replacement; use `tempfile::persist`
  for atomic backup/exact-path recovery.
- Retirement/falsifier: a second resolver, backup format, channel store,
  package-manager branch, Windows uninstall helper, or caller-side recovery
  branch falsifies the plan.
- Verdict: proceed with one module and no compatibility layer.

## Plan Pressure Test

- Owner/contract/retirement: exact owners are known; only the immutable v0.1.0
  asset shape remains as bounded compatibility.
- Architecture integrity/higher-level path: dependency primitives cover the
  hard platform operations; no custom archive or Windows process code is needed.
- Verification scope: focused deterministic policy/filesystem tests plus native
  platform acceptance and six-target builds.
- Task executability: each task below names files, fixed APIs, commands, expected
  output, and commit boundary.
- Pressure result: proceed.

## Complexity Budget

- Artifact class: binary CLI owner, new lifecycle source owner, release workflow,
  focused tests, and bilingual docs.
- Target files/artifacts: `main.rs` (878 lines), `cli.rs` (307), new
  `lifecycle.rs`, release workflow (111), quickstarts (184 each), README, ADR.
- Current pressure: `main.rs` is above the 800-line soft signal but receives only
  early wiring; new behavior must not be added there.
- Projected post-change pressure: `main.rs` grows by one bounded match; lifecycle
  production and tests remain cohesive in one module and should stay below the
  800-line soft signal. This detailed handoff plan is about 1,100 lines: above
  the soft signal but below the 1,200-line strong signal.
- Budget result: source boundary is within-budget if behavior stays out of
  `main.rs`; the plan artifact is at-risk-but-governed because one ordered file
  is materially easier for a zero-context executor than split plan fragments.
- Planned governance: keep pure policy helpers compact, use table-driven cases,
  avoid a backend trait, and split only if the actual file crosses the soft
  signal with clearly independent update/uninstall sections.

## Plan-Time Complexity Check

- Target files: `main.rs`, `cli.rs`, and new `modes/lifecycle.rs`.
- Existing size/shape signals: `main.rs` is large/mixed; `cli.rs` is a direct
  enum owner; lifecycle is a new cohesive owner.
- Owner fit: wiring-only in `main.rs`, flags-only in `cli.rs`, all behavior in
  lifecycle.
- Add-in-place risk: placing update/uninstall helpers in `main.rs` would add a
  new responsibility to an already large router.
- Better file boundary: exactly one lifecycle module, as approved.
- Recommendation: add the lifecycle owner and keep `main.rs` wiring-only. If
  lifecycle crosses 800 lines after formatting, move only its `#[cfg(test)]`
  module to `modes/lifecycle/tests.rs`; do not fragment production ownership.

## Execution Readiness View

- Intent Lock: deliver exact cross-platform update/rollback/uninstall behavior
  and repair release identity.
- Scope Fence: manifests/lockfile, release workflow, CLI/early dispatch, one
  lifecycle owner, bilingual docs, README link, focused tests, ADR after proof.
- Baseline Lock: approved design and current six-asset workflow.
- Approved Behavior: `--unstable`, `--stable`, and `--rollback` are mutually
  exclusive; rollback consumes one `.bak`; uninstall asks about Neo home.
- Owner/Contract Constraints: no config load before lifecycle; Cargo and
  `config::neo_home()` remain canonical; lifecycle returns plain text.
- Compatibility Boundary: v0.1.0 plain Unix only; current archives otherwise;
  no package-manager or old flag alias.
- Retirement Boundary: no stale `0.1.1` Neo package version, no mismatched tag
  release path, no extra backup after successful rollback/recovery.
- Task Batches: release identity; policy/mapping; verified update; rollback;
  uninstall; CLI wiring; docs; native acceptance and ADR.
- Test Obligations: six exact binary-unit filters, formatting/Clippy/diff,
  public metadata inspection, three native OS runs, six target builds.
- Review Gates: stop if digest absence cannot be made fatal, staged version
  cannot be verified before backup, backup promotion is not atomic, Windows
  error recovery cannot restore the captured path, or uninstall would schedule
  delayed deletion.
- Drift/Rewind Rules: return to the approved spec instead of adding a helper,
  fallback, second backup, package manager, or broader config refactor.
- Evidence Required Before Completion: focused command output, exact partial
  failure evidence, native OS evidence, six assets/targets, lingering-reference
  scan, and ADR/index sync.
- Advisory Boundary: method-pack execution guidance only; not `GateDecision`,
  `PolicySnapshot`, or completion authority.

## Frozen Implementation Contract

### CLI Shapes

Add only these variants to `cli::Command`:

```rust
/// Update Neo from GitHub Releases or restore the previous installation
Update {
    /// Install the newest prerelease
    #[arg(long, conflicts_with_all = ["stable", "rollback"])]
    unstable: bool,
    /// Return from a prerelease to the newest stable release
    #[arg(long, conflicts_with_all = ["unstable", "rollback"])]
    stable: bool,
    /// Restore and consume the adjacent .bak without network access
    #[arg(long, conflicts_with_all = ["unstable", "stable"])]
    rollback: bool,
},
/// Remove this Neo binary and optionally its active data directory
Uninstall {
    /// Answer yes to the data-deletion confirmation
    #[arg(short = 'y', long = "yes")]
    yes: bool,
},
```

Do not add a `--rc` alias or a nested update subcommand.

### Lifecycle API

`modes::lifecycle` exposes only:

```rust
pub(crate) async fn update(
    unstable: bool,
    stable: bool,
    rollback: bool,
) -> anyhow::Result<String>;

pub(crate) fn uninstall(yes: bool) -> anyhow::Result<String>;
```

Everything else remains private to the module.

### Internal Policy Types

Use these semantic types; names may change only if the replacement is equally
small and preserves the same ownership:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpdateMode {
    Stable,
    Unstable,
    StableSwitch,
    Rollback,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AssetSpec {
    archive_name: String,
    binary_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TargetRelease {
    version: semver::Version,
    tag: String,
    asset: AssetSpec,
}

enum ReleaseDecision {
    AlreadyCurrent { channel: &'static str },
    Install(TargetRelease),
    RequireStableSwitch { current: semver::Version, target: semver::Version },
}
```

Use `Version::cmp_precedence`, never derived `Ord`, for update decisions because
build metadata must compare as equal precedence.

### Exact Asset Table

```text
linux  x86_64 -> neo-linux-x86_64.tar.gz  / neo-linux-x86_64
linux  aarch64 -> neo-linux-arm64.tar.gz  / neo-linux-arm64
macos  x86_64 -> neo-macos-x86_64.tar.gz  / neo-macos-x86_64
macos  aarch64 -> neo-macos-arm64.tar.gz  / neo-macos-arm64
windows x86_64 -> neo-windows-x86_64.zip  / neo-windows-x86_64.exe
windows aarch64 -> neo-windows-arm64.zip   / neo-windows-arm64.exe
```

For version exactly `0.1.0`, remove `.tar.gz` from Linux/macOS archive names.
Windows keeps `.zip`. No other historical fallback exists.

### Replacement Transaction

1. Fetch all public releases and choose a channel target with pure Neo policy.
2. Validate exactly one exact-name asset and require `digest().is_some()`.
3. Configure `self_update` with the exact reconstructed `v{version}` tag,
   exact asset matcher, exact archive binary path, unattended output, and a
   staged-binary verification hook.
4. In the verification hook, require staged `neo --version` to equal the target.
5. Only then copy current Neo to a `NamedTempFile` beside `current_exe()`, copy
   permissions, flush/sync, verify its version, and `persist()` it over `.bak`.
6. Mark backup-ready only after `persist()` succeeds; then let `self_update`
   invoke `self_replace`.
7. If update fails before backup-ready, return the error without recovery.
8. If it fails after backup-ready, restore the captured installation path from
   `.bak` using a new verified `NamedTempFile` beside that path. Do not call
   `self_replace` again on Windows after the original path may have disappeared.
9. Verify the restored path reports the original version. On success remove
   `.bak` and still return the update error plus restoration success. On restore
   failure retain `.bak` and report both errors and exact paths.

For manual rollback, keep a transient verified copy of the current version
until replacement and post-replacement verification finish. If either fails,
restore that transient copy to the captured installation path and leave the
canonical `.bak` untouched. Delete `.bak` only after rollback is installed and
verified.

### Uninstall Transaction

1. Resolve `current_exe()`, adjacent `.bak`, `config::neo_home()`, and
   `config::user_home()` before mutation.
2. For an existing Neo home, reject relative paths, filesystem/drive roots, the
   user home itself, non-directories, and symlink targets that cannot be proven
   safe. Canonicalize only for validation; never broaden the target.
3. Prompt `Delete Neo data at <exact-path>? [y/N]` unless `--yes` or no home.
4. Remove the exact current executable with `std::fs::remove_file`.
5. If and only if step 4 succeeded or was already absent, remove the exact
   adjacent `.bak` entry without following it.
6. If and only if both binary artifacts are absent, remove the confirmed home.
7. Preserve and report partial state. On Windows, the expected occupied current
   `.exe` error stops before `.bak` or home deletion.

## File Map

| File | Action | Ownership |
| --- | --- | --- |
| `Cargo.toml` | Modify | Add pinned workspace dependencies |
| `crates/neo-ai/Cargo.toml` | Modify | Align package version |
| `crates/neo-agent-core/Cargo.toml` | Modify | Align package version |
| `crates/neo-tui/Cargo.toml` | Modify | Align package version |
| `crates/neo-agent/Cargo.toml` | Modify | Align version and consume lifecycle dependencies |
| `Cargo.lock` | Modify | Lock aligned local versions and pinned dependencies |
| `.github/workflows/release.yml` | Modify | Validate version/tag before six builds |
| `crates/neo-agent/src/cli.rs` | Modify | Parse exact commands/flags only |
| `crates/neo-agent/src/main.rs` | Modify | Early lifecycle dispatch only |
| `crates/neo-agent/src/modes/mod.rs` | Modify | Register lifecycle owner |
| `crates/neo-agent/src/modes/lifecycle.rs` | Create | All lifecycle policy and behavior plus focused tests |
| `docs/en/quickstart.md` | Modify | English lifecycle reference |
| `docs/zh/quickstart.md` | Modify | Chinese lifecycle reference |
| `README.md` | Modify | Link to detailed quickstart lifecycle docs |
| `docs/aegis/adr/ADR-0005-self-update-lifecycle.md` | Create after proof | Durable architecture decision; stop if ADR-0005 is occupied |
| `docs/aegis/INDEX.md` | Modify via Aegis helper | Register plan and eventual ADR |

Do not edit neo-ai/provider/core/TUI/session/tool code. Banner code remains
unchanged because it already reads `CARGO_PKG_VERSION`.

## Task 1: Align Version Identity And Gate Releases

**Files:**

- Modify: `crates/neo-ai/Cargo.toml`
- Modify: `crates/neo-agent-core/Cargo.toml`
- Modify: `crates/neo-tui/Cargo.toml`
- Modify: `crates/neo-agent/Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `.github/workflows/release.yml`

**Why:** The banner and clap already consume package metadata; release drift is
fixed at the source by aligning package versions and rejecting a wrong tag.

**Change Necessity:** A process note cannot stop an invalid tag build. The
minimum source boundary is package metadata plus the release workflow gate.

**Impact/Compatibility:** No banner source changes. All four Neo packages move
to `0.1.1-rc.2+20260721.0634`; non-Neo dependency versions named `0.1.1` in the
lockfile must not be changed.

**Verification:**

```bash
rtk cargo check -p neo-agent --bin neo
rtk cargo run -p neo-agent --bin neo -- --version
rtk git diff --check
```

Expected version output:

```text
neo 0.1.1-rc.2+20260721.0634
```

**Steps:**

1. Change exactly the four Neo package `version` fields to
   `0.1.1-rc.2+20260721.0634`; let Cargo update only their lockfile package
   entries.
2. Add a `validate` Ubuntu job before `build`. Checkout, install the Rust
   toolchain, run `cargo metadata --no-deps --format-version 1`, select exactly
   `neo-ai`, `neo-agent-core`, `neo-tui`, and `neo-agent` with `jq`, require four
   rows and one unique version, then require
   `GITHUB_REF_NAME == v${version}`.
   Use this job body (the existing `build` job remains otherwise unchanged):

   ```yaml
   validate:
     runs-on: ubuntu-latest
     steps:
       - uses: actions/checkout@v4
       - uses: dtolnay/rust-toolchain@stable
       - name: Validate package versions and tag
         shell: bash
         run: |
           set -euo pipefail
           metadata="$(cargo metadata --no-deps --format-version 1)"
           mapfile -t versions < <(
             jq -r '.packages[]
               | select(.name == "neo-ai"
                   or .name == "neo-agent-core"
                   or .name == "neo-tui"
                   or .name == "neo-agent")
               | .version' <<<"$metadata"
           )
           if [ "${#versions[@]}" -ne 4 ]; then
             echo "expected four Neo workspace packages, found ${#versions[@]}" >&2
             exit 1
           fi
           mapfile -t unique_versions < <(printf '%s\n' "${versions[@]}" | sort -u)
           if [ "${#unique_versions[@]}" -ne 1 ]; then
             printf 'Neo package versions differ: %s\n' "${versions[*]}" >&2
             exit 1
           fi
           expected_tag="v${unique_versions[0]}"
           if [ "$GITHUB_REF_NAME" != "$expected_tag" ]; then
             echo "release tag $GITHUB_REF_NAME must equal $expected_tag" >&2
             exit 1
           fi
   ```
3. Add `needs: validate` to `build`; do not change its six-target matrix or
   packaging commands.
4. Run the three verification commands and inspect that banner call sites still
   contain `env!("CARGO_PKG_VERSION")` rather than a new version owner:

   ```bash
   rtk rg -n 'CARGO_PKG_VERSION' crates/neo-agent/src/modes/interactive
   ```

5. Commit only this slice:

   ```bash
   rtk git add Cargo.lock crates/neo-ai/Cargo.toml crates/neo-agent-core/Cargo.toml crates/neo-tui/Cargo.toml crates/neo-agent/Cargo.toml .github/workflows/release.yml
   rtk git diff --cached --check
   rtk git commit -m "fix: align release version identity"
   ```

## Task 2: Add Dependencies, Release Policy, And Asset Mapping

**Files:**

- Modify: `Cargo.toml`
- Modify: `crates/neo-agent/Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `crates/neo-agent/src/modes/mod.rs`
- Create: `crates/neo-agent/src/modes/lifecycle.rs`

**Why:** Channel and platform decisions must be deterministic, testable, and
independent from network/filesystem mutation.

**Change Necessity:** Existing CLI/config modules do not own release policy.
The minimum boundary is one new lifecycle module using pinned dependencies.

**Impact/Compatibility:** The v0.1.0 Unix exception is explicit and bounded.
No fallback matching or target guessing is permitted.

**Verification:**

```bash
rtk cargo nextest run -p neo-agent --bin neo lifecycle::tests::release_selection_enforces_channel_and_downgrade_policy
rtk cargo nextest run -p neo-agent --bin neo lifecycle::tests::platform_assets_cover_six_targets_and_v0_1_0
rtk cargo clippy -p neo-agent --bin neo -- -D clippy::all
```

**Steps:**

1. Add these exact workspace dependency entries:

   ```toml
   self-replace = "=1.5.0"
   self_update = { version = "=1.0.0-rc.6", default-features = false, features = ["reqwest", "rustls", "github", "async", "archive-tar", "compression-tar-gz", "archive-zip", "compression-zip-deflate", "checksums"] }
   semver = "1.0.28"
   ```

   Add these exact `neo-agent` entries:

   ```toml
   self-replace.workspace = true
   self_update.workspace = true
   semver.workspace = true
   ```
2. Register `pub mod lifecycle;` in `modes/mod.rs`.
3. Add the constants `REPO_OWNER = "matrixheaven"`, `REPO_NAME = "neo"`, and
   the private policy types from Frozen Implementation Contract.
4. Implement `UpdateMode::from_flags`. Accept exactly the four valid boolean
   states and return an error for every multi-true state even though clap will
   also reject it.
5. Implement `platform_asset(version, os, arch)` with the exact six-row table
   and v0.1.0 exception. Return a named unsupported-platform error before any
   network call.
6. Implement release selection by parsing `Release::version()`, filtering the
   selected stable/unstable channel, scanning for the greatest
   `cmp_precedence`, and preserving the first GitHub item on equal precedence.
   Apply these decisions:
   - default stable: never downgrade; a prerelease newer than stable returns
     `RequireStableSwitch`;
   - unstable: never downgrade any newer stable/prerelease;
   - stable switch: permit a downgrade only when current is prerelease;
   - equal precedence, including different build metadata: `AlreadyCurrent`.
7. Implement exact asset validation: one exact filename, no duplicate, and a
   present GitHub digest. Return a hard error for zero/multiple/missing digest.
8. Add one table-driven selection test covering all channel/downgrade/equal
   precedence cases, and one mapping test covering six targets, v0.1.0 Unix,
   and unsupported OS/architecture. Build releases with
   `self_update::Release::builder()` and `ReleaseAsset::new().with_digest()`;
   never contact GitHub in tests.
9. Run the exact verification commands and commit:

   ```bash
   rtk git add Cargo.toml Cargo.lock crates/neo-agent/Cargo.toml crates/neo-agent/src/modes/mod.rs crates/neo-agent/src/modes/lifecycle.rs
   rtk git diff --cached --check
   rtk git commit -m "feat: define lifecycle release policy"
   ```

## Task 3: Implement Verified Update, Backup, And Automatic Recovery

**Files:**

- Modify: `crates/neo-agent/src/modes/lifecycle.rs`

**Why:** Network update must not cross the mutation boundary until both the
successor and recoverable current binary are verified.

**Change Necessity:** The dependency owns transport/archive/digest/replacement,
but Neo must own exact channel, backup timing, staged version, and recovery.

**Impact/Compatibility:** One `.bak` replaces the previous slot only after the
successor is valid. No home/config/project path is touched.

**Verification:**

```bash
rtk cargo nextest run -p neo-agent --bin neo lifecycle::tests::backup_promotion_and_failed_replace_preserve_recovery
rtk cargo nextest run -p neo-agent --bin neo lifecycle::tests::release_selection_enforces_channel_and_downgrade_policy
rtk cargo clippy -p neo-agent --bin neo -- -D clippy::all
```

**Steps:**

1. Implement `backup_path(current_exe)` by appending `.bak` to the complete
   `OsString` filename (`neo.exe` becomes `neo.exe.bak`); reject paths without a
   filename/parent.
2. Implement a staged-copy helper using `NamedTempFile::new_in(parent)`,
   `std::fs::copy`, copied permissions, `flush`, and `sync_all`. On Windows,
   execute only a temporary copy whose suffix is `.exe`; keep canonical backup
   spelling `.bak`.
3. Implement `parse_neo_version_output` and `verify_binary_version`. Require a
   successful child status and exact `neo <semver>` stdout. Treat stderr,
   non-UTF-8, wrong prefix, parse failure, and wrong expected version as errors.
4. Implement backup promotion: stage current beside it, verify the staged copy
   reports `CARGO_PKG_VERSION`, then call `NamedTempFile::persist(backup_path)`.
   Set `Arc<AtomicBool>` backup-ready only after persist succeeds.
5. Fetch releases with
   `github::ReleaseList::configure().repo_owner(...).repo_name(...).build()?.fetch_async()`;
   apply the pure policy before building the updater.
6. Configure the updater with the exact selected tag, exact asset matcher,
   `.bin_name("neo")`, exact `.bin_path_in_archive(...)`, current version,
   `.unattended()`, release digest verification, and `.verify_binary(...)`.
   The hook must verify the successor first and promote the backup second.
   The builder shape is fixed:

   ```rust
   let mut builder = self_update::backends::github::Update::configure();
   builder
       .repo_owner(REPO_OWNER)
       .repo_name(REPO_NAME)
       .current_version(current.to_string())
       .release_tag(target.tag.clone())
       .bin_name("neo")
       .bin_path_in_archive(target.asset.binary_name.clone())
       .unattended()
       .verify_release_digest(true)
       .asset_matcher(move |assets| exact_asset_with_digest(assets, &asset_name))
       .verify_binary(move |staged| {
           verify_and_promote_backup(staged, &expected, &current_exe, &backup)
               .map_err(|error| self_update::Error::verification_rejected(error.to_string()))?;
           backup_ready_for_hook.store(true, std::sync::atomic::Ordering::Release);
           Ok(())
       });
   let updater = builder.build_async()?;
   ```

   `verify_and_promote_backup` must verify `staged` against `expected` before
   copying/promoting current. The closure may capture cloned owned values only;
   it runs in the dependency's blocking finish task.
7. Call `build_async()?.update_extended_async().await`. Add context containing
   target tag, asset name, and platform.
8. On failure with backup-ready false, return the original error. On failure
   with backup-ready true, restore the captured current path from `.bak` via a
   new verified `NamedTempFile` in its parent. First accept an existing path
   only if it already reports the original version; otherwise atomically
   persist the staged recovery copy over it.
9. After recovery, verify the exact installed path reports the original
   version. Remove `.bak`; if removal fails, report restored current plus the
   retained backup. The command remains non-zero because update failed.
10. If recovery fails, retain `.bak` and return one error containing original
    update error, restore error, exact current path, exact backup path, and
    manual-recovery instruction.
11. Add one focused filesystem test with temporary files and a verification
    closure. Prove successor rejection does not change current/backup; verified
    backup replaces the old slot; simulated missing current restores from
    backup; simulated restore failure keeps backup and includes both errors.
12. Run verification and commit:

    ```bash
    rtk git add crates/neo-agent/src/modes/lifecycle.rs
    rtk git diff --cached --check
    rtk git commit -m "feat: add verified self update"
    ```

## Task 4: Implement One-Time Offline Rollback

**Files:**

- Modify: `crates/neo-agent/src/modes/lifecycle.rs`

**Why:** Users need a deterministic way to restore the single version saved by
the last replacement attempt without another network request.

**Change Necessity:** `self_update` selects online releases; offline rollback
must use the verified local `.bak` and direct replacement primitive.

**Impact/Compatibility:** Rollback ignores SemVer order and channel. It never
swaps the current version back into `.bak` and never creates history.

**Verification:**

```bash
rtk cargo nextest run -p neo-agent --bin neo lifecycle::tests::rollback_is_offline_and_consumes_one_backup
rtk cargo nextest run -p neo-agent --bin neo lifecycle::tests::backup_promotion_and_failed_replace_preserve_recovery
```

**Steps:**

1. Reject absent `.bak`, directory, symlink, or non-regular file before any
   replacement.
2. Stage and execute the backup; accept any parseable Neo SemVer, including
   lower/higher/different-channel versions.
3. Create a transient verified staged copy of the current executable. This is
   a recovery guard only and never becomes another canonical backup.
4. Call a small `replace_with_recovery` helper whose production closure invokes
   `self_replace::self_replace(staged_backup.path())`. On replacement or
   post-install version failure, atomically restore the transient current guard
   to the captured path and leave `.bak` unchanged.
5. On successful installed-version verification, remove `.bak`. If removal
   fails, return non-zero with the installed version and exact retained path;
   do not undo the successful rollback.
6. Return old version, restored version, consumed backup, and restart notice.
7. Add one focused test with an injected replace closure. Prove there is no
   network call, invalid backup is rejected, success consumes once, second
   rollback reports absent backup, and simulated replacement failure restores
   current while retaining `.bak`.
8. Run verification and commit:

   ```bash
   rtk git add crates/neo-agent/src/modes/lifecycle.rs
   rtk git diff --cached --check
   rtk git commit -m "feat: add offline update rollback"
   ```

## Task 5: Implement Guarded Uninstall

**Files:**

- Modify: `crates/neo-agent/src/modes/lifecycle.rs`

**Why:** Uninstall must remove the exact installation without scanning PATH and
must protect non-rebuildable Neo data behind explicit confirmation.

**Change Necessity:** No current command owns binary deletion, Y/N input, or
guarded home deletion. This belongs beside update/rollback lifecycle behavior.

**Impact/Compatibility:** Unix can unlink the running file. Windows direct
`remove_file` surfaces sharing/access denial and stops before `.bak`/home.

**Verification:**

```bash
rtk cargo nextest run -p neo-agent --bin neo lifecycle::tests::uninstall_confirmation_and_partial_order_are_safe
rtk cargo clippy -p neo-agent --bin neo -- -D clippy::all
```

**Steps:**

1. Implement `confirm_delete_home(reader, writer, path)`: print and flush the
   exact prompt; accept case-insensitive `y`/`yes`, `n`/`no`; empty/EOF means
   No; all other input repeats.
2. Implement Neo-home validation from `config::neo_home()` and
   `config::user_home()`: require absolute existing directory, reject symlink,
   canonicalize for comparisons, reject root and canonical user home. Keep the
   original exact path as the deletion target after it passes validation.
3. Resolve and validate all selected paths before mutation. If home is absent,
   skip the prompt. If no home resolves, continue binary uninstall and report
   that state.
4. Remove current with `std::fs::remove_file`. Treat only NotFound as already
   absent; retain underlying permission/sharing errors and stop.
5. Remove the exact adjacent `.bak` entry next. Do not canonicalize/follow it;
   if it is not a regular file/symlink entry that `remove_file` can safely
   remove, return partial state and keep home.
6. Remove the confirmed original Neo-home directory only after both binary
   entries are absent. Report binary-gone/data-retained if recursive deletion
   fails.
7. Never call `self_replace::self_delete`, spawn a child, schedule reboot
   deletion, or invoke a shell.
8. Add one focused test covering prompt parsing, EOF default No, `--yes`, unsafe
   root/home/symlink rejection, current-delete failure blocking both later
   deletions, backup-delete failure retaining home, and home-delete failure
   reporting partial state. Use only temp paths and injected input; never target
   the test runner or real home.
9. Run verification and commit:

   ```bash
   rtk git add crates/neo-agent/src/modes/lifecycle.rs
   rtk git diff --cached --check
   rtk git commit -m "feat: add self uninstall"
   ```

## Task 6: Expose CLI And Early Config-Independent Dispatch

**Files:**

- Modify: `crates/neo-agent/src/cli.rs`
- Modify: `crates/neo-agent/src/main.rs`
- Modify: `crates/neo-agent/src/modes/lifecycle.rs`

**Why:** Lifecycle must remain usable when provider config is missing or broken.

**Change Necessity:** Adding behavior without clap and pre-config dispatch leaves
it unreachable or incorrectly coupled to `AppConfig`.

**Impact/Compatibility:** Existing command match remains exhaustive. Lifecycle
commands are non-TUI. Existing resume/subcommand conflict remains enforced.

**Verification:**

```bash
rtk cargo nextest run -p neo-agent --bin neo lifecycle::tests::cli_lifecycle_contract_is_exact
rtk cargo nextest run -p neo-agent --bin neo lifecycle::tests::rollback_is_offline_and_consumes_one_backup
rtk cargo check -p neo-agent --bin neo
```

**Steps:**

1. Add exactly the two `Command` variants from Frozen Implementation Contract.
2. At the start of `dispatch()`, before `ConfigOverrides`, resume workspace, or
   `AppConfig::load`, keep the existing `--resume` + subcommand rejection and
   return early for `Update`/`Uninstall` by calling lifecycle functions.
3. Add unreachable-in-normal-flow match arms in `dispatch_command()` that bail
   with `lifecycle command must be dispatched before application startup`,
   mirroring the existing process-guard invariant.
4. Do not change `is_interactive_tui_mode`; its `Some(_) => false` already
   classifies both commands correctly.
5. Add one clap test using `Cli::try_parse_from`: all seven valid invocations,
   all three pairwise update-flag conflicts, all-three conflict, and rejection
   of `--rc`. Assert `-y` and `--yes` produce the same `yes = true` semantic
   state in one test rather than duplicate tests.
6. Prove config bypass with a non-mutating missing-backup invocation after
   building the binary:

   ```bash
   NEO_CONFIG=/definitely/missing/neo-config.toml target/debug/neo update --rollback
   ```

   Expected: non-zero missing `.bak` error; no config-load error.
7. Run verification and commit:

   ```bash
   rtk git add crates/neo-agent/src/cli.rs crates/neo-agent/src/main.rs crates/neo-agent/src/modes/lifecycle.rs
   rtk git diff --cached --check
   rtk git commit -m "feat: expose lifecycle commands"
   ```

## Task 7: Document The Complete Lifecycle Contract

**Files:**

- Modify: `docs/en/quickstart.md`
- Modify: `docs/zh/quickstart.md`
- Modify: `README.md`

**Why:** Update channels, backup recovery, destructive confirmation, and the
Windows uninstall limitation are user-facing operational contracts.

**Change Necessity:** Source behavior alone does not tell users how to enter or
leave unstable, consume rollback, or manually handle Windows uninstall.

**Impact/Compatibility:** English and Chinese must describe identical behavior.
README links instead of duplicating the full contract.

**Verification:**

```bash
rtk rg -n 'neo update|--unstable|--stable|--rollback|neo uninstall|--yes|\.bak' docs/en/quickstart.md docs/zh/quickstart.md
rtk git diff --check
```

**Steps:**

1. Add an English `Update, rollback, and uninstall` section covering command
   examples, stable/unstable semantics, one backup path on Unix/Windows,
   automatic recovery, one-time offline rollback, restart notice, and errors.
2. Add uninstall confirmation text, `-y`/`--yes`, `NEO_HOME`, deletion order,
   safe-path guards, Unix running unlink, and Windows occupied `.exe` manual
   cleanup. State that Windows failure leaves `.bak` and Neo home untouched.
3. Add the semantically identical Chinese section; do not merely link English.
4. Add update/uninstall entries to both quickstart cheat sheets.
5. Add one concise README sentence/link pointing to the quickstart lifecycle
   section; do not duplicate matrices or error semantics.
6. Run verification and commit:

   ```bash
   rtk git add README.md docs/en/quickstart.md docs/zh/quickstart.md
   rtk git diff --cached --check
   rtk git commit -m "docs: document lifecycle commands"
   ```

## Task 8: Native Acceptance, Six-Target Proof, And ADR

**Files:**

- Modify only if evidence finds a scoped regression: files already listed above
- Create after all required evidence passes:
  `docs/aegis/adr/ADR-0005-self-update-lifecycle.md`
- Modify via helper: `docs/aegis/INDEX.md`

**Why:** Replacement and running-file deletion behavior cannot be fully proven
from one OS or pure unit tests. The architecture decision should be recorded
only after the dependency assumptions are demonstrated.

**Change Necessity:** Cross-platform proof is required by the approved contract;
the ADR preserves the version/release/backup owner decisions after proof.

**Impact/Compatibility:** Use disposable copied binaries and temporary
`NEO_HOME`. Never update/uninstall a real user installation or real home during
acceptance.

**Verification:** all commands below must produce fresh evidence.

**Steps:**

1. On macOS, run every exact lifecycle unit filter from Verification plus:

   ```bash
   rtk cargo fmt --all --check
   rtk cargo clippy -p neo-agent --bin neo -- -D clippy::all
   rtk git diff --check
   ```

2. Create a disposable install directory with `mktemp -d`, copy the built Neo
   binary there, point `NEO_HOME` at a sibling disposable directory, and verify
   update creates the exact `.bak`, rollback consumes it once, and Unix
   uninstall removes the running copy only after the chosen data answer. Never
   point `NEO_HOME` at the real home.
3. Inspect public metadata without mutation:

   ```bash
   gh api repos/matrixheaven/neo/releases --paginate
   ```

   Confirm the selected stable/prerelease tag, exact host asset, and
   `sha256:` digest exist. Do not use a fuzzy asset match.
4. Check host memory before starting Parallels. Boot only Fedora, copy the repo
   to a disposable path, run the same exact test filters and disposable binary
   update/rollback/uninstall flow, capture evidence, then shut Fedora down with
   `prlctl` before starting Windows.
5. Boot Windows 11 only after Fedora is stopped. In a disposable path run the
   same exact test filters, update/rollback flow, and `neo uninstall -y` from the
   disposable running copy. Require uninstall to return the occupied `.exe`
   error and prove `.bak` and disposable `NEO_HOME` still exist. Shut Windows
   down with `prlctl` after evidence is captured.
6. Exercise automatic recovery on every OS with the test-only injected failure
   boundary. Prove recovery success restores the original version and consumes
   backup; prove dual failure reports both errors and retains backup.
7. Build exactly the six release targets with the same commands/tooling as the
   workflow. Classify any failure against unmodified HEAD/environment before
   editing. Do not weaken a target or delete it to make the matrix green.
   The exact build commands are:

   ```bash
   rtk cargo build --release --target x86_64-unknown-linux-gnu -p neo-agent
   rtk cargo zigbuild --release --target aarch64-unknown-linux-gnu -p neo-agent
   rtk cargo build --release --target x86_64-apple-darwin -p neo-agent
   rtk cargo build --release --target aarch64-apple-darwin -p neo-agent
   rtk cargo build --release --target x86_64-pc-windows-msvc -p neo-agent
   rtk cargo xwin build --release --target aarch64-pc-windows-msvc -p neo-agent
   ```

   Run each command only on a host/toolchain that supports it. If `rtk` is not
   installed inside a disposable VM, run the identical underlying cargo command
   there and record that environment fact.
8. Run the final lingering-boundary scan:

   ```bash
   rtk rg -n -- '--rc|self_delete\(|rollback manifest|timestamped backup|package manager' crates/neo-agent/src .github/workflows docs/en docs/zh README.md
   rtk git diff --check
   ```

   Expected: no `--rc`, no lifecycle `self_delete`, no manifest/history or
   package-manager fallback. Legitimate unrelated historical text must be
   identified, not deleted blindly.
9. Create the ADR only after steps 1-8 pass. Record:
   - Cargo package version as the single runtime/release identity;
   - tag `v{version}` gate and six exact assets;
   - early config-independent lifecycle owner;
   - pinned update/replacement dependencies and verified-before-mutation order;
   - one adjacent backup, exact-path recovery, one-time rollback;
   - confirmation-first Neo-home deletion and explicit Windows uninstall error;
   - rejected package-manager, multi-backup, swap, helper, and delayed-delete
     alternatives;
   - consequences and the signing-key deferred boundary.
10. If `ADR-0005` is already occupied, stop and select the next unused ADR
    number without rewriting another decision. Append the ADR to the Aegis
    index with `aegis-workspace.py append-index`, then run its `check` command.
11. Commit ADR/index only after verification:

    ```bash
    rtk git add docs/aegis/adr/ADR-0005-self-update-lifecycle.md docs/aegis/INDEX.md
    rtk git diff --cached --check
    rtk git commit -m "docs: record lifecycle architecture"
    ```

12. Do not push, tag, create a release, or mutate a branch/worktree without
    explicit user authorization. If remote six-target CI is still required,
    report it as pending evidence rather than claiming release-ready.

## Acceptance Mapping

| Approved criterion | Owning task/evidence |
| --- | --- |
| Banner/`--version` equals RC2 identity | Task 1 |
| Mismatched tag blocked before build | Task 1 validate job |
| Stable/unstable/stable-switch policy | Task 2 selection test |
| Six assets plus v0.1.0 exception | Task 2 mapping test; Task 8 metadata/build |
| Digest and staged version before backup | Task 3 hook/order test |
| Successful update retains exact `.bak` | Task 3; Task 8 disposable flow |
| Failed update restores or preserves recovery evidence | Task 3; Task 8 injected/native evidence |
| Offline rollback consumes once | Task 4; Task 8 disposable flow |
| Unix running uninstall plus prompt | Task 5; Task 8 macOS/Fedora |
| Windows occupied `.exe` leaves backup/home | Task 5; Task 8 Windows |
| `-y`/`--yes` and guarded home | Tasks 5-6 |
| Bilingual matching docs | Task 7 |
| Durable architecture decision | Task 8 ADR/index |

## Risks And Stop Conditions

- Stop if GitHub returns the selected exact asset without a digest; do not
  silently skip integrity verification.
- Stop if `self_update` invokes replacement before Neo's staged verification
  and backup hook; do not fork the library pipeline casually.
- Stop if backup promotion cannot use same-directory atomic persist.
- Stop if Windows failure recovery cannot restore the captured installation
  path from a verified copy; do not add a scheduled/reboot helper.
- Stop if uninstall would delete or schedule deletion of Windows current `.exe`
  instead of surfacing the occupied-file error.
- Stop if safe Neo-home validation would require following an ambiguous
  symlink/junction; preserve the data and report the exact path.
- Stop if implementation requires provider/config/session/TUI changes; that is
  scope drift.
- If a target build fails on both baseline and patch, record the baseline issue
  separately and do not misclassify it as a lifecycle regression.

## Retirement And Anti-Entropy

- Replace all four stale Neo `0.1.1` package identities; do not change unrelated
  dependency versions with the same string.
- The release workflow must have one validation owner and one build matrix, not
  parallel legacy/new release paths.
- The only packaging compatibility branch is exact `v0.1.0` plain Unix.
- Every successful network update overwrites one `.bak`; every successful
  rollback consumes it; every successful automatic restore consumes it.
- Failed dual recovery retains the same `.bak` only for manual repair.
- No updater config, manifest, timestamped backup, helper executable owned by
  Neo, or package-manager branch remains after completion.
- ADR Auto Backfill signal remains active: only create the accepted ADR after
  native/cross-target evidence proves the dependency and platform assumptions.

## Completion Checklist

- [ ] Eight task boundaries completed with focused conventional commits.
- [ ] Spec criteria map to fresh evidence.
- [ ] No product-code edits outside the file map without a documented stop and
  user-approved plan revision.
- [ ] Exact lifecycle tests pass on macOS, Linux, and Windows.
- [ ] Six release targets build or unmodified-baseline failures are separated
  and completion is downgraded accordingly.
- [ ] Disposable update/rollback/uninstall never touches a real installation or
  real Neo home.
- [ ] Parallels VMs used for acceptance are shut down.
- [ ] `cargo fmt`, targeted Clippy, and `git diff --check` pass.
- [ ] English and Chinese docs match.
- [ ] ADR/index committed only after proof.
- [ ] Worktree contains no unrelated staged changes.
