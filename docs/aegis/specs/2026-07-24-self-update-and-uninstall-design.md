# Neo Self-Update and Uninstall Design

Date: `2026-07-24`
Status: `approved; implementation planning`
ArchitectureReviewRequired: `yes`

## 1. Summary

Neo adds two lifecycle commands:

- `neo update [--unstable | --stable | --rollback]` updates the running
  installation from GitHub Releases or restores its one local backup.
- `neo uninstall [-y | --yes]` removes the running Neo binary and optionally
  removes the active Neo home directory after explicit confirmation.

The Cargo package version remains the single version source for the banner,
`neo --version`, release tags, and update comparison. The release workflow must
reject a tag that is not exactly `v{package-version}`.

The implementation supports the six release targets already published by Neo:
Linux, macOS, and Windows on x86_64 and ARM64.

## 2. Context and Problem

The current banner and `neo --version` use `CARGO_PKG_VERSION`, currently
`0.1.1`, while the latest published release tag is
`v0.1.1-rc.2+20260721.0634`. The release process allowed these two version
identities to diverge.

Neo already publishes native GitHub Release assets for six targets. It does not
currently expose an update or uninstall command, and normal command dispatch
loads application configuration before running a subcommand.

Two historical asset shapes must be supported:

- `v0.1.0`: plain Unix binaries and both plain/zip Windows binaries.
- `v0.1.1-rc.*`: `.tar.gz` on Linux/macOS and `.zip` on Windows.

GitHub exposes a SHA-256 digest for every current `v0.1.0` and RC2 asset.

## 3. Goals

1. Make the banner, `neo --version`, Cargo package version, and release tag
   represent the same SemVer value.
2. Let stable users update to the newest stable release with `neo update`.
3. Let users explicitly enter the prerelease channel with
   `neo update --unstable`.
4. Let prerelease users explicitly return to the newest stable release with
   `neo update --stable`, including an intentional SemVer downgrade when the
   newest stable version is older.
5. Download, verify, and atomically replace the current executable on Linux,
   macOS, and Windows for both supported architectures.
6. Let `neo uninstall` remove the current executable and optionally remove Neo
   user state with a Y/N confirmation.
7. Leave the installed binary untouched for every pre-commit failure; after
   replacement begins, restore from the verified backup or report exact
   recoverable/partial state without touching user data.
8. Preserve the version replaced by the latest successful update as one
   adjacent `.bak`, automatically restore it if replacement fails, and let the
   user consume it once with `neo update --rollback`.

## 4. Non-Goals

- No background updater, automatic update check, daemon, scheduler, or TUI
  notification.
- No arbitrary `--version`, nightly channel, channel persistence, multiple
  backups, backup manifest, or update history.
- No package-manager integration for Cargo, Homebrew, winget, apt, or similar.
- No privilege escalation, `sudo`, UAC prompt, shell script, PowerShell script,
  Neo-authored helper, or uninstall helper. The pinned replacement dependency's
  internal temporary Windows process remains part of its update primitive.
- No deletion of workspaces or files outside the exact resolved Neo home.
- No Windows delayed-delete or delete-on-reboot behavior.
- No compatibility with release asset layouts other than the proven `v0.1.0`
  layout and the current canonical archive layout.

## 5. CLI Contract

### 5.1 Update

```text
neo update
neo update --unstable
neo update --stable
neo update --rollback
```

`--unstable`, `--stable`, and `--rollback` are mutually exclusive; clap rejects
any combination of them.

| Invocation | Target | Downgrade policy |
| --- | --- | --- |
| `neo update` | Newest non-draft stable SemVer release | Never downgrade |
| `neo update --unstable` | Newest non-draft SemVer prerelease | Never downgrade |
| `neo update --stable` | Newest non-draft stable SemVer release | May downgrade only when the running version is a prerelease |

`neo update --rollback` is a separate offline operation. It does not select a
release or apply channel/downgrade policy. It validates and installs the single
adjacent backup created before the latest replacement attempt, then deletes
that backup. If no backup exists, it exits non-zero without changing the
current executable.

