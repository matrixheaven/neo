use super::*;
use std::path::PathBuf;

#[test]
fn unix_detects_bin_bash_first() {
    let probes = EnvProbes {
        is_windows: false,
        env_get: Box::new(|_| None),
        is_file: Box::new(|p: &Path| p == Path::new("/bin/bash")),
        exec_file_text: Box::new(|_, _| None),
    };
    let env = detect_with(&probes).unwrap();
    assert_eq!(env.shell_path, PathBuf::from("/bin/bash"));
    assert!(!env.is_windows);
}

#[test]
fn unix_falls_back_to_sh() {
    let probes = EnvProbes {
        is_windows: false,
        env_get: Box::new(|_| None),
        is_file: Box::new(|_| false),
        exec_file_text: Box::new(|_, _| None),
    };
    let env = detect_with(&probes).unwrap();
    assert_eq!(env.shell_path, PathBuf::from("/bin/sh"));
}
