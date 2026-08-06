//! Cross-platform shell resolution for the Bash/Terminal tools.
//!
//! The shell is always a POSIX `bash`/`sh`. On Windows we locate Git Bash (the
//! canonical POSIX shell that ships with Git for Windows) rather than
//! cmd/PowerShell, so tool input stays POSIX regardless of host OS — no
//! per-OS command-syntax handling. On Unix we resolve `/bin/bash` (or fall
//! back to `/bin/sh`).
//!
//! All probing is a pure function of injected [`EnvProbes`] so the suite runs
//! identically in tests; [`detect_shell_env`] wires in real `std::env`/`std::fs`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;

/// The resolved POSIX shell to invoke for Bash/Terminal commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellEnv {
    /// `true` on Windows (Git Bash). Controls POSIX path/NUL rewriting.
    pub is_windows: bool,
    /// Absolute path to the `bash`/`sh` executable.
    pub shell_path: PathBuf,
}

/// Failure to locate a usable POSIX shell, with the places already checked so
/// the caller can surface an actionable install hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellEnvError {
    /// `true` when detection ran in Windows mode (affects the hint text).
    pub is_windows: bool,
    /// Every candidate path that was probed and rejected.
    pub checked: Vec<String>,
}

impl std::fmt::Display for ShellEnvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_windows {
            write!(
                f,
                "Git Bash was not found on this Windows host. Install Git for Windows from \
                 https://gitforwindows.org/ or set NEO_SHELL_PATH to a bash.exe. Checked: {}.",
                self.checked.join(", ")
            )
        } else {
            write!(
                f,
                "No POSIX shell (bash/sh) was found. Checked: {}.",
                self.checked.join(", ")
            )
        }
    }
}

impl std::error::Error for ShellEnvError {}

/// Injected probes so detection is deterministic in tests. In production use
/// [`EnvProbes::real`] via [`detect_shell_env`]. Owned closures (`'static`)
/// keep construction simple on both paths.
pub struct EnvProbes {
    pub is_windows: bool,
    pub env_get: EnvGet,
    pub is_file: IsFile,
    /// Runs `<exe> --exec-path`-style queries and returns trimmed stdout, or
    /// `None` on any error/timeout. Used only on Windows to ask `git` where it
    /// lives.
    pub exec_file_text: ExecFileText,
}

type EnvGet = Box<dyn Fn(&str) -> Option<String>>;
type IsFile = Box<dyn Fn(&Path) -> bool>;
type ExecFileText = Box<dyn Fn(&Path, &[&str]) -> Option<String>>;

impl EnvProbes {
    /// Production probe bundle backed by `std::env`/`std::fs`/`std::process`.
    pub fn real() -> Self {
        Self {
            is_windows: cfg!(target_os = "windows"),
            env_get: Box::new(real_env_get),
            is_file: Box::new(real_is_file),
            exec_file_text: Box::new(real_exec_file_text),
        }
    }
}