Definitions:

- Stable means the SemVer prerelease field is empty.
- Unstable means the SemVer prerelease field is non-empty, including alpha,
  beta, RC, or another valid SemVer prerelease identifier.
- Build metadata does not affect SemVer precedence. Equal precedence is treated
  as already current; the release process must use a new prerelease identifier
  rather than relying on build metadata to order rebuilds.

Specific behavior:

- Running stable `0.1.0`, latest stable `0.1.1`: `neo update` installs `0.1.1`.
- Running stable `0.1.0`, latest prerelease `0.1.1-rc.2`:
  `neo update --unstable` installs the RC.
- Running `0.1.1-rc.2`, latest stable `0.1.0`: `neo update` does not downgrade
  and tells the user that `--stable` is required for an intentional channel
  switch.
- Running `0.1.1-rc.2`, latest stable `0.1.0`:
  `neo update --stable` installs `0.1.0`.
- Running a stable version newer than the latest published stable:
  `neo update --stable` does not downgrade it. The downgrade exception applies
  only when the running version is a prerelease.
- `--unstable` never downgrades a newer stable build to an older prerelease.

An already-current result exits successfully without downloading an asset.
Update runs without an additional confirmation prompt.

The backup path appends `.bak` to the complete executable filename using
`OsString`/`PathBuf`, without assuming UTF-8 or a path separator:

- Unix/macOS: `/path/to/neo` -> `/path/to/neo.bak`;
- Windows: `C:\\path\\neo.exe` -> `C:\\path\\neo.exe.bak`.

Only one backup slot exists. Each successful network update replaces the prior
slot with the version that was current immediately before that update.
Successful manual rollback consumes the slot instead of swapping the two
versions, so repeated rollback cannot toggle versions.

### 5.2 Uninstall

```text
neo uninstall
neo uninstall --yes
neo uninstall -y
```

`neo uninstall` always targets only the executable returned by
`std::env::current_exe()`. It never scans `PATH` and never removes other Neo
copies.

If the active Neo home exists, Neo asks:

```text
Delete Neo data at <exact-path>? [y/N]
```

Accepted input is case-insensitive `y`/`yes` or `n`/`no`; empty input and EOF
mean No. Other input repeats the prompt. `--yes`/`-y` answers Yes to every
confirmation owned by this command; there is one confirmation in this design.

If the active Neo home does not exist, no prompt is shown. If no Neo home can
be resolved, the executable may still be removed and the result states that no
data directory was resolved.

## 6. Canonical Ownership

| Concern | Canonical owner |
| --- | --- |
| Running version | `neo-agent` Cargo package version / `CARGO_PKG_VERSION` |
| Banner version | Existing interactive-mode call sites reading the package version |
| Release identity | Git tag, validated as `v{package-version}` |
| Neo home | Existing `config::neo_home()` (`NEO_HOME`, else platform home + `.neo`) |
| CLI parsing | Existing `cli::Command` enum |
| Lifecycle behavior | One new `modes::lifecycle` module |
| Backup path and lifecycle | The same `modes::lifecycle` module |
| Release artifacts | Existing GitHub Release workflow |

There is no second version file, build-time Git query, version alias, update
config file, or duplicate home-directory resolver.

`Update` and `Uninstall` are dispatched before `AppConfig::load`. A broken or
missing provider configuration must not prevent lifecycle operations.

## 7. Version and Release Contract

### 7.1 Immediate version correction

All four workspace package versions and their lockfile entries move from
`0.1.1` to `0.1.1-rc.2+20260721.0634`. This makes the current banner and
`neo --version` match the latest published release identity, without the
conventional leading `v` used by Git tags.

### 7.2 Release gate

The release workflow adds one validation job before the six build jobs. It
uses a structured TOML parser to verify:

1. all four Neo workspace packages have one identical version;
2. `GITHUB_REF_NAME` equals `v{neo-agent-version}` exactly.

