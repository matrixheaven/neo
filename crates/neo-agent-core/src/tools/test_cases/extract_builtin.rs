use super::*;

#[tokio::test]
async fn extract_builtin_skills_refreshes_stale_builtin_files() {
    let temp = tempfile::tempdir().expect("tempdir");
    let builtin_skill_dir = temp.path().join("skills").join(".builtin").join("self-evo");
    fs::create_dir_all(&builtin_skill_dir)
        .await
        .expect("mkdir builtin skill dir");
    let skill_path = builtin_skill_dir.join("SKILL.md");
    fs::write(
        &skill_path,
        "---\nname: self-evo\ndescription: stale\ndisableModelInvocation: true\n---\n\nSTALE_MARKER\n",
    )
    .await
    .expect("write stale builtin");

    crate::skills::builtin::extract_builtin_skills(&temp.path().join("skills"))
        .expect("extract built-ins");

    let content = fs::read_to_string(skill_path)
        .await
        .expect("read refreshed builtin");
    assert!(content.contains("No-argument invocation is not a scope"));
    assert!(!content.contains("STALE_MARKER"), "{content}");
}