fn real_env_get(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn real_is_file(path: &Path) -> bool {
    path.is_file()
}

fn real_exec_file_text(exe: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new(exe).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Resolve the POSIX shell using real host probes. Cached per-process via
/// [`resolved_shell`] — prefer that in tool hot paths.
pub fn detect_shell_env() -> Result<ShellEnv, ShellEnvError> {
    detect_with(&EnvProbes::real())
}

/// Pure detection over injected probes (testable).
pub fn detect_with(probes: &EnvProbes) -> Result<ShellEnv, ShellEnvError> {
    if probes.is_windows {
        detect_windows(probes)
    } else {
        Ok(detect_unix(probes))
    }
}

fn detect_unix(probes: &EnvProbes) -> ShellEnv {
    let mut checked = Vec::new();
    for candidate in ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"] {
        let path = Path::new(candidate);
        checked.push(candidate.to_owned());
        if (probes.is_file)(path) {
            return ShellEnv {
                is_windows: false,
                shell_path: path.to_path_buf(),
            };
        }
    }
    checked.push("/bin/sh".to_owned());
    ShellEnv {
        is_windows: false,
        shell_path: PathBuf::from("/bin/sh"),
    }
}

fn detect_windows(probes: &EnvProbes) -> Result<ShellEnv, ShellEnvError> {
    let mut checked: Vec<String> = Vec::new();

    // 1. Explicit override.
    if let Some(override_path) = (probes.env_get)("NEO_SHELL_PATH")
        && !override_path.trim().is_empty()
    {
        let path = PathBuf::from(&override_path);
        checked.push(override_path.clone());
        if (probes.is_file)(&path) {
            return Ok(ShellEnv {
                is_windows: true,
                shell_path: path,
            });
        }
    }

    // 2. `git.exe` on PATH → infer `<root>\bin\bash.exe`.
    if let Some(path_env) = (probes.env_get)("PATH") {
        for git_exe in find_executables_on_path("git.exe", &path_env, probes) {
            let git_exe_str = git_exe.to_string_lossy().into_owned();
            if let Some(candidates) = git_bash_candidates_from_git_exe(&git_exe_str) {
                for candidate in candidates {
                    let path = PathBuf::from(&candidate);
                    checked.push(candidate.clone());
                    if (probes.is_file)(&path) {
                        return Ok(ShellEnv {
                            is_windows: true,
                            shell_path: path,
                        });
                    }
                }
            }

            // 3. Ask `git` where its exec-path is, then infer from that.
            if let Some(exec_path) = (probes.exec_file_text)(&git_exe, &["--exec-path"]) {
                for candidate in git_bash_candidates_from_git_exec_path(&exec_path) {
                    let path = PathBuf::from(&candidate);
                    checked.push(candidate.clone());
                    if (probes.is_file)(&path) {
                        return Ok(ShellEnv {
                            is_windows: true,
                            shell_path: path,
                        });
                    }
                }
            }
        }
    }

    // 4. Hardcoded well-known install locations.
    for candidate in hardcoded_windows_candidates(probes) {
        let path = PathBuf::from(&candidate);
        checked.push(candidate.clone());
        if (probes.is_file)(&path) {
            return Ok(ShellEnv {
                is_windows: true,
                shell_path: path,
            });
        }
    }

    Err(ShellEnvError {
        is_windows: true,
        checked,
    })
}

/// Well-known Git-for-Windows install locations, including a `%LOCALAPPDATA%`
/// user-scope install when that env var is set.
fn hardcoded_windows_candidates(probes: &EnvProbes) -> Vec<String> {
    let mut candidates: Vec<String> = [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files\Git\usr\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\usr\bin\bash.exe",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();

    if let Some(local_app_data) = (probes.env_get)("LOCALAPPDATA")
        && !local_app_data.trim().is_empty()
    {
        candidates.push(format!(r"{local_app_data}\Programs\Git\bin\bash.exe"));
        candidates.push(format!(r"{local_app_data}\Programs\Git\usr\bin\bash.exe"));
    }

    candidates
}

/// Split a PATH-style env var and return entries that contain `name`. Mirrors
/// kimi-code `findExecutablesOnPath` (`;` separator, absolute-dir filter on
/// Windows). Existence is checked via the probe's `is_file` so it stays
/// deterministic in tests. Separator/guard follow `probes.is_windows` (this
/// helper is only called from the Windows branch) rather than the host's
/// `cfg!`, so the Windows suite runs identically on a Unix test host.
fn find_executables_on_path(name: &str, path_env: &str, probes: &EnvProbes) -> Vec<PathBuf> {
    let sep = if probes.is_windows { ';' } else { ':' };
    let mut found = Vec::new();
    for raw_dir in path_env.split(sep) {
        let dir = raw_dir.trim();
        if dir.is_empty() {
            continue;
        }
        // On Windows only consider absolute entries (package-manager shims in
        // relative dirs are unreliable). Use a Windows-aware absolute check —
        // `Path::is_relative` is host-dependent and misclassifies `C:\...` as
        // relative on a Unix host. kimi-code applies the same guard.
        if probes.is_windows && !is_absolute_windows_path(dir) {
            continue;
        }
        // Build the candidate with the Windows separator when probing Windows
        // paths, so it matches a Windows-form key regardless of host OS
        // (`Path::join` would use the host separator and mismatch on a Unix
        // test host). On Unix, `Path::join` is correct.
        let candidate = if probes.is_windows {
            PathBuf::from(format!(r"{dir}\{name}"))
        } else {
            Path::new(dir).join(name)
        };
        if (probes.is_file)(&candidate) {
            found.push(candidate);
        }
    }
    found
}

/// Windows-aware absolute-path check (`C:\`, `C:/`, or UNC `\\`), independent
/// of the host OS. Mirrors kimi-code's `isAbsoluteWindowsPath`.
fn is_absolute_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return bytes.len() >= 3 && matches!(bytes[2], b'\\' | b'/');
    }
    path.starts_with(r"\\")
}