Any mismatch stops the release before platform builds or asset publication.
Future RC and stable releases therefore update Cargo versions before creating
the tag.

## 8. Update Architecture

### 8.1 Dependency choice

Use `self_update = "=1.0.0-rc.6"` with default features disabled and only
`reqwest`, `rustls`, `github`, `async`, `archive-tar`,
`compression-tar-gz`, `archive-zip`, `compression-zip-deflate`, and
`checksums` enabled. Add `semver` directly for Neo's channel-selection policy.

The crate owns download staging, archive extraction, executable permissions,
and cross-platform atomic self-replacement. Neo owns only:

- stable/unstable target selection;
- exact platform asset naming;
- downgrade authorization;
- expected-version verification;
- user-facing output and errors.

This is preferred over reimplementing archive parsing and Windows executable
replacement. The dependency version is pinned because the required 1.0 API is
currently a release candidate.

### 8.2 Release selection

1. Parse `CARGO_PKG_VERSION` as SemVer. Failure is a build defect and aborts.
2. Fetch public releases from `matrixheaven/neo`.
3. Ignore drafts and non-SemVer tags. GitHub public listings already omit
   inaccessible drafts; Neo still treats only parseable `v<semver>` tags as
   candidates.
4. Filter candidates by the selected stable or unstable channel.
5. Choose the highest SemVer precedence. If precedence ties, preserve GitHub's
   newest-first order, but do not install over an equal-precedence running
   version.
6. Apply the downgrade policy from the CLI contract.
7. Pin the updater to the selected exact tag. The dependency must not perform
   a second independent "latest" choice.

If the selected release lacks the exact current-platform asset, the update
fails. Neo does not silently fall back to an older release or another
architecture.

### 8.3 Platform mapping

| Runtime | Canonical archive asset | Binary inside archive |
| --- | --- | --- |
| Linux x86_64 | `neo-linux-x86_64.tar.gz` | `neo-linux-x86_64` |
| Linux aarch64 | `neo-linux-arm64.tar.gz` | `neo-linux-arm64` |
| macOS x86_64 | `neo-macos-x86_64.tar.gz` | `neo-macos-x86_64` |
| macOS aarch64 | `neo-macos-arm64.tar.gz` | `neo-macos-arm64` |
| Windows x86_64 | `neo-windows-x86_64.zip` | `neo-windows-x86_64.exe` |
| Windows aarch64 | `neo-windows-arm64.zip` | `neo-windows-arm64.exe` |

Only `std::env::consts::{OS, ARCH}` drives this mapping. Unsupported operating
systems or architectures fail before network or filesystem mutation.

For the immutable `v0.1.0` release only, Linux/macOS select the proven plain
binary asset with the same suffix and no extension. Windows continues to
prefer its `.zip` asset. This is a bounded external compatibility exception;
future packaging drift is an error rather than a new fallback.

### 8.4 Backup preparation

Backup creation begins only after the downloaded successor has passed digest,
archive-entry, and `--version` verification. Neo copies the current executable
to a sibling staging path such as `<backup>.new`, preserves executable
permissions where applicable, flushes the file, and verifies that the staged
backup is a regular file whose reported version equals the running version.

On Windows, a `.bak` filename is not directly executable. Verification copies
the staged backup to an isolated temporary path ending in `.exe` and executes
that copy with `--version`. The canonical stored backup remains `neo.exe.bak`.

Only after the staged backup is complete and verified does Neo atomically
promote it over the old `.bak` from the same directory. If the platform path
cannot provide this replacement guarantee, implementation must stop for design
review rather than delete the old backup first. If preparing or promoting the
new backup fails, the network update stops before touching the current
executable. Staging-path cleanup failure is reported rather than treated as
update failure recovery.

The backup is a byte copy, not a second version database. Neo stores no
manifest, channel marker, timestamped copies, or mutable update history.

### 8.5 Verification, replacement, and automatic recovery

The mutation order is fixed:

