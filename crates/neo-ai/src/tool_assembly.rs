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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishAllOutcome {
    pub events: Vec<ToolCallAssemblyEvent>,
    pub error: Option<ToolCallAssemblyError>,
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

    pub fn finish_all(&mut self) -> FinishAllOutcome {
        let mut events = Vec::new();
        let mut error = None;
        for (key, slot) in &mut self.slots {
            if slot.finished {
                continue;
            }
            let id = slot
                .stable_id
                .clone()
                .unwrap_or_else(|| fallback_id_for_key(*key));
            let Some(name) = slot.name.clone() else {
                if error.is_none() {
                    error = Some(ToolCallAssemblyError::MissingName { id });
                }
                continue;
            };
            if !slot.started {
                slot.started = true;
                events.push(ToolCallAssemblyEvent::Start {
                    id: id.clone(),
                    name,
                });
                if !slot.raw_arguments.is_empty() {
                    events.push(ToolCallAssemblyEvent::ArgsDelta {
                        id: id.clone(),
                        json_fragment: slot.raw_arguments.clone(),
                    });
                }
            }
            slot.finished = true;
            events.push(ToolCallAssemblyEvent::End {
                id,
                raw_arguments: slot.raw_arguments.clone(),
            });
        }
        FinishAllOutcome { events, error }
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
#[path = "test_cases/tool_assembly.rs"]
mod tests;
