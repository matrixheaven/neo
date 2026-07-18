# Shell OS Sandbox

## Status

Approved design for adding an operating-system-enforced sandbox to Neo's Bash
and Terminal command children. This design builds on the guardian architecture
from `2026-07-15-supervised-shell-execution-design.md` and does not change the
guardian ownership model.

## Context

Neo currently validates whether shell access is allowed and constrains the
requested working directory through `WorkspaceAccessPolicy`. Once Bash or
Terminal is approved, however, the command still runs with the full authority
of the Neo user account. A command can change directory, redirect output, open
network sockets, or invoke another program outside the workspace.

The process guardian already provides the correct enforcement boundary. It owns
the child process tree, output, resource sampling, deadlines, status artifacts,
and cleanup. The sandbox should restrict only the command child. The guardian
must remain outside the sandbox so it can continue to supervise and clean up the
restricted process.

## Goals

- Enforce workspace write boundaries in the operating system rather than relying
  on command inspection or prompt instructions.
- Disable command-child network access by default.
- Apply one sandbox contract to Bash and Terminal.
- Preserve guardian process-tree ownership, resource limits, output draining,
  and create-once final status behavior.
- Use the existing `WorkspaceAccessPolicy` as the canonical source of primary
  and secondary workspace roots.
- Support macOS, Linux, and Windows with fail-closed platform backends.
- Keep `ask`, `auto`, and `yolo` as the only user-facing permission modes.
- Allow an `ask` approval to grant narrowly scoped network or write capability
  to one execution.

## Non-Goals

- Restricting file reads in the first version.
- Persisting an approved external write path as a workspace automatically.
- Session-wide sandbox capability grants.
- Inferring network or filesystem requirements by parsing shell commands.
- Container, VM, or remote-execution backends.
- Terminal reattachment after Neo restarts.
- Silently falling back to unsandboxed execution.

## Permission-Mode Semantics

Sandbox behavior follows the existing permission mode:

| Permission mode | Default command | Extra capability request |
| --- | --- | --- |
| `ask` | Run in the default sandbox | Ask once; expand only this execution's policy |
| `auto` | Run in the default sandbox | Reject without prompting |
| `yolo` | Run unsandboxed | No capability request is required |

`yolo` retains its current unrestricted meaning. Sandbox availability is not a
prerequisite for `yolo` execution.

## Architecture

```text
ToolDispatch
  |-- resolve permission mode
  |-- resolve the one-shot capability request
  |-- read WorkspaceAccessPolicy roots
  `-- build SandboxPolicy
          |
          v
GuardianClient / StartRequest
          |
          v
Host guardian
  |-- write running/final status and logs
  |-- enforce deadlines and resource limits
  |-- own process group / Windows Job
  `-- SandboxLauncher::spawn(...)
          |
          v
Sandboxed shell or PTY child and all descendants
```

The guardian is the only component that selects and invokes a platform backend.
Bash and Terminal do not contain platform-specific sandbox branches.

## Policy Types

The tool schema exposes an optional request only when extra capability is
needed:

```rust
struct SandboxCapabilityRequest {
    network: bool,
    writable_paths: Vec<PathBuf>,
    justification: String,
}
```

After permission resolution, ToolDispatch produces the complete policy consumed
by the guardian:

```rust
enum SandboxMode {
    Disabled,
    Restricted,
}

enum SandboxNetworkPolicy {
    Disabled,
    Enabled,
}

struct SandboxPolicy {
    mode: SandboxMode,
    writable_roots: Vec<PathBuf>,
    temp_dir: PathBuf,
    network: SandboxNetworkPolicy,
}
```

The guardian never receives the permission mode, approval result, permission
rule, or user justification. It receives only the resolved policy.

## Writable Roots

Default writable roots are:

1. The primary workspace.
2. Every enabled `/add-workspace` root whose `WorkspaceAccessRoot.write` value is
   `true`.
3. A task-specific temporary directory created under the guardian runtime.

`WorkspaceAccessPolicy::roots()` is the only source of workspace roots. The
sandbox does not maintain a second workspace allowlist.

An approved one-shot `writable_paths` request adds roots for one execution. It
does not modify `/add-workspace`, project configuration, or later commands.

All roots are canonicalized before the guardian starts. Resolution must:

- reject a path whose existing canonical parent is not the approved root;
- reject symlink-based escapes;
- remove duplicates;
- remove child roots already covered by a writable parent root; and
- preserve the root's read/write decision from `WorkspaceAccessPolicy`.

For a missing target, authorization applies to the existing canonical parent,
not to an unchecked lexical path.

## Tool Interface

Bash accepts the optional `sandbox` capability request for foreground and
background execution. Terminal accepts it only for `start`; `write`, `read`,
`resize`, and `stop` use the immutable policy established at startup.

Example:

```json
{
  "command": "cargo install cargo-nextest",
  "sandbox": {
    "network": true,
    "writable_paths": ["/Users/me/.cargo"],
    "justification": "Download and install a Cargo tool"
  }
}
```

The first version supports only one-shot approval. A Terminal that needs broader
capability must be stopped and restarted because the selected OS backends do not
provide a safe portable way to widen a running process's policy.

## Platform Backends

### macOS

The macOS launcher generates a Seatbelt profile per execution. The profile:

- permits host filesystem reads;
- permits writes only below the resolved writable roots and task temp;
- denies network access unless the policy enables it; and
- applies to the shell or PTY child and all descendants.