1. Resolve and validate the target release and exact asset.
2. Download into a temporary directory.
3. Require and verify the GitHub asset SHA-256 digest.
4. Extract only the exact expected binary entry, or stage the exact plain
   `v0.1.0` binary.
5. Execute the staged binary with `--version` and require the selected target
   version in its clap output.
6. Prepare, verify, and promote the current executable into its one `.bak`
   slot as described above.
7. Atomically replace `current_exe()` using the dependency's cross-platform
   self-replacement path.
8. Report the old version, new version, selected channel, backup path, and that
   Neo must be restarted.

Any failure before step 7 leaves the current executable untouched. Once a
verified `.bak` has been promoted, any replacement failure triggers one
automatic restore attempt from that exact backup. Neo copies the backup to an
isolated staging executable, validates that exact staged copy, and atomically
restores the exact `current_exe()` installation path captured before replacement.
This path-level restore is required because Windows replacement may already have
moved the running executable aside before returning an error; calling the
self-replacement primitive a second time cannot resolve the now-absent original
path.

If automatic restoration succeeds, Neo removes the now-consumed `.bak` so a
failed update does not create a false manual rollback point. The command still
exits non-zero and reports both the update failure and successful restoration.
If restoration fails, Neo must preserve `.bak`, exit non-zero, and report:

- the original update/replacement error;
- the restoration error;
- the exact current executable and backup paths;
- that manual recovery is required.

Neo never reports update success unless replacement completed. A digest missing
from the selected GitHub asset is a hard error. HTTPS plus the GitHub digest
provides integrity, not independent publisher authenticity; signed release
assets are deferred until Neo has a release-signing key contract.

### 8.6 Manual rollback

`neo update --rollback` performs no network request. It:

1. resolves `current_exe()` and its exact adjacent `.bak`;
2. rejects an absent backup, directory, symlink, or other non-regular file;
3. copies the backup to an isolated staging executable, preserving executable
   permissions where applicable;
4. executes the staged file with `--version` and requires parseable Neo clap
   version output;
5. replaces `current_exe()` through the same cross-platform replacement path;
6. deletes `.bak` only after replacement succeeds;
7. reports the replaced and restored versions and that Neo must be restarted.

There is intentionally no SemVer ordering restriction: rollback means restore
the exact previous installation, whether its version is lower, higher, or on a
different channel. The replacement primitive must leave the current executable
unchanged on failure; `.bak` is not consumed until success. If that guarantee
cannot be proven for a target platform, implementation must stop for design
review. If replacement succeeds but deleting `.bak` fails, Neo reports the
rollback as installed but incompletely consumed, includes the exact remaining
path, and exits non-zero.

The updater never changes `~/.neo`, project files, PATH, shell profiles, or
package-manager metadata.

## 9. Uninstall Architecture

### 9.1 Neo home resolution and guards

The prompt and deletion target use the existing `config::neo_home()`:

- Unix/macOS default: `$HOME/.neo`;
- Windows default: `%USERPROFILE%\.neo`;
- override on every platform: `$NEO_HOME`.

The prompt always prints the exact resolved path. Before recursive deletion,
Neo requires an absolute existing directory, canonicalizes it, and rejects:

- a filesystem/drive root;
- the platform user-home directory itself;
- a non-directory target;
- a path that cannot be canonicalized safely.

This guard applies even with `--yes`.

### 9.2 Mutation order

1. Resolve `current_exe()` and the optional Neo home without mutating either.
2. If the Neo home exists and `--yes` is absent, collect the Y/N decision.
3. Validate all selected deletion targets.
4. Remove the exact current executable.
5. After current-executable removal succeeds, remove its adjacent `.bak` if it
   exists.
6. Only after both executable artifacts are absent, remove the Neo home if the
   user chose Yes.
7. Report each removed, absent, or retained path.

If the executable path is already absent, it is treated as already removed and
the confirmed data cleanup may proceed.

