use super::{
    ArtifactMetadata, TaskOutputPage, TaskOutputRequest, TaskOutputView, WorkflowArtifactId,
    WorkflowId, WorkflowState, artifacts_cursor, compute_query_hash, decode_cursor,
    measure_tool_result_bytes, serialize_page, shrink_page_to_tool_result_cap, truncate_summary,
};

#[test]
fn truncating_multibyte_summary_keeps_a_character_boundary() {
    let mut summary = "审查".repeat(100);

    truncate_summary(&mut summary);

    assert!(summary.ends_with('…'));
    assert!(summary.len() <= 131);
}

#[test]
fn shrinking_second_artifact_page_advances_from_its_start_index() {
    let run_id = WorkflowId::from_existing("workflow_cursor".to_owned());
    let artifact = |index: usize| ArtifactMetadata {
        artifact_id: WorkflowArtifactId::new(run_id.clone(), format!("{index:064x}"))
            .expect("artifact id"),
        sha256: format!("{index:064x}"),
        byte_len: 1,
        media_type: "text/plain".to_owned(),
        logical_name: format!("artifact-{index}"),
        version: 1,
    };
    let query_hash = compute_query_hash(TaskOutputView::Artifacts, None);
    let request = TaskOutputRequest {
        view: TaskOutputView::Artifacts,
        cursor: Some(artifacts_cursor(&run_id, 5, &query_hash).expect("cursor")),
        max_output_bytes: u64::MAX,
        artifact_id: None,
    };
    let mut page = TaskOutputPage {
        view: TaskOutputView::Artifacts,
        run_id: run_id.as_str().to_owned(),
        kind: "workflow".to_owned(),
        status: "running".to_owned(),
        first_seq: None,
        last_seq: None,
        has_more: false,
        next_cursor: None,
        returned_bytes: 0,
        summary: None,
        journal: Vec::new(),
        result: None,
        artifacts: vec![artifact(5), artifact(6), artifact(7)],
        artifact_content: None,
        pending_user: None,
        state: WorkflowState::Running,
        failure_count: 0,
        invocation_count: 0,
    };
    let mut two_items = page.clone();
    two_items.artifacts.pop();
    let (content, details) = serialize_page(&two_items).expect("serialize");
    let max_output_bytes = measure_tool_result_bytes(&content, &details) as u64;
    let request = TaskOutputRequest {
        max_output_bytes,
        ..request
    };

    shrink_page_to_tool_result_cap(&mut page, &request).expect("shrink");

    assert!(page.artifacts.len() < 3);
    let cursor =
        decode_cursor(page.next_cursor.as_deref().expect("next cursor")).expect("decode cursor");
    assert_eq!(cursor.artifact_index, Some(5 + page.artifacts.len()));
    assert_eq!(
        page.returned_bytes,
        page.artifacts
            .iter()
            .map(|item| serde_json::to_vec(item).expect("serialize item").len() as u64)
            .sum::<u64>()
    );
    assert!(page.has_more);
}
