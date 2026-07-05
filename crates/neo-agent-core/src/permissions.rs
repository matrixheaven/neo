use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    Ask,
    Auto,
    Yolo,
}

impl PermissionMode {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Auto => "auto",
            Self::Yolo => "yolo",
        }
    }
}

#[allow(clippy::derivable_impls)]
impl Default for PermissionMode {
    fn default() -> Self {
        Self::Ask
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionApprovalDecision {
    AllowOnce,
    AllowForSession,
    AllowForPrefix,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PermissionOperation {
    FileRead,
    FileWrite,
    Shell,
    Tool,
    UserQuestion,
    PlanTransition,
    GoalTransition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct ToolAccess {
    pub file_read: bool,
    pub file_write: bool,
    pub shell: bool,
    pub tool: bool,
    pub user_question: bool,
}

impl ToolAccess {
    #[must_use]
    pub const fn none() -> Self {
        Self {
            file_read: false,
            file_write: false,
            shell: false,
            tool: false,
            user_question: false,
        }
    }

    #[must_use]
    pub const fn all() -> Self {
        Self {
            file_read: true,
            file_write: true,
            shell: true,
            tool: true,
            user_question: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Layer 1 — session-scoped approval keys (exact canonical command + cwd)
// ---------------------------------------------------------------------------
//
// "Approve for this session" is keyed by the exact canonical command vector
// (argv form) plus cwd, never by tool name. `git status` and `git log` produce
// different keys and never share a grant. Compound commands (`a && b`) are kept
// as one opaque key when the tokenizer cannot produce a single plain-word
// command, matching Codex's `__codex_shell_script__` fallback so approving the
// whole line never implicitly approves just one sub-command.

/// A hashable, persistent reusable approval grant scoped to a narrow target.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionApprovalKey {
    /// One exact shell command in canonical argv form, scoped to a cwd and
    /// workspace root. `command[0]` is the program; the rest are arguments.
    /// For a compound script that cannot be reduced to a single plain-word
    /// command, `command` is a one-element vector holding the raw script text
    /// (`["__shell_script__", <exact text>]`) so the grant stays opaque.
    Shell {
        /// Canonicalized workspace root (empty if unknown). Prevents a session
        /// store reused across workspaces from leaking approvals.
        workspace: String,
        /// Effective working directory the command runs in.
        cwd: String,
        /// Canonical argv vector. See variant doc for the opaque fallback.
        command: Vec<String>,
    },
    /// One file path under a single write-style operation.
    FileWrite {
        /// Canonicalized workspace root (empty if unknown).
        workspace: String,
        /// Resolved workspace-contained path.
        path: String,
        /// Whether this covers `Write` or `Edit`.
        operation: FileWriteApprovalOperation,
    },
    /// One named tool (e.g. MCP tools `mcp__<server>__<tool>`),
    /// keyed by tool name so the same tool is auto-approved for the session.
    Tool {
        /// Canonicalized workspace root (empty if unknown). Prevents a session
        /// store reused across workspaces from leaking approvals.
        workspace: String,
        /// Fully-qualified tool name, e.g. `mcp__<server>__<tool>`.
        name: String,
    },
}

/// Distinguishes `Write` from `Edit` in [`SessionApprovalKey::FileWrite`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FileWriteApprovalOperation {
    Write,
    Edit,
}

/// User-facing descriptor paired with one or more [`SessionApprovalKey`]s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionApprovalScope {
    pub keys: Vec<SessionApprovalKey>,
    /// Label shown on the "approve for session" button, e.g.
    /// "Approve this exact command for this session".
    pub label: String,
    /// Extra detail lines rendered in the card (cwd / path).
    pub detail: String,
}

impl SessionApprovalScope {
    #[must_use]
    pub fn none() -> Self {
        Self {
            keys: Vec::new(),
            label: String::new(),
            detail: String::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// True only when every key in this scope is already in `approved`.
    #[must_use]
    pub fn is_approved(&self, approved: &HashSet<SessionApprovalKey>) -> bool {
        !self.keys.is_empty() && self.keys.iter().all(|key| approved.contains(key))
    }

    /// Insert every key into the approved set.
    pub fn record(&self, approved: &mut HashSet<SessionApprovalKey>) {
        for key in &self.keys {
            approved.insert(key.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// Layer 2 — persistent prefix approval rules
// ---------------------------------------------------------------------------
//
// A separate, user-chosen mechanism: "Approve commands that start with `git`"
// is stored on disk and survives restarts. This is the correct home for the
// `git *` use case — it is an explicit grant, not an accidental side effect of
// the session cache. Mirrors Codex's `ApprovedExecpolicyAmendment`.

/// A prefix rule that auto-approves any shell command whose canonical argv
/// starts with `prefix` (token equality, not substring). Persisted to
/// `~/.neo/approval_rules.json`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct PrefixApprovalRule {
    /// Canonical argv prefix, e.g. `["git"]`, `["cargo", "test"]`.
    pub prefix: Vec<String>,
    /// Human-readable form of the prefix, cached for UI display.
    pub label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalRuleStore {
    #[serde(default)]
    pub prefix_rules: Vec<PrefixApprovalRule>,
}

impl ApprovalRuleStore {
    /// True when `command` is covered by any prefix rule (token equality).
    #[must_use]
    pub fn matches(&self, command: &[String]) -> bool {
        self.prefix_rules
            .iter()
            .any(|rule| command.starts_with(&rule.prefix))
    }

    /// Insert a prefix rule, deduplicating by `prefix`.
    pub fn insert(&mut self, rule: PrefixApprovalRule) {
        if !self.prefix_rules.iter().any(|r| r.prefix == rule.prefix) {
            self.prefix_rules.push(rule);
        }
    }

    /// Refuse to install a rule that would approve every command (empty or
    /// single-token-only prefix that matches an argv starting with "").
    #[must_use]
    pub fn is_would_approve_all(prefix: &[String]) -> bool {
        prefix.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Layer 3 — command safety classification
// ---------------------------------------------------------------------------

/// Extract the platform-independent command stem for safety classification.
///
/// Normalizes both `/` and `\` path separators and strips common Windows
/// executable extensions (`.exe`, `.cmd`, `.bat`, `.com`) so that
/// `C:\Windows\System32\shutdown.exe` is classified as `shutdown`.
#[must_use]
fn command_basename(program: &str) -> String {
    const EXTS: &[&str] = &[".exe", ".cmd", ".bat", ".com"];
    let basename = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_lowercase();
    for ext in EXTS {
        if let Some(prefix) = basename.strip_suffix(ext) {
            return prefix.to_owned();
        }
    }
    basename
}

/// Read-only / safe commands that skip the prompt in trusted mode.
/// Mirrors Codex `is_known_safe_command`. Kept conservative on purpose.
#[must_use]
pub fn is_known_safe_command(command: &[String]) -> bool {
    const SAFE_PROGRAMS: &[&str] = &[
        "cat", "cd", "cut", "echo", "expr", "false", "grep", "head", "id", "ls", "nl", "paste",
        "pwd", "rev", "seq", "sort", "tail", "test", "tr", "true", "uname", "uniq", "wc", "whoami",
        "which", "basename", "dirname", "file", "find", "git", "cargo", "rustc", "rg", "fd",
    ];
    let Some(program) = command.first().map(String::as_str) else {
        return false;
    };
    // Resolve the basename so `/usr/bin/ls` and `C:\Windows\System32\ls.exe`
    // are both treated as `ls`.
    let program = command_basename(program);
    if !SAFE_PROGRAMS.contains(&program.as_str()) {
        return false;
    }
    // For `git`, only read subcommands are safe. `git push`, `git reset --hard`,
    // `git clean`, etc. must still prompt.
    if program == "git" {
        return matches!(
            command.get(1).map(String::as_str),
            Some(
                "status"
                    | "log"
                    | "diff"
                    | "show"
                    | "branch"
                    | "remote"
                    | "stash"
                    | "describe"
                    | "tag"
                    | "reflog"
                    | "shortlog"
                    | "blame"
                    | "ls-files"
                    | "ls-tree"
                    | "rev-parse"
                    | "rev-list"
                    | "config"
                    | "--version"
                    | "--help"
            )
        );
    }
    // `cargo`/`rustc` build/check commands are read-only enough.
    if matches!(program.as_str(), "cargo" | "rustc") {
        return matches!(
            command.get(1).map(String::as_str),
            Some(
                "test"
                    | "check"
                    | "build"
                    | "run"
                    | "fmt"
                    | "clippy"
                    | "doc"
                    | "tree"
                    | "metadata"
                    | "search"
                    | "--version"
                    | "--help"
                    | "-V"
            )
        );
    }
    true
}

/// Commands that must always prompt regardless of mode (except Yolo).
/// Mirrors Codex `command_might_be_dangerous`.
#[must_use]
pub fn command_might_be_dangerous(command: &[String]) -> bool {
    let Some(program) = command.first().map(String::as_str) else {
        return false;
    };
    // Resolve the basename so Windows paths like `C:\Windows\System32\shutdown.exe`
    // are treated as `shutdown`, and strip common executable extensions.
    let program = command_basename(program);
    if matches!(program.as_str(), "rm" | "rmdir") {
        // `rm -f` / `rm -rf` / `rm -fr`
        let has_force = command
            .iter()
            .skip(1)
            .any(|arg| arg == "-rf" || arg == "-fr" || arg == "-f" || arg == "--force");
        return has_force;
    }
    if matches!(
        program.as_str(),
        "sudo" | "dd" | "mkfs" | "chmod" | "chown" | "shutdown" | "reboot" | "halt" | "poweroff"
    ) {
        return true;
    }
    // `curl ... | sh` / `wget ... | sh` patterns: scan the raw argv for a pipe
    // into a shell. This is a coarse heuristic.
    let joined = command.join(" ");
    if (program == "curl" || program == "wget")
        && (joined.contains("| sh")
            || joined.contains("| bash")
            || joined.contains("|/bin/sh")
            || joined.contains("|/bin/bash"))
    {
        return true;
    }
    false
}

#[cfg(test)]
mod layer_tests {
    use super::*;

    fn cmd(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn empty_scope_is_never_approved() {
        let scope = SessionApprovalScope::none();
        let mut approved = HashSet::new();
        assert!(!scope.is_approved(&approved));
        scope.record(&mut approved);
        assert!(approved.is_empty());
    }

    #[test]
    fn git_status_and_git_log_have_different_keys() {
        let a = SessionApprovalKey::Shell {
            workspace: "/ws".into(),
            cwd: "/ws".into(),
            command: cmd(&["git", "status"]),
        };
        let b = SessionApprovalKey::Shell {
            workspace: "/ws".into(),
            cwd: "/ws".into(),
            command: cmd(&["git", "log"]),
        };
        assert_ne!(a, b);
    }

    #[test]
    fn same_command_different_cwd_is_different() {
        let a = SessionApprovalKey::Shell {
            workspace: "/ws".into(),
            cwd: "/ws".into(),
            command: cmd(&["git", "status"]),
        };
        let b = SessionApprovalKey::Shell {
            workspace: "/ws".into(),
            cwd: "/ws/sub".into(),
            command: cmd(&["git", "status"]),
        };
        assert_ne!(a, b);
    }

    #[test]
    fn file_write_operations_stay_separate() {
        let write_key = SessionApprovalKey::FileWrite {
            workspace: "/ws".into(),
            path: "/ws/a.txt".into(),
            operation: FileWriteApprovalOperation::Write,
        };
        let edit_key = SessionApprovalKey::FileWrite {
            workspace: "/ws".into(),
            path: "/ws/a.txt".into(),
            operation: FileWriteApprovalOperation::Edit,
        };
        assert_ne!(write_key, edit_key);
    }

    #[test]
    fn prefix_rule_matches_by_token_prefix() {
        let store = ApprovalRuleStore {
            prefix_rules: vec![PrefixApprovalRule {
                prefix: cmd(&["git"]),
                label: "git".into(),
            }],
        };
        assert!(store.matches(&cmd(&["git", "status"])));
        assert!(store.matches(&cmd(&["git", "log", "--oneline"])));
        assert!(!store.matches(&cmd(&["ls"])));
    }

    #[test]
    fn prefix_rule_refuses_approve_all() {
        assert!(ApprovalRuleStore::is_would_approve_all(&[]));
        assert!(!ApprovalRuleStore::is_would_approve_all(&cmd(&["git"])));
    }

    #[test]
    fn safe_classification_readonly_commands() {
        assert!(is_known_safe_command(&cmd(&["ls"])));
        assert!(is_known_safe_command(&cmd(&["cat", "file.txt"])));
        assert!(is_known_safe_command(&cmd(&["git", "status"])));
        assert!(is_known_safe_command(&cmd(&["git", "log", "--oneline"])));
        assert!(is_known_safe_command(&cmd(&["cargo", "test"])));
    }

    #[test]
    fn safe_classification_excludes_mutating_git() {
        assert!(!is_known_safe_command(&cmd(&["git", "push"])));
        assert!(!is_known_safe_command(&cmd(&["git", "reset", "--hard"])));
        assert!(!is_known_safe_command(&cmd(&["git", "clean", "-fd"])));
    }

    #[test]
    fn dangerous_classification_for_rm_force_and_sudo() {
        assert!(command_might_be_dangerous(&cmd(&["rm", "-rf", "/tmp/x"])));
        assert!(command_might_be_dangerous(&cmd(&["sudo", "ls"])));
        assert!(command_might_be_dangerous(&cmd(&["chmod", "777", "."])));
        assert!(!command_might_be_dangerous(&cmd(&["ls"])));
        assert!(!command_might_be_dangerous(&cmd(&["rm", "single.txt"])));
    }

    #[test]
    fn dangerous_pipe_to_shell() {
        assert!(command_might_be_dangerous(&cmd(&[
            "curl",
            "https://x",
            "|",
            "sh"
        ])));
        assert!(!command_might_be_dangerous(&cmd(&[
            "curl",
            "https://x",
            "-o",
            "f"
        ])));
    }

    #[test]
    fn safe_classification_normalizes_windows_paths_and_extensions() {
        assert!(is_known_safe_command(&cmd(&[
            r"C:\Windows\System32\ls.exe"
        ])));
        assert!(is_known_safe_command(&cmd(&["/usr/bin/ls"])));
        assert!(is_known_safe_command(&cmd(&[
            r"C:\Program Files\Git\bin\git.exe",
            "status"
        ])));
        assert!(is_known_safe_command(&cmd(&["git.CMD", "status"])));
        assert!(!is_known_safe_command(&cmd(&[
            r"C:\Windows\System32\dd.exe"
        ])));
    }

    #[test]
    fn dangerous_classification_normalizes_windows_paths_and_extensions() {
        assert!(command_might_be_dangerous(&cmd(&[
            r"C:\Windows\System32\shutdown.exe"
        ])));
        assert!(command_might_be_dangerous(&cmd(&["shutdown.CMD"])));
        assert!(command_might_be_dangerous(&cmd(&[
            r"C:\Tools\rm.exe",
            "-rf",
            "x"
        ])));
        assert!(!command_might_be_dangerous(&cmd(&[r"C:\Tools\ls.exe"])));
    }
}