Permission and sharing errors while removing either executable artifact are not
treated as absence. If current-executable removal fails, neither `.bak` nor Neo
home deletion is attempted. If current-executable removal succeeds but `.bak`
removal fails, Neo home deletion is not attempted and the partial result names
the removed current executable and retained backup/data paths. If both binary
artifacts are absent but Neo home deletion fails, Neo reports that the binaries
are gone and the data path remains. Uninstall never creates a new backup.
The `.bak` deletion removes only the exact adjacent directory entry and never
follows a symlink or junction to another target.

### 9.3 Platform behavior

On Linux and macOS, the operating system permits unlinking the running binary.
The current process continues until it prints the result and exits.

On Windows, direct removal of the running `.exe` normally fails with a sharing
or access-denied error. Neo surfaces the exact path and underlying OS error,
states that neither `.bak` nor Neo home data was removed, and exits non-zero.
The user must close Neo and remove the shown executable from another process.

Update succeeds on Windows because it has a verified successor binary and uses
the updater dependency's Windows-specific replacement algorithm. Uninstall has
no successor binary and intentionally does not create a helper process,
scheduled task, reboot-time deletion, or shell script.

## 10. Errors and Output

Expected result outputs are concise and script-readable as plain text:

- already current: current version and selected channel;
- updated: old version, new version, backup path, and restart notice;
- automatically restored: failed target plus restored version and backup
  disposition;
- rolled back: replaced version, restored version, consumed backup, and restart
  notice;
- uninstalled: removed/retained current executable, backup, and Neo home.

Expected errors include the relevant exact path, release tag, asset name, or
platform and retain the underlying source error:

- GitHub unavailable or rate-limited;
- no stable/unstable release exists;
- unsupported OS/architecture;
- missing or ambiguous asset;
- missing/mismatched digest;
- archive entry mismatch;
- staged binary reports the wrong version;
- backup staging, validation, promotion, or cleanup failure;
- executable replacement/removal denied;
- replacement failure followed by automatic-restoration success or failure;
- absent, unsafe, or invalid rollback backup;
- rollback installed but backup consumption failed;
- unsafe Neo home target;
- recursive data deletion failure.

All errors exit non-zero. No error is converted into a silent fallback.

## 11. Implementation Boundary

Expected production edits:

- four crate manifests and `Cargo.lock` for the version correction;
- workspace/agent dependencies for the updater;
- `.github/workflows/release.yml` for the version gate;
- `crates/neo-agent/src/cli.rs` for the two commands and flags;
- `crates/neo-agent/src/main.rs` and `modes/mod.rs` for early dispatch wiring;
- one new `crates/neo-agent/src/modes/lifecycle.rs` owner;
- English and Chinese quickstart/reference documentation.

No core, provider, TUI renderer, session, tool, or configuration-schema change
is required.

## 12. Verification Plan

### 12.1 Focused automated coverage

One focused lifecycle test surface must cover:

- clap accepts all valid commands and rejects every pair of update mode flags;
- stable, unstable, no-downgrade, and explicit prerelease-to-stable downgrade
  selection;
- equal precedence/build-metadata behavior;
- exact six-target asset mapping;
- the bounded `v0.1.0` plain-binary compatibility path;
- unsupported platform rejection;
- digest or staged-version rejection before replacement;
- verified backup promotion occurs only after successor verification;
- replacement failure automatically restores from `.bak`, while dual failure
  preserves `.bak` and reports both errors;
- rollback is offline, rejects invalid/non-regular backups, replaces the current
  executable, and consumes `.bak` once;
- Windows backup validation uses a temporary `.exe` without changing the
  canonical `neo.exe.bak` path;
- Y/N parsing, EOF default No, and `--yes` behavior;
- unsafe Neo home rejection;
- binary-delete failure prevents data deletion;
- backup-delete failure prevents data deletion and reports partial uninstall;
- data-delete failure reports the binary/data partial result.

Tests use temporary executables/directories and injected release metadata; they
must not replace the test runner or contact GitHub.

### 12.2 Cross-platform evidence

Before completion is claimed:

