//! paths behavior (moved from `mod.rs`).

use super::*;
use std::path::PathBuf;

#[test]
fn config_loads_system_prompt_file_with_tilde_expansion() {
    let (_temp, config_path, project_dir) =
        temp_project_config("system_prompt_file = \"~/neo-system.md\"\n");
    let home = std::env::var_os("HOME").map(PathBuf::from).expect("home");

    let config = load_config(config_path, project_dir);

    assert_eq!(config.system_prompt_file, Some(home.join("neo-system.md")));
}

#[test]
fn tilde_expansion_uses_user_home_semantics() {
    let home = PathBuf::from("/home/alice");

    assert_eq!(
        super::super::expand_user_path_with_home(PathBuf::from("~/neo-sessions"), Some(&home)),
        PathBuf::from("/home/alice/neo-sessions")
    );
    assert_eq!(
        super::super::expand_user_path_with_home(PathBuf::from("relative/path"), Some(&home)),
        PathBuf::from("relative/path")
    );
}

#[test]
fn tilde_expansion_accepts_windows_separator() {
    let home = PathBuf::from(r"C:\Users\Alice");

    assert_eq!(
        super::super::expand_user_path_with_home(PathBuf::from(r"~\neo-sessions"), Some(&home)),
        home.join("neo-sessions")
    );
}

#[test]
fn neo_home_prefers_neo_home_env() {
    temp_env::with_var("NEO_HOME", Some("/custom/neo"), || {
        assert_eq!(super::super::neo_home(), Some(PathBuf::from("/custom/neo")));
    });
}

#[test]
#[cfg(not(windows))]
fn neo_home_uses_home_on_unix() {
    temp_env::with_vars(
        [("NEO_HOME", None::<&str>), ("HOME", Some("/home/alice"))],
        || {
            assert_eq!(
                super::super::neo_home(),
                Some(PathBuf::from("/home/alice/.neo"))
            );
            assert_eq!(
                super::super::user_home(),
                Some(PathBuf::from("/home/alice"))
            );
        },
    );
}

#[test]
#[cfg(windows)]
fn neo_home_uses_userprofile_on_windows() {
    temp_env::with_vars(
        [
            ("NEO_HOME", None::<&str>),
            ("USERPROFILE", Some(r"C:\Users\Alice")),
            ("HOME", None::<&str>),
        ],
        || {
            assert_eq!(
                super::super::neo_home(),
                Some(PathBuf::from(r"C:\Users\Alice\.neo"))
            );
            assert_eq!(
                super::super::user_home(),
                Some(PathBuf::from(r"C:\Users\Alice"))
            );
        },
    );
}

#[test]
fn default_config_path_is_none_when_home_unresolvable() {
    temp_env::with_vars(
        [
            ("NEO_HOME", None::<&str>),
            ("HOME", None::<&str>),
            ("USERPROFILE", None::<&str>),
        ],
        || {
            assert!(super::super::default_config_path().is_none());
        },
    );
}

#[test]
fn default_config_path_uses_neo_home() {
    temp_env::with_var("NEO_HOME", Some("/custom/neo"), || {
        assert_eq!(
            super::super::default_config_path(),
            Some(PathBuf::from("/custom/neo/config.toml"))
        );
    });
}
