//! Unit tests for the tool-run group render cache in [`TranscriptPane`]:
//! unchanged visible groups are served from the cache, and live groups are
//! never cached.

use super::*;

const WIDTH: usize = 120;
const HEIGHT: usize = 40;

fn exit_plan_mode_pane() -> TranscriptPane {
    let mut pane = TranscriptPane::new(WIDTH, HEIGHT);
    {
        let store = pane.transcript_mut();
        store.push_tool_run("plan-1", "ExitPlanMode", Some("{}".to_owned()));
        store.mutate_tool("plan-1", |tool| {
            tool.set_result(
                Some("submitted".to_owned()),
                Some(serde_json::json!({
                    "plan_content": "# Plan\n\n1. First step\n2. Second step\n",
                    "plan_path": "/tmp/plan.md",
                })),
                false,
                None,
            )
        });
    }
    pane
}

#[test]
fn visible_unchanged_group_block_is_served_from_cache() {
    let mut pane = exit_plan_mode_pane();
    let first = pane.render_visible_slice(WIDTH, HEIGHT);
    assert_eq!(
        pane.tool_group_cache_hit_rows_for_test(),
        0,
        "the first compose renders fresh and only populates the cache"
    );

    let second = pane.render_visible_slice(WIDTH, HEIGHT);
    assert!(
        pane.tool_group_cache_hit_rows_for_test() > 0,
        "an unchanged visible group must be served from the group cache"
    );
    assert_eq!(first, second);
}

#[test]
fn live_group_block_is_never_served_from_cache() {
    let mut pane = TranscriptPane::new(WIDTH, HEIGHT);
    pane.transcript_mut()
        .push_tool_run("bash-1", "Bash", Some("{\"command\":\"ls\"}".to_owned()));

    let _ = pane.render_visible_slice(WIDTH, HEIGHT);
    let _ = pane.render_visible_slice(WIDTH, HEIGHT);
    assert_eq!(
        pane.tool_group_cache_hit_rows_for_test(),
        0,
        "live cards carry time-dependent chips and must re-render every frame"
    );
}