1. run the exact lifecycle test target on macOS;
2. run the same exact target on native Linux;
3. run the same exact target on native Windows;
4. build the release binary for all six release targets;
5. on each OS, run a non-mutating current-channel check against the real public
   release listing and confirm the expected asset is selected;
6. verify Unix uninstall against a copied disposable Neo binary and temporary
   `NEO_HOME`;
7. verify Windows uninstall returns the expected occupied-executable error and
   leaves `.bak` and the temporary Neo home untouched;
8. on each OS, update a disposable installation, verify the old binary is the
   adjacent `.bak`, then run one successful offline rollback and verify the
   backup is consumed;
9. inject a replacement failure on each OS and prove automatic restoration;
   separately inject a restoration failure and prove `.bak` remains available.

Parallels VMs used for acceptance must be shut down when no longer needed.

## 13. Documentation

Update both `docs/en` and `docs/zh` with:

- `neo update`, `--unstable`, `--stable`, and offline `--rollback` examples;
- the channel and downgrade matrix;
- supported OS/architecture pairs;
- network, permission, and restart behavior;
- one-slot `.bak` replacement, automatic recovery, one-time rollback, and
  manual-recovery error behavior;
- `neo uninstall`, its data prompt, `-y`/`--yes`, and exact Neo home behavior;
- uninstall cleanup of `.bak`, plus the Windows running-executable limitation
  and manual cleanup expectation.

The README installation section should link to the detailed quickstart rather
than duplicate the full lifecycle contract.

## 14. Alternatives Considered

### A. Pinned self-update dependency plus Neo policy layer (recommended)

Reuses tested archive, permission, checksum, and Windows replacement behavior.
Neo keeps channel and deletion policy explicit. This is the smallest approach
that satisfies the cross-platform safety requirement.

### B. Fully custom reqwest/archive/self-replace pipeline

Provides complete control but duplicates established archive and replacement
logic across three operating systems. Rejected because it creates more code
and a larger failure surface without adding required behavior.

### C. Invoke Cargo or platform package managers

Would require Rust or a specific package manager, would not cover users who
installed GitHub binaries, and would produce different behavior per platform.
Rejected.

For Windows uninstall, detached helper scripts and delete-on-reboot were also
considered and rejected because they introduce delayed hidden mutation and a
second executable/shell owner. The explicit occupied-file error is the selected
contract.

For rollback, swapping the current version back into `.bak`, retaining multiple
timestamped backups, and adding a backup manifest were rejected. One consumed
slot satisfies recovery without turning Neo into a version-history manager.

## 15. Safety and Governance

Anti-Entropy Declaration:

- Deletion Class: executable installation plus optional persistent user state
- Exact Targets: `current_exe()`, its adjacent `.bak`, and the exact guarded
  `config::neo_home()`
- Expected Preserved Behavior: user data remains unless Y/Yes or `--yes`
- Expected Retired Behavior: installed binary, adjacent `.bak`, and explicitly
  confirmed Neo home
- External Boundary Touched: yes, GitHub Release assets and OS executable rules
- Source-of-Truth Data Risk: confirmed for Neo home
- User Confirmation Required: yes at runtime for Neo home, satisfied only by
  Y/Yes or `--yes`

Retirement Decision:

- Path: `confirmation-first` for Neo home; direct explicit command for the
  executable
- Why: Neo home contains sessions, credentials, trust state, skills, and other
  non-rebuildable user data
- Non-edits: no workspace cleanup, no PATH cleanup, no package-manager cleanup

Existence Check:

- Proposed new surface: lifecycle command owner
- Existing owner / reuse candidate: CLI dispatch, Cargo version, release
  workflow, and `config::neo_home()` are reused
- Why existing surface is insufficient: no component currently selects and
  installs releases or performs command-scoped uninstall
- Creation proof: two explicit user-facing lifecycle commands require one
  cohesive implementation owner
- Entropy / retirement impact: one module, one adjacent backup slot, no fallback
  owner, no persisted channel state, manifest, or version history
- Decision: `add-with-proof`

Architecture Integrity Lens:

