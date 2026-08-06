use super::*;
use std::path::PathBuf;

#[test]
fn git_bash_cwd_translates_drive_and_unc() {
    assert_eq!(
        GitBashCwd::new(Path::new(r"C:\Users\repo"))
            .unwrap()
            .posix(),
        "/c/Users/repo"
    );
    assert_eq!(
        GitBashCwd::new(Path::new(r"\\server\share\dir"))
            .unwrap()
            .posix(),
        "//server/share/dir"
    );
}

#[test]
fn git_bash_cwd_rejects_relative_paths() {
    let err = GitBashCwd::new(Path::new("relative/path")).unwrap_err();
    assert!(err.reason.contains("not a Windows drive or UNC"));
}

#[test]
fn git_bash_cwd_rejects_bare_drive_relative() {
    let err = GitBashCwd::new(Path::new("D:dev")).unwrap_err();
    assert!(err.reason.contains("drive-relative"));
}

#[test]
fn git_bash_cwd_rejects_malformed_unc() {
    let err = GitBashCwd::new(Path::new(r"\\server")).unwrap_err();
    assert!(err.reason.contains("UNC path must include"));
}

#[cfg(unix)]
#[test]
fn git_bash_cwd_rejects_non_unicode_path() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let path = PathBuf::from(OsString::from_vec(b"C:\\bad-\xff".to_vec()));
    let err = GitBashCwd::new(&path).unwrap_err();
    assert!(err.reason.contains("not valid Unicode"));
}

#[test]
fn git_bash_cwd_shell_cd_escapes_apostrophes_and_spaces() {
    let cwd = GitBashCwd::new(Path::new(r"C:\Users\O'Reilly\my dir")).unwrap();
    assert_eq!(cwd.shell_cd(), "'/c/Users/O'\\''Reilly/my dir'");
}

#[test]
fn git_bash_cwd_preserves_trailing_separator() {
    let cwd = GitBashCwd::new(Path::new(r"C:\Users\repo\")).unwrap();
    assert_eq!(cwd.posix(), "/c/Users/repo/");
}
