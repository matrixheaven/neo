use std::collections::HashSet;

use super::AgentDisplayName;

pub const DEFAULT_AGENT_NAMES: &[&str] = &[
    "Zeno",
    "Gibbs",
    "Hokke",
    "Laber",
    "Ada",
    "Turing",
    "Knuth",
    "Shannon",
    "Euler",
    "Noether",
    "Gauss",
    "Hypatia",
    "Athena",
    "Hermes",
    "Apollo",
    "Atlas",
    "Merlin",
    "Arthur",
    "Wukong",
    "Nezha",
    "Mulan",
    "Orion",
    "Kepler",
    "Curie",
    "Feynman",
    "Lovelace",
    "Hopper",
    "Ramanujan",
    "Socrates",
    "Plato",
    "Artemis",
    "Diana",
    "Minerva",
    "Loki",
    "Freya",
];

#[derive(Debug, Clone)]
pub struct DisplayNamePool {
    next_index: usize,
    assigned: HashSet<String>,
}

impl Default for DisplayNamePool {
    fn default() -> Self {
        Self {
            next_index: 0,
            assigned: HashSet::new(),
        }
    }
}

impl DisplayNamePool {
    #[must_use]
    pub fn next_name(&mut self) -> AgentDisplayName {
        loop {
            let index = self.next_index;
            self.next_index += 1;
            let base = DEFAULT_AGENT_NAMES[index % DEFAULT_AGENT_NAMES.len()];
            let candidate = if index < DEFAULT_AGENT_NAMES.len() {
                base.to_owned()
            } else {
                format!("{base}{}", index / DEFAULT_AGENT_NAMES.len() + 1)
            };
            if self.assigned.insert(candidate.clone()) {
                return AgentDisplayName::new(candidate);
            }
        }
    }

    pub fn reserve(&mut self, name: &AgentDisplayName) {
        self.assigned.insert(name.as_str().to_owned());
    }
}