Failure to generate or apply the profile is a sandbox setup failure. The command
must not run without the profile in `ask` or `auto` mode.

### Linux

The Linux launcher uses a Neo-owned sandbox helper. Before executing the shell,
the helper applies:

- `no_new_privs`;
- Landlock filesystem write rules for the resolved writable roots; and
- seccomp rules that block Internet networking when network is disabled.

The restrictions survive `exec` and are inherited by descendants. Neo does not
fall back to an unconfined command when the running kernel cannot enforce the
required Landlock or seccomp policy.

### Windows

The Windows launcher combines the existing Job Object ownership with a restricted
token. Per-root capability SIDs and ACLs grant writes to the approved roots while
the normal user token continues to provide host read access.

Network-disabled execution uses a per-user offline sandbox identity with an
outbound firewall rule. Network-enabled execution uses the corresponding online
identity. A one-time elevated setup creates and verifies the identities, base
capabilities, and firewall rules.

Root-specific ACL provisioning occurs when a writable `/add-workspace` root is
added and is verified again at command launch. If the current user cannot apply
the required ACL to a root, that root cannot be granted to the sandbox.

The user-facing setup surface is:

```text
neo sandbox setup
neo sandbox status
```

Normal command execution does not prompt for elevation. Missing, outdated, or
ineffective setup makes restricted execution fail closed with an actionable
error directing the user to `neo sandbox setup`.

## Guardian Protocol

`StartRequest` gains a required `sandbox_policy` field. It has no default and no
legacy decode path. Guardian and client are always spawned from the same Neo
executable, so retaining an old frame shape would add complexity without a valid
runtime use case.

After the backend is established, `Started` includes:

```rust
enum SandboxBackend {
    MacosSeatbelt,
    LinuxLandlockSeccomp,
    WindowsRestrictedToken,
}

struct SandboxExecutionInfo {
    active: bool,
    backend: Option<SandboxBackend>,
    writable_roots: Vec<PathBuf>,
    network_enabled: bool,
}
```

`ToolResult.details` exposes whether sandboxing is active, the backend, network
state, and writable-root count. It does not duplicate every absolute root into
the transcript.

The complete execution info is stored in the running and final status artifacts
for local audit and recovery.

## Error Model

Errors are separated by ownership boundary:

- `SandboxPermissionDenied`: ToolDispatch rejected requested capability before
  starting a guardian.
- `SandboxUnavailable`: the platform, kernel, or Windows setup cannot enforce the
  requested policy.
- `SandboxSetupFailed`: the backend is supported but profile, token, ACL, helper,
  or child creation failed.
- Runtime sandbox violation: the OS rejects an operation after the child starts.
  This remains a normal command failure with the child's exit code and stderr.

A setup failure after the running marker is committed writes a create-once failed
final status containing `sandbox_error`. It does not send `Started`, register a
background task, or create a Terminal handle. Any partial child, process group,
Job, temp directory, or platform setup is cleaned before returning the typed
start failure.

No new `GuardStatusKind` is added. A backend setup problem is a typed start
failure, while the durable task status remains `failed` with structured sandbox
details.

## Temporary Directory Lifecycle

Each guardian creates a unique task temp directory before launching the child.
The child receives platform-standard temp environment variables pointing to this
directory. The directory is writable inside the sandbox and is deleted after the
guardian has drained output and written final status.

If cleanup fails, the path and error are recorded in `cleanup_errors`; the runtime
scavenger may remove it only after a valid final status exists.

## Testing

### Policy tests

- primary and secondary workspace roots are included correctly;
- read-only secondary roots are excluded from writes;
- overlapping roots are minimized;
- symlink and missing-parent escapes are rejected;
- `ask`, `auto`, and `yolo` resolve to the approved semantics;
- one-shot roots do not mutate `WorkspaceAccessPolicy`; and
- network is disabled by default.

### Guardian and protocol tests

- `StartRequest` requires the new policy;
- backend execution info is returned in `Started`;
- setup failure never returns `Started`;
- running and final status contain sandbox execution details;
- setup failure does not register a background task or Terminal handle; and
- existing parent-EOF, timeout, Stop, and guardian-loss cleanup still work.

### Platform integration tests

On macOS, Linux, and Windows, both Bash and Terminal must prove:

- writes succeed in the primary workspace;
- writes succeed in a writable `/add-workspace` root;
- writes fail outside approved roots;
- task-temp writes succeed;
- descendant processes inherit restrictions;
- default Internet network access fails;
- approved network access succeeds; and
- process-tree cleanup remains effective after timeout, Stop, and guardian loss.

Windows tests additionally verify capability ACL provisioning, offline firewall
behavior, setup-version validation, and Job Object containment. Tests use local
listeners and deterministic filesystem fixtures rather than public network
services.

## Acceptance Criteria

The feature is complete when:

1. Bash and Terminal share one typed policy and platform launcher boundary.
2. `ask` and `auto` never execute unsandboxed after backend setup failure.
3. `yolo` preserves existing unrestricted behavior.
4. Primary and writable secondary workspace roots work without separate sandbox
   configuration.
5. Unauthorized writes and default network access are rejected by the OS on all
   three target platforms.
6. Existing guardian resource, output, persistence, and process-tree tests remain
   valid.
7. Windows setup is performed once, verified on every restricted launch, and does
   not require elevation per command.
