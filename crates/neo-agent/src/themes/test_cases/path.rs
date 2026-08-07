//! path behavior (moved from `themes.rs`).

use super::super::*;
use std::path::PathBuf;

#[test]
fn theme_path_tilde_expands_to_user_home() {
    assert_eq!(
        expand_user_path_with_home(
            PathBuf::from("~/themes/night.json"),
            Some(Path::new("/home/alice")),
        ),
        PathBuf::from("/home/alice/themes/night.json")
    );
}