- Invariant: one version owner, one Neo-home owner, exact asset selection, one
  backup slot, no executable mutation before successor and backup verification
- Canonical owner: Cargo manifests, `config::neo_home()`, release workflow, and
  `modes::lifecycle`
- Responsibility overlap: none; main only wires early dispatch
- Higher-level simplification: reuse the update dependency rather than owning
  platform replacement internals
- Retirement / falsifier: if the dependency cannot prove exact-asset digest
  verification and pre-install binary verification, this design must return to
  review rather than add an unchecked fallback
- Verdict: coherent

## 16. Acceptance Criteria

The design is complete when implementation evidence proves all of the
following:

1. Banner and `neo --version` show `0.1.1-rc.2+20260721.0634` for the corrected
   build.
2. A mismatched release tag is rejected before release assets build.
3. Default update never downgrades.
4. `--unstable` selects only prereleases and never downgrades.
5. `--stable` is the only path that may move a prerelease to an older latest
   stable release.
6. All six current archive assets map exactly; the `v0.1.0` Unix plain assets
   remain usable for the stable downgrade.
7. Digest and staged-version checks happen before backup creation or
   replacement.
8. Every successful network update retains exactly the replaced executable as
   the single adjacent `.bak`.
9. Replacement failure attempts automatic restoration; successful restoration
   returns the original version, while failed restoration preserves `.bak` and
   reports both errors and exact paths.
10. `--rollback` is offline, accepts no channel flag, installs the verified
    `.bak` regardless of SemVer ordering, and consumes it once on success.
11. Unix uninstall removes a disposable running binary and honors the data
   confirmation.
12. Windows uninstall reports the occupied `.exe`, exits non-zero, and removes
    neither `.bak` nor Neo home.
13. Uninstall removes `.bak` after the current executable; a backup-removal
    failure prevents Neo-home deletion and reports partial state.
14. `-y` and `--yes` delete only the guarded active Neo home after both binary
    artifacts are absent.
15. English and Chinese documentation describe the same contract.

## 17. Working Artifacts

TaskIntentDraft:

- Outcome: consistent release identity plus explicit cross-platform update and
  uninstall commands
- Success evidence: acceptance criteria 1-15
- Stop condition: spec approval, then a separate implementation plan
- Non-goals: section 4
- Main risks: wrong channel, unintended downgrade, wrong architecture,
  unchecked executable, failed dual replacement/recovery, stale backup, broad
  data deletion, Windows partial uninstall

BaselineReadSetHint:

- `Cargo.toml` and four crate manifests
- `.github/workflows/release.yml`
- `crates/neo-agent/src/cli.rs`, `main.rs`, and `config/paths.rs`
- English/Chinese quickstart docs
- published `v0.1.0` and `v0.1.1-rc.2` asset metadata

BaselineUsageDraft:

- Required baseline refs: current CLI/config/release owners and published asset
  contract
- Cited in design refs: sections 2, 6, 7, and 8
- Missing refs: none blocking
- Decision: `continue`

ImpactStatementDraft:

- Affected layers: release workflow, CLI parsing/dispatch, one lifecycle mode,
  package metadata, documentation
- Owners preserved: Cargo version, `config::neo_home()`, GitHub Releases
- Invariants: verified-before-replace, one verified backup slot,
  confirmed-before-data-delete, no-downgrade unless explicit stable switch from
  prerelease, one-time offline rollback
- Compatibility: bounded `v0.1.0` asset support only
- Non-goals: no daemon, package manager, helper script, or config schema

Baseline Role Alignment:

- Product / Requirement Baseline: this conversation and the approved command
  semantics
- Architecture / Runtime Boundary Baseline: existing Cargo, CLI, config-path,
  and release-workflow owners
- Result: aligned
- Scope: both
- Next action: user review, then implementation planning

ADR signal: yes. The accepted implementation should record the durable version
source, release asset contract, lifecycle owner, and one-slot backup/rollback
contract after the implementation is verified; this proposed spec alone does
not create an accepted ADR.
