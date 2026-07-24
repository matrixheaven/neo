# ADR-0005: Self-Update Lifecycle

Date: `2026-07-24`
Status: `accepted`

## Context

Neo previously had no mechanism to update, rollback, or uninstall itself. The
banner and `neo --version` used `CARGO_PKG_VERSION`, which diverged from the
published release tag (`v0.1.1-rc.2+20260721.0634`). Users had to manually
download and replace binaries.

## Decision

### Single version source

The Cargo package version is the sole runtime and release identity. Banner,
`neo --version`, and release tag validation all read from `CARGO_PKG_VERSION`.
The release workflow validates that the Git tag equals `v{package-version}`
exactly before building any platform assets.

### Update architecture

- `self_update = "=1.0.0-rc.6"` handles transport, archive extraction,
  checksum verification, and cross-platform self-replacement.
- `self-replace = "=1.5.0"` is a direct dependency for offline rollback and
  recovery (self_update 1.0 no longer re-exports the replacement primitive).
- `semver = "1.0.28"` provides `Version::cmp_precedence` so build metadata
  does not affect release ordering.
- Neo owns: channel selection, exact asset matching, digest requirement,
  staged binary verification, backup timing, and recovery.

### Lifecycle commands

```text
neo update
neo update --unstable
neo update --stable
neo update --rollback
neo uninstall
neo uninstall -y | --yes
```

Commands are dispatched before `AppConfig::load` so broken provider config
cannot block lifecycle operations.

### Channel and downgrade policy

| Invocation | Target | Downgrade |
| --- | --- | --- |
| `neo update` | Latest stable | Never |
| `neo update --unstable` | Latest prerelease | Never |
| `neo update --stable` | Latest stable | Only from prerelease |

Equal precedence (different build metadata) is treated as already current.

### Six-platform asset mapping

| Runtime | Archive | Binary |
| --- | --- | --- |
| Linux x86_64 | `neo-linux-x86_64.tar.gz` | `neo-linux-x86_64` |
| Linux aarch64 | `neo-linux-arm64.tar.gz` | `neo-linux-arm64` |
| macOS x86_64 | `neo-macos-x86_64.tar.gz` | `neo-macos-x86_64` |
| macOS aarch64 | `neo-macos-arm64.tar.gz` | `neo-macos-arm64` |
| Windows x86_64 | `neo-windows-x86_64.zip` | `neo-windows-x86_64.exe` |
| Windows aarch64 | `neo-windows-arm64.zip` | `neo-windows-arm64.exe` |

The immutable `v0.1.0` release uses plain Unix binary assets (no `.tar.gz`).
This is the only historical packaging exception.

### Verified-before-mutate order

1. Select target release (pure policy, no mutation).
2. Download asset, verify GitHub SHA-256 digest.
3. Extract and verify staged binary (`neo --version`).
4. Promote current executable to `.bak` (copy, verify, atomic persist).
5. Atomically replace running binary via `self_replace`.

### One adjacent backup

- Unix: `neo.bak`
- Windows: `neo.exe.bak`
- One slot only; each successful update overwrites the previous backup.
- Successful rollback consumes the backup.
- Successful automatic recovery consumes the backup.
- Dual failure (update + recovery) retains the backup for manual repair.

### Automatic recovery

If replacement fails after backup promotion, Neo restores the captured
installation path from the verified `.bak` using `NamedTempFile::persist`.
On Windows, this avoids a second `self_replace` call (which can fail if the
original path was already moved).

### Offline rollback

`neo update --rollback` performs no network request. It validates the `.bak`,
creates a transient guard copy, replaces via `self_replace`, verifies the
installed version, and consumes `.bak`. If replacement fails, the guard
restores the previous version.

### Uninstall

Deletion order: current executable → `.bak` → Neo home.
Any failure at an earlier step blocks later ones.

- Y/N prompt for Neo home data (case-insensitive, EOF = No).
- `-y`/`--yes` skips prompt only; path safety guards still apply.
- Safety guards: reject symlinks, filesystem roots, user home directory.
- Unix: unlink running binary (process continues).
- Windows: `remove_file` fails for occupied `.exe`; reports error without
  self_delete, helper process, shell script, or delayed delete.

## Alternatives Rejected

- **Fully custom pipeline**: duplicates established archive/replacement logic.
- **Package-manager integration**: different behavior per platform, not
  universal.
- **Multiple backups / manifest / history**: unnecessary complexity.
- **Windows helper / delayed delete**: violates explicit occupied-file
  contract.
- **Swap-style rollback**: rejected in favor of one-time consumed backup.

## Consequences

- Neo users can update from GitHub Releases with integrity verification.
- Failed updates are automatically recovered or leave a manual-recovery
  artifact.
- Uninstall is explicit and guarded.
- Release identity drift is prevented by the workflow gate.
- The pinned `self_update` RC dependency must be tracked for a stable 1.0
  release.
- Signed publisher assets are deferred until Neo has a release-signing key
  contract.

## Verification

Seven focused binary tests cover:

1. CLI contract (all invocations, flag conflicts, `--rc` rejection).
2. Channel selection and downgrade policy (stable, unstable, stable-switch,
   equal precedence).
3. Platform asset mapping (six targets, v0.1.0 exception).
4. Digest validation (single match required, digest required).
5. Backup promotion and recovery (staged verification, promotion, restore
   from missing current, idempotent restore).
6. Offline rollback (guard creation, replace, success consumes .bak,
   simulated failure restores).
7. Uninstall confirmation and safety (Y/N parsing, EOF, `--yes`, symlink/
   root/home rejection, deletion order).

Cross-platform acceptance on macOS (native), Fedora, and Windows is required
before release. Six target builds must pass or baseline failures be separated.
