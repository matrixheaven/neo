use super::*;
use crate::skills::SkillStore;
use crate::skills::SkillStoreHandle;
use crate::skills::discovery;

fn list_skills_tool(neo_home: Option<PathBuf>) -> ListSkillsTool {
    let user_dirs = neo_home
        .as_deref()
        .map(discovery::user_skill_dirs)
        .unwrap_or_default();
    let store = SkillStore::load(
        &user_dirs,
        &[],
        crate::skills::builtin::builtin_skills().expect("builtin skills"),
    );
    ListSkillsTool::new(SkillStoreHandle::new(store))
}

#[test]
fn tool_descriptions_are_non_empty() {
    assert!(!list_skills_tool(None).description().is_empty());
    assert!(!CreateSkillTool::new(".").description().is_empty());
    assert!(!MoveSkillTool::new(".").description().is_empty());
}
