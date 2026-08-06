use super::*;

#[test]
fn context_groups_merge_adjacent_matches() {
    let groups = build_context_groups(&[1, 3], 1, 1, 10);
    assert_eq!(groups, vec![(0, 4)]);
}

#[test]
fn context_groups_separate_distant_matches() {
    let groups = build_context_groups(&[1, 5], 0, 0, 10);
    assert_eq!(groups, vec![(1, 1), (5, 5)]);
}
