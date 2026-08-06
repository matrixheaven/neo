use super::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

/// Test-only probe bundle: an in-memory filesystem + env view.
struct MockEnv {
    files: HashSet<PathBuf>,
    env: HashMap<String, String>,
    path: String,
}

impl MockEnv {
    fn new() -> Self {
        Self {
            files: HashSet::new(),
            env: HashMap::new(),
            path: String::new(),
        }
    }

    fn file(mut self, path: &str) -> Self {
        self.files.insert(PathBuf::from(path));
        self
    }

    fn env(mut self, key: &str, value: &str) -> Self {
        self.env.insert(key.to_owned(), value.to_owned());
        self
    }

    fn path(mut self, p: &str) -> Self {
        self.path = p.to_owned();
        self
    }

    /// Build owned `'static` probes. Closures clone the mock state so there
    /// is no borrow entanglement with `self`.
    fn probes(&self) -> EnvProbes {
        let files = self.files.clone();
        let env = self.env.clone();
        let path = self.path.clone();
        let files_for_exec = files.clone();
        let path_for_exec = path.clone();
        EnvProbes {
            is_windows: true,
            env_get: Box::new(move |name: &str| {
                if name == "PATH" {
                    (!path.is_empty()).then(|| path.clone())
                } else {
                    env.get(name).cloned()
                }
            }),
            is_file: Box::new(move |p: &Path| files.contains(p)),
            exec_file_text: Box::new(move |_exe: &Path, _args: &[&str]| {
                // Pretend git reports a mingw64 exec-path when a git.exe is
                // present on PATH, so the exec-path inference branch runs.
                if path_for_exec
                    .split(';')
                    .any(|d| files_for_exec.contains(&PathBuf::from(format!(r"{d}\git.exe"))))
                {
                    Some(r"C:\Git\mingw64\libexec\git-core".to_owned())
                } else {
                    None
                }
            }),
        }
    }
}

#[test]
fn windows_neo_shell_path_override_wins() {
    let env = MockEnv::new()
        .file(r"C:\custom\bash.exe")
        .env("NEO_SHELL_PATH", r"C:\custom\bash.exe");
    let env_detected = detect_with(&env.probes()).unwrap();
    assert_eq!(
        env_detected.shell_path,
        PathBuf::from(r"C:\custom\bash.exe")
    );
    assert!(env_detected.is_windows);
}

#[test]
fn windows_kimi_shell_path_is_not_an_override() {
    let env = MockEnv::new()
        .file(r"C:\custom\bash.exe")
        .env("KIMI_SHELL_PATH", r"C:\custom\bash.exe");
    let err = detect_with(&env.probes()).unwrap_err();
    assert!(err.is_windows);
    assert!(!err.checked.iter().any(|path| path == r"C:\custom\bash.exe"));
}

#[test]
fn windows_git_exe_on_path_infers_bash() {
    // git.exe in <root>\cmd → bash at <root>\bin\bash.exe.
    let env = MockEnv::new()
        .file(r"C:\Git\cmd\git.exe")
        .file(r"C:\Git\bin\bash.exe")
        .path(r"C:\Git\cmd");
    let env_detected = detect_with(&env.probes()).unwrap();
    assert_eq!(
        env_detected.shell_path,
        PathBuf::from(r"C:\Git\bin\bash.exe")
    );
}

#[test]
fn windows_hardcoded_fallback_program_files() {
    let env = MockEnv::new().file(r"C:\Program Files\Git\bin\bash.exe");
    let env_detected = detect_with(&env.probes()).unwrap();
    assert_eq!(
        env_detected.shell_path,
        PathBuf::from(r"C:\Program Files\Git\bin\bash.exe")
    );
}

#[test]
fn windows_missing_shell_reports_checked_candidates() {
    let env = MockEnv::new();
    let err = detect_with(&env.probes()).unwrap_err();
    assert!(err.is_windows);
    assert!(err.checked.iter().any(|c| c.contains("Program Files")));
}

#[test]
fn windows_path_to_posix_drive_letter() {
    assert_eq!(
        windows_path_to_posix(Path::new(r"C:\Users\repo")),
        "/c/Users/repo"
    );
    // Bare drive-relative paths like `D:dev` (no separator after the colon)
    // are left untouched — matches docs/kimi-code, which only rewrites
    // `<drive>:` when followed by a separator or end-of-string.
    assert_eq!(windows_path_to_posix(Path::new("D:dev")), "D:dev");
}

#[test]
fn windows_path_to_posix_unc() {
    assert_eq!(
        windows_path_to_posix(Path::new(r"\\server\share\dir")),
        "//server/share/dir"
    );
}

#[test]
fn windows_path_to_posix_forward_slashes_passthrough() {
    assert_eq!(
        windows_path_to_posix(Path::new(r"already/posix")),
        "already/posix"
    );
}

#[test]
fn nul_redirect_rewrites_basic_and_handles_case() {
    assert_eq!(rewrite_windows_nul_redirect("foo > NUL"), "foo > /dev/null");
    assert_eq!(rewrite_windows_nul_redirect("foo 2>nul"), "foo 2>/dev/null");
    assert_eq!(
        rewrite_windows_nul_redirect("foo >> NUL 2>&1"),
        "foo >> /dev/null 2>&1"
    );
}
