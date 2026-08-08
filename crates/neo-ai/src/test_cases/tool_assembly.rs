//! Tool assembly behavior (moved from `tool_assembly.rs`).

use super::*;

fn chunk(
    index: Option<u64>,
    id: Option<&str>,
    name: Option<&str>,
    args: Option<&str>,
) -> ToolCallChunk {
    ToolCallChunk {
        index,
        id: id.map(str::to_owned),
        name: name.map(str::to_owned),
        arguments_delta: args.map(str::to_owned),
    }
}

#[test]
fn stable_index_survives_id_mutation() {
    let mut assembler = StreamingToolCallAssembler::new();
    let first = assembler
        .ingest(chunk(
            Some(0),
            Some("functions.read:0"),
            Some("read"),
            Some("{\"path\":"),
        ))
        .unwrap();
    let second = assembler
        .ingest(chunk(
            Some(0),
            Some("chatcmpl-tool-a"),
            None,
            Some("\"Cargo.toml\"}"),
        ))
        .unwrap();
    let end = assembler.finish_all().events;

    assert_eq!(
        [first, second, end].concat(),
        vec![
            ToolCallAssemblyEvent::Start {
                id: "functions.read:0".to_owned(),
                name: "read".to_owned(),
            },
            ToolCallAssemblyEvent::ArgsDelta {
                id: "functions.read:0".to_owned(),
                json_fragment: "{\"path\":".to_owned(),
            },
            ToolCallAssemblyEvent::ArgsDelta {
                id: "functions.read:0".to_owned(),
                json_fragment: "\"Cargo.toml\"}".to_owned(),
            },
            ToolCallAssemblyEvent::End {
                id: "functions.read:0".to_owned(),
                raw_arguments: "{\"path\":\"Cargo.toml\"}".to_owned(),
            },
        ]
    );
}

#[test]
fn arguments_before_name_are_buffered_until_start() {
    let mut assembler = StreamingToolCallAssembler::new();
    assert_eq!(
        assembler
            .ingest(chunk(
                Some(0),
                Some("call-1"),
                None,
                Some("{\"path\":\"Cargo")
            ))
            .unwrap(),
        Vec::<ToolCallAssemblyEvent>::new()
    );
    let events = assembler
        .ingest(chunk(Some(0), None, Some("read"), Some(".toml\"}")))
        .unwrap();

    assert_eq!(
        events,
        vec![
            ToolCallAssemblyEvent::Start {
                id: "call-1".to_owned(),
                name: "read".to_owned(),
            },
            ToolCallAssemblyEvent::ArgsDelta {
                id: "call-1".to_owned(),
                json_fragment: "{\"path\":\"Cargo".to_owned(),
            },
            ToolCallAssemblyEvent::ArgsDelta {
                id: "call-1".to_owned(),
                json_fragment: ".toml\"}".to_owned(),
            },
        ]
    );
}

#[test]
fn indexed_arguments_before_id_use_later_provider_id() {
    let mut assembler = StreamingToolCallAssembler::new();
    assert_eq!(
        assembler
            .ingest(chunk(Some(0), None, None, Some("{\"path\":\"Cargo")))
            .unwrap(),
        Vec::<ToolCallAssemblyEvent>::new()
    );

    let events = assembler
        .ingest(chunk(
            Some(0),
            Some("call-1"),
            Some("read"),
            Some(".toml\"}"),
        ))
        .unwrap();
    let end = assembler.finish_all().events;

    assert_eq!(
        events.into_iter().chain(end).collect::<Vec<_>>(),
        vec![
            ToolCallAssemblyEvent::Start {
                id: "call-1".to_owned(),
                name: "read".to_owned(),
            },
            ToolCallAssemblyEvent::ArgsDelta {
                id: "call-1".to_owned(),
                json_fragment: "{\"path\":\"Cargo".to_owned(),
            },
            ToolCallAssemblyEvent::ArgsDelta {
                id: "call-1".to_owned(),
                json_fragment: ".toml\"}".to_owned(),
            },
            ToolCallAssemblyEvent::End {
                id: "call-1".to_owned(),
                raw_arguments: "{\"path\":\"Cargo.toml\"}".to_owned(),
            },
        ]
    );
}

#[test]
fn unindexed_arguments_before_id_use_later_provider_id() {
    let mut assembler = StreamingToolCallAssembler::new();
    assert_eq!(
        assembler
            .ingest(chunk(None, None, None, Some("{\"path\":\"Cargo")))
            .unwrap(),
        Vec::<ToolCallAssemblyEvent>::new()
    );

    let events = assembler
        .ingest(chunk(None, Some("call-1"), Some("read"), Some(".toml\"}")))
        .unwrap();
    let end = assembler.finish_all().events;

    assert!(events.contains(&ToolCallAssemblyEvent::Start {
        id: "call-1".to_owned(),
        name: "read".to_owned(),
    }));
    assert!(end.contains(&ToolCallAssemblyEvent::End {
        id: "call-1".to_owned(),
        raw_arguments: "{\"path\":\"Cargo.toml\"}".to_owned(),
    }));
}

