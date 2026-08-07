//! repository behavior (moved from `themes.rs`).

use super::super::*;
use super::*;
use tempfile::TempDir;

#[test]
fn repository_rejects_ids_traversing_symlinked_directories() {
    let temp = TempDir::new().expect("tempdir");
    let repo = repository_in(&temp);
    std::fs::create_dir_all(repo.root()).expect("create repo root");
    let outside_dir = temp.path().join("outside-dir");
    std::fs::create_dir_all(&outside_dir).expect("create outside dir");
    let outside_theme = outside_dir.join("x.json");
    std::fs::write(&outside_theme, r#"{"name": "Escaped", "colors": {}}"#)
        .expect("write outside theme");
    let link = repo.root().join("nested");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_dir, &link).expect("symlink");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&outside_dir, &link).expect("symlink");

    let id = ThemeId::new("nested/x.json").expect("valid id");
    let error = repo
        .load(&id)
        .expect_err("symlinked directory must be rejected");
    assert!(error.to_string().contains("symlink"), "load error: {error}");

    let theme = TuiTheme::default();
    let overwrite_error = repo
        .overwrite(&id, "Escaped", &theme)
        .expect_err("overwrite through symlink must be rejected");
    assert!(
        overwrite_error.to_string().contains("symlink"),
        "overwrite error: {overwrite_error}"
    );

    let save_error = repo
        .save_as_new(&ThemeId::new("nested/new.json").unwrap(), "Escaped", &theme)
        .expect_err("save-as-new through symlink must be rejected");
    assert!(
        save_error.to_string().contains("symlink"),
        "save_as_new error: {save_error}"
    );

    let delete_error = repo
        .delete(&id)
        .expect_err("delete through symlink must be rejected");
    assert!(
        delete_error.to_string().contains("symlink"),
        "delete error: {delete_error}"
    );
}

#[test]
fn repository_overwrite_and_save_as_new_are_atomic_and_validate() {
    let temp = TempDir::new().expect("tempdir");
    let repo = repository_in(&temp);
    let id = write_theme(&repo, "base.json", "Base", "blue");

    let theme = TuiTheme {
        brand: Color::Rgb(1, 2, 3),
        ..Default::default()
    };
    let entry = repo
        .overwrite(&id, "Renamed", &theme)
        .expect("overwrite existing");
    assert!(entry.is_valid());
    assert_eq!(entry.name, "Renamed");

    let catalog = repo.catalog().expect("catalog");
    let reloaded = catalog.by_id(&id).expect("reload");
    assert_eq!(reloaded.theme.brand, Color::Rgb(1, 2, 3));

    let saved = repo
        .save_as_new(&ThemeId::new("new.json").unwrap(), "New", &theme)
        .expect("save as new");
    assert!(saved.is_valid());
    assert!(
        repo.save_as_new(&ThemeId::new("new.json").unwrap(), "Again", &theme)
            .is_err(),
        "save-as-new must reject existing ids"
    );

    let missing_error = repo
        .overwrite(&ThemeId::new("missing.json").unwrap(), "Ghost", &theme)
        .expect_err("overwrite must not create a missing theme");
    assert!(
        missing_error.to_string().contains("does not exist"),
        "overwrite error: {missing_error}"
    );
    let catalog = repo.catalog().expect("catalog");
    assert!(
        catalog
            .by_id(&ThemeId::new("missing.json").unwrap())
            .is_none(),
        "failed overwrite must not create the theme file"
    );
}

#[test]
fn repository_import_reads_outside_source_without_storing_its_path() {
    let temp = TempDir::new().expect("tempdir");
    let repo = repository_in(&temp);
    let source = temp.path().join("outside-source.json");
    std::fs::write(
        &source,
        r##"{"name": "Imported", "colors": {"brand": "#abcdef"}}"##,
    )
    .expect("write source");

    let id = ThemeId::new("imported.json").unwrap();
    let entry = repo.import(&id, &source).expect("import");
    assert!(entry.is_valid());
    assert_eq!(entry.name, "Imported");
    assert_eq!(entry.theme.brand, Color::Rgb(0xab, 0xcd, 0xef));

    let path = id.path_under(repo.root());
    let content = std::fs::read_to_string(&path).expect("read imported");
    assert!(
        !content.contains("outside-source"),
        "source path must not be stored"
    );
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse");
    assert_eq!(parsed["name"], "Imported");
}

#[test]
fn repository_delete_removes_the_theme_file() {
    let temp = TempDir::new().expect("tempdir");
    let repo = repository_in(&temp);
    let id = write_theme(&repo, "doomed.json", "Doomed", "blue");
    repo.delete(&id).expect("delete");
    let catalog = repo.catalog().expect("catalog");
    assert!(catalog.by_id(&id).is_none());
    assert!(repo.delete(&id).is_err(), "missing delete must fail");
}