/// `<git-exe-dir>` is `<root>\cmd` or `<root>\bin` → bash lives at
/// `<root>\bin\bash.exe` / `<root>\usr\bin\bash.exe`. String-based Windows path
/// ops (`Path::parent` is host-dependent and misparses `\` paths on a Unix
/// host); mirrors kimi-code's `nodePath.win32` usage.
fn git_bash_candidates_from_git_exe(git_exe: &str) -> Option<Vec<String>> {
    let git_exe = normalize_windows_path(git_exe);
    let git_dir = win_dirname(&git_exe)?;
    let dir_name = win_basename(&git_dir).to_lowercase();
    if dir_name != "cmd" && dir_name != "bin" {
        return None;
    }
    let root = win_dirname(&git_dir)?;
    Some(git_bash_candidates_from_root(&root))
}

/// `git --exec-path` usually points at `<root>\mingw64\libexec\...`; climb to
/// the `mingw32`/`mingw64` segment and take its parent as the Git root. Falls
/// back to two levels up if no mingw segment is present.
fn git_bash_candidates_from_git_exec_path(exec_path: &str) -> Vec<String> {
    let normalized = normalize_windows_path(exec_path);
    let parts: Vec<&str> = normalized.split('\\').collect();
    for i in (0..parts.len()).rev() {
        let segment = parts[i].to_lowercase();
        if segment == "mingw32" || segment == "mingw64" {
            let root = parts[..i].join("\\");
            if !root.is_empty() {
                return git_bash_candidates_from_root(&root);
            }
        }
    }
    // Fallback: two levels up.
    let root = win_dirname(&normalized)
        .and_then(|d| win_dirname(&d))
        .unwrap_or_else(|| normalized.clone());
    git_bash_candidates_from_root(&root)
}

fn git_bash_candidates_from_root(root: &str) -> Vec<String> {
    let root = root.trim_end_matches('\\');
    [
        format!(r"{root}\bin\bash.exe"),
        format!(r"{root}\usr\bin\bash.exe"),
    ]
    .to_vec()
}

/// `dirname` for `\`-separated Windows paths, host-independent. Returns the
/// path with the last non-empty segment removed, or `None` if there is no
/// parent (mirrors `nodePath.win32.dirname` for these inputs).
fn win_dirname(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('\\');
    let parts: Vec<&str> = trimmed.split('\\').collect();
    if parts.len() <= 1 {
        return None;
    }
    let parent = parts[..parts.len() - 1].join("\\");
    if parent.is_empty() {
        None
    } else {
        Some(parent)
    }
}

/// `basename` for `\`-separated Windows paths (last non-empty segment).
fn win_basename(path: &str) -> String {
    path.trim_end_matches('\\')
        .rsplit('\\')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_owned()
}

fn normalize_windows_path(path: &str) -> String {
    path.replace('/', r"\")
}

/// Failure to translate a Windows path into a Git Bash POSIX cwd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsCwdError {
    pub path: String,
    pub reason: &'static str,
}

impl std::fmt::Display for WindowsCwdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cannot translate cwd `{}` to Git Bash form: {}",
            self.path, self.reason
        )
    }
}

impl std::error::Error for WindowsCwdError {}

/// Canonical Git Bash working directory derived from an absolute Windows path.
///
/// Only accepts drive-letter (`C:\dir`) or UNC (`\\server\share\dir`) forms.
/// The resulting POSIX path is single-quote-escaped for shell injection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBashCwd {
    posix: String,
}