#[test]
fn name_before_id_starts_when_provider_id_arrives() {
    let mut assembler = StreamingToolCallAssembler::new();
    assert_eq!(
        assembler
            .ingest(chunk(Some(0), None, Some("read"), None))
            .unwrap(),
        Vec::<ToolCallAssemblyEvent>::new()
    );

    let events = assembler
        .ingest(chunk(Some(0), Some("call-1"), None, Some("{}")))
        .unwrap();

    assert_eq!(
        events,
        vec![
            ToolCallAssemblyEvent::Start {
                id: "call-1".to_owned(),
                name: "read".to_owned(),
            },
            ToolCallAssemblyEvent::ArgsDelta {
                id: "call-1".to_owned(),
                json_fragment: "{}".to_owned(),
            },
        ]
    );
}

#[test]
fn interleaved_indexed_calls_finish_independently() {
    let mut assembler = StreamingToolCallAssembler::new();
    let mut events = Vec::new();
    events.extend(
        assembler
            .ingest(chunk(
                Some(0),
                Some("call-a"),
                Some("read"),
                Some("{\"path\":"),
            ))
            .unwrap(),
    );
    events.extend(
        assembler
            .ingest(chunk(
                Some(1),
                Some("call-b"),
                Some("grep"),
                Some("{\"pattern\":"),
            ))
            .unwrap(),
    );
    events.extend(
        assembler
            .ingest(chunk(Some(0), None, None, Some("\"Cargo.toml\"}")))
            .unwrap(),
    );
    events.extend(
        assembler
            .ingest(chunk(Some(1), None, None, Some("\"neo\"}")))
            .unwrap(),
    );
    events.extend(assembler.finish_all().events);

    assert!(events.contains(&ToolCallAssemblyEvent::End {
        id: "call-a".to_owned(),
        raw_arguments: "{\"path\":\"Cargo.toml\"}".to_owned(),
    }));
    assert!(events.contains(&ToolCallAssemblyEvent::End {
        id: "call-b".to_owned(),
        raw_arguments: "{\"pattern\":\"neo\"}".to_owned(),
    }));
}

#[test]
fn repeated_prefix_delta_is_preserved() {
    let mut assembler = StreamingToolCallAssembler::new();
    assembler
        .ingest(chunk(
            Some(0),
            Some("call-1"),
            Some("read"),
            Some("{\"x\":\""),
        ))
        .unwrap();
    let repeated = assembler
        .ingest(chunk(Some(0), None, None, Some("{")))
        .unwrap();
    assembler
        .ingest(chunk(Some(0), None, None, Some("\"}")))
        .unwrap();
    let end = assembler.finish_all().events;

    assert_eq!(
        repeated,
        vec![ToolCallAssemblyEvent::ArgsDelta {
            id: "call-1".to_owned(),
            json_fragment: "{".to_owned(),
        }]
    );
    assert!(end.contains(&ToolCallAssemblyEvent::End {
        id: "call-1".to_owned(),
        raw_arguments: "{\"x\":\"{\"}".to_owned(),
    }));
}

#[test]
fn final_arguments_override_preview_without_duplicate_delta() {
    let mut assembler = StreamingToolCallAssembler::new();
    let preview = assembler
        .ingest(chunk(
            Some(0),
            Some("call-1"),
            Some("read"),
            Some("{\"path\":\"Car"),
        ))
        .unwrap();
    let done = assembler
        .finish_with_final_arguments(
            Some(0),
            "call-1".to_owned(),
            "read".to_owned(),
            "{\"path\":\"Cargo.toml\"}".to_owned(),
        )
        .unwrap();

    assert_eq!(
        preview.into_iter().chain(done).collect::<Vec<_>>(),
        vec![
            ToolCallAssemblyEvent::Start {
                id: "call-1".to_owned(),
                name: "read".to_owned(),
            },
            ToolCallAssemblyEvent::ArgsDelta {
                id: "call-1".to_owned(),
                json_fragment: "{\"path\":\"Car".to_owned(),
            },
            ToolCallAssemblyEvent::End {
                id: "call-1".to_owned(),
                raw_arguments: "{\"path\":\"Cargo.toml\"}".to_owned(),
            },
        ]
    );
}

#[test]
fn multiple_unindexed_tool_calls_with_different_ids_fail_closed() {
    let mut assembler = StreamingToolCallAssembler::new();
    assembler
        .ingest(chunk(None, Some("call-1"), Some("read"), None))
        .unwrap();
    let result = assembler.ingest(chunk(None, Some("call-2"), Some("grep"), None));
    assert!(matches!(
        result,
        Err(ToolCallAssemblyError::AmbiguousUnindexedToolCalls)
    ));
}

#[test]
fn finish_all_emits_end_for_started_named_tools_before_missing_name() {
    let mut assembler = StreamingToolCallAssembler::new();
    assembler
        .ingest(chunk(
            Some(0),
            Some("call-named"),
            Some("read"),
            Some("{\"path\":\"x\"}"),
        ))
        .unwrap();
    // Unnamed unfinished slot must not block finishing the named tool.
    assembler
        .ingest(chunk(Some(1), Some("call-missing"), None, Some("{}")))
        .unwrap();

    let outcome = assembler.finish_all();

    assert_eq!(
        outcome.events,
        vec![ToolCallAssemblyEvent::End {
            id: "call-named".to_owned(),
            raw_arguments: "{\"path\":\"x\"}".to_owned(),
        }]
    );
    assert_eq!(
        outcome.error,
        Some(ToolCallAssemblyError::MissingName {
            id: "call-missing".to_owned(),
        })
    );
}
