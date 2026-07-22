use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallChunk {
    pub index: Option<u64>,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments_delta: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallAssemblyEvent {
    Start { id: String, name: String },
    ArgsDelta { id: String, json_fragment: String },
    End { id: String, raw_arguments: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolCallAssemblyError {
    #[error("multiple unindexed tool calls cannot be assembled deterministically")]
    AmbiguousUnindexedToolCalls,
    #[error("tool call {id} finished without a function name")]
    MissingName { id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ToolCallKey {
    Indexed(u64),
    Unindexed,
}

#[derive(Debug, Clone, Default)]
struct ToolCallSlot {
    stable_id: Option<String>,
    name: Option<String>,
    raw_arguments: String,
    started: bool,
    finished: bool,
}

#[derive(Debug, Default)]
pub struct StreamingToolCallAssembler {
    slots: BTreeMap<ToolCallKey, ToolCallSlot>,
    saw_unindexed: bool,
}

impl StreamingToolCallAssembler {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ingest(
        &mut self,
        chunk: ToolCallChunk,
    ) -> Result<Vec<ToolCallAssemblyEvent>, ToolCallAssemblyError> {
        let key = self.key_for(&chunk)?;
        let slot = self.slots.entry(key).or_default();
        Ok(update_slot(slot, chunk))
    }

    pub fn finish_all(&mut self) -> Result<Vec<ToolCallAssemblyEvent>, ToolCallAssemblyError> {
        let mut out = Vec::new();
        for (key, slot) in &mut self.slots {
            if slot.finished {
                continue;
            }
            let id = slot
                .stable_id
                .clone()
                .unwrap_or_else(|| fallback_id_for_key(*key));
            let Some(name) = slot.name.clone() else {
                return Err(ToolCallAssemblyError::MissingName { id });
            };
            if !slot.started {
                slot.started = true;
                out.push(ToolCallAssemblyEvent::Start {
                    id: id.clone(),
                    name,
                });
                if !slot.raw_arguments.is_empty() {
                    out.push(ToolCallAssemblyEvent::ArgsDelta {
                        id: id.clone(),
                        json_fragment: slot.raw_arguments.clone(),
                    });
                }
            }
            slot.finished = true;
            out.push(ToolCallAssemblyEvent::End {
                id,
                raw_arguments: slot.raw_arguments.clone(),
            });
        }
        Ok(out)
    }

    pub fn finish_with_final_arguments(
        &mut self,
        index: Option<u64>,
        id: String,
        name: String,
        raw_arguments: String,
    ) -> Result<Vec<ToolCallAssemblyEvent>, ToolCallAssemblyError> {
        let key = index.map_or(ToolCallKey::Unindexed, ToolCallKey::Indexed);
        let slot = self.slots.entry(key).or_default();
        let mut out = Vec::new();
        if slot.stable_id.is_none() {
            slot.stable_id = Some(id.clone());
        }
        if slot.name.is_none() {
            slot.name = Some(name.clone());
        }
        if !slot.started {
            slot.started = true;
            out.push(ToolCallAssemblyEvent::Start {
                id: slot.stable_id.clone().unwrap_or(id.clone()),
                name,
            });
        }
        slot.raw_arguments.clone_from(&raw_arguments);
        if !slot.finished {
            slot.finished = true;
            out.push(ToolCallAssemblyEvent::End {
                id: slot.stable_id.clone().unwrap_or(id),
                raw_arguments,
            });
        }
        Ok(out)
    }

    fn key_for(&mut self, chunk: &ToolCallChunk) -> Result<ToolCallKey, ToolCallAssemblyError> {
        if let Some(index) = chunk.index {
            return Ok(ToolCallKey::Indexed(index));
        }
        if self.saw_unindexed && chunk.id.is_some() {
            let existing = self.slots.get(&ToolCallKey::Unindexed);
            let same_or_unassigned_id = existing
                .and_then(|slot| slot.stable_id.as_deref())
                .is_none_or(|id| chunk.id.as_deref() == Some(id));
            if !same_or_unassigned_id {
                return Err(ToolCallAssemblyError::AmbiguousUnindexedToolCalls);
            }
        }
        self.saw_unindexed = true;
        Ok(ToolCallKey::Unindexed)
    }
}

fn update_slot(slot: &mut ToolCallSlot, chunk: ToolCallChunk) -> Vec<ToolCallAssemblyEvent> {
    let mut out = Vec::new();
    if slot.stable_id.is_none() {
        slot.stable_id = chunk.id;
    }
    if slot.name.is_none() {
        slot.name = chunk.name;
    }
    if !slot.started
        && let (Some(id), Some(name)) = (slot.stable_id.clone(), slot.name.clone())
    {
        slot.started = true;
        out.push(ToolCallAssemblyEvent::Start {
            id: id.clone(),
            name,
        });
        if !slot.raw_arguments.is_empty() {
            out.push(ToolCallAssemblyEvent::ArgsDelta {
                id,
                json_fragment: slot.raw_arguments.clone(),
            });
        }
    }
    if let Some(delta) = chunk.arguments_delta.filter(|delta| !delta.is_empty()) {
        slot.raw_arguments.push_str(&delta);
        if slot.started {
            out.push(ToolCallAssemblyEvent::ArgsDelta {
                id: slot.stable_id.clone().expect("started tool call has an id"),
                json_fragment: delta,
            });
        }
    }
    out
}

fn fallback_id_for_key(key: ToolCallKey) -> String {
    match key {
        ToolCallKey::Indexed(index) => format!("tool-{index}"),
        ToolCallKey::Unindexed => "tool-0".to_owned(),
    }
}

#[cfg(test)]
mod tests {
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
        let end = assembler.finish_all().unwrap();

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
        let end = assembler.finish_all().unwrap();

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
        let end = assembler.finish_all().unwrap();

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
        events.extend(assembler.finish_all().unwrap());

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
        let end = assembler.finish_all().unwrap();

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
}