impl GitBashCwd {
    /// Translate an absolute Windows path into the POSIX form Git Bash expects.
    ///
    /// # Errors
    ///
    /// Returns an error if the path is not absolute or is otherwise unusable
    /// as a Git Bash cwd.
    pub fn new(path: &Path) -> Result<Self, WindowsCwdError> {
        let path_text = path.to_str().ok_or_else(|| WindowsCwdError {
            path: path.to_string_lossy().into_owned(),
            reason: "path is not valid Unicode and cannot be translated to Git Bash",
        })?;
        if path_text.is_empty() {
            return Err(WindowsCwdError {
                path: path_text.to_owned(),
                reason: "path is empty",
            });
        }

        // UNC: must be \\server\share at minimum.
        if path_text.starts_with(r"\\") {
            let trimmed = path_text.trim_start_matches('\\');
            let parts: Vec<&str> = trimmed.split('\\').collect();
            if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
                return Err(WindowsCwdError {
                    path: path_text.to_owned(),
                    reason: "UNC path must include server and share",
                });
            }
            return Ok(Self {
                posix: windows_path_to_posix(path),
            });
        }

        // Drive letter: must be `C:\` or `C:/` (not bare `C:`).
        let bytes = path_text.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            if bytes.len() < 3 || !matches!(bytes[2], b'\\' | b'/') {
                return Err(WindowsCwdError {
                    path: path_text.to_owned(),
                    reason: "drive-relative path is not absolute; use a full path like `C:\\dir`",
                });
            }
            return Ok(Self {
                posix: windows_path_to_posix(path),
            });
        }

        Err(WindowsCwdError {
            path: path_text.to_owned(),
            reason: "path is not a Windows drive or UNC absolute path",
        })
    }

    /// The POSIX cwd string (e.g. `/c/dev/repo` or `//server/share/dir`).
    #[must_use]
    pub fn posix(&self) -> &str {
        &self.posix
    }

    /// The cwd as a single-quote-escaped shell literal suitable for `cd`.
    #[must_use]
    pub fn shell_cd(&self) -> String {
        format!("'{}'", self.posix().replace('\'', "'\\''"))
    }
}

/// Convert a Windows filesystem path to the POSIX form Git Bash expects
/// (`C:\dev\repo` → `/c/dev/repo`, UNC `\\server\share` → `//server/share`).
/// Mirrors kimi-code `windowsPathToPosixPath`.
#[must_use]
pub fn windows_path_to_posix(path: &Path) -> String {
    let path = path.to_string_lossy();

    // UNC: leading `\\` → everything forward-slashed.
    if path.starts_with(r"\\") {
        return path.replace('\\', "/");
    }

    // Drive letter: `C:` / `C:\` / `C:/` → `/c/...`.
    let bytes = path.as_bytes();
    if bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || matches!(bytes[2], b'\\' | b'/'))
    {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = path[2..].replace('\\', "/");
        let rest = if rest.starts_with('/') {
            rest
        } else {
            format!("/{rest}")
        };
        return format!("/{drive}{rest}");
    }

    path.replace('\\', "/")
}

static WINDOWS_NUL_REDIRECT: LazyLock<Regex> = LazyLock::new(|| {
    // Match a redirect target of `NUL` (any case) preceded by a redirect
    // operator like `>`, `2>`, `>&1>`, `1>>`. Rust's regex crate has no
    // lookahead, so unlike docs/kimi-code's JS `WINDOWS_NUL_REDIRECT` we
    // *capture* the trailing delimiter (group 2) and re-emit it, and allow
    // end-of-string as the terminator so a bare trailing `> NUL` is rewritten.
    // Group 1 is the redirect operator, also re-emitted unchanged.
    Regex::new(r"(\d?&?>+\s*)[Nn][Uu][Ll]([\s|;&)]|$)").expect("valid static regex")
});

/// Rewrite Windows `>NUL` redirects to `>/dev/null` for Git Bash. No-op when
/// there is nothing to rewrite. Mirrors kimi-code `rewriteWindowsNullRedirect`.
#[must_use]
pub fn rewrite_windows_nul_redirect(command: &str) -> String {
    WINDOWS_NUL_REDIRECT
        .replace_all(command, "$1/dev/null$2")
        .into_owned()
}

#[cfg(test)]
#[path = "test_cases/unix.rs"]
mod unix;

#[cfg(test)]
#[path = "test_cases/windows.rs"]
mod windows;

#[cfg(test)]
#[path = "test_cases/git_bash_cwd.rs"]
mod git_bash_cwd;
