use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::fs;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub objective: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_criterion: Option<String>,
    pub status: GoalStatus,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub turn_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
}

impl Goal {
    #[must_use]
    pub fn new(objective: impl Into<String>) -> Self {
        let now = now_millis();
        Self {
            id: Uuid::new_v4().to_string(),
            objective: objective.into(),
            completion_criterion: None,
            status: GoalStatus::Active,
            created_at: now,
            updated_at: now,
            turn_count: 0,
            session_id: None,
            blocked_reason: None,
        }
    }

    #[must_use]
    pub fn with_completion_criterion(mut self, criterion: impl Into<String>) -> Self {
        self.completion_criterion = Some(criterion.into());
        self
    }

    pub fn touch(&mut self) {
        self.updated_at = now_millis();
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoalStore {
    active: Option<Goal>,
    queue: VecDeque<Goal>,
}

impl GoalStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(&mut self, goal: Goal) -> Option<Goal> {
        let previous = self.active.take();
        self.active = Some(goal);
        previous
    }

    pub fn pause(&mut self) -> Option<Goal> {
        let mut goal = self.active.take()?;
        goal.status = GoalStatus::Paused;
        goal.touch();
        self.active = Some(goal);
        self.active.clone()
    }

    pub fn resume(&mut self) -> Option<Goal> {
        let goal = self.active.as_mut()?;
        if matches!(goal.status, GoalStatus::Paused | GoalStatus::Blocked) {
            goal.status = GoalStatus::Active;
            goal.blocked_reason = None;
            goal.touch();
        }
        self.active.clone()
    }

    pub fn cancel(&mut self) -> Option<Goal> {
        self.active.take()
    }

    pub fn replace(&mut self, goal: Goal) -> Option<Goal> {
        self.active.replace(goal)
    }

    pub fn queue_next(&mut self, goal: Goal) {
        self.queue.push_back(goal);
    }

    pub fn dequeue_next(&mut self) -> Option<Goal> {
        self.queue.pop_front()
    }

    #[must_use]
    pub fn active(&self) -> Option<&Goal> {
        self.active.as_ref()
    }

    pub fn active_mut(&mut self) -> Option<&mut Goal> {
        self.active.as_mut()
    }

    #[must_use]
    pub fn queue(&self) -> &VecDeque<Goal> {
        &self.queue
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_none() && self.queue.is_empty()
    }
}

fn goals_dir(home: &Path) -> PathBuf {
    home.join("goals")
}

fn goal_path(home: &Path, id: &str) -> PathBuf {
    goals_dir(home).join(format!("{id}.json"))
}

pub async fn load_goal_store(home: &Path) -> Result<GoalStore> {
    let dir = goals_dir(home);
    fs::create_dir_all(&dir).await?;
    let mut store = GoalStore::new();
    let mut entries = fs::read_dir(&dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let content = fs::read_to_string(&path).await?;
        let goal: Goal = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse goal {}", path.display()))?;
        if matches!(
            goal.status,
            GoalStatus::Active | GoalStatus::Paused | GoalStatus::Blocked
        ) {
            if store.active.is_none() {
                store.active = Some(goal);
            } else {
                store.queue.push_back(goal);
            }
        }
    }
    Ok(store)
}

pub async fn save_goal(home: &Path, goal: &Goal) -> Result<()> {
    let path = goal_path(home, &goal.id);
    fs::create_dir_all(path.parent().unwrap()).await?;
    let content = serde_json::to_string_pretty(goal)?;
    fs::write(&path, content).await?;
    Ok(())
}

pub async fn delete_goal(home: &Path, id: &str) -> Result<()> {
    let path = goal_path(home, id);
    if path.exists() {
        fs::remove_file(&path).await?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct GoalManager {
    home: PathBuf,
    store: Arc<Mutex<GoalStore>>,
}

impl GoalManager {
    pub async fn load(home: PathBuf) -> Result<Self> {
        let store = load_goal_store(&home).await?;
        Ok(Self {
            home,
            store: Arc::new(Mutex::new(store)),
        })
    }

    #[must_use]
    pub fn active(&self) -> Option<Goal> {
        self.store.lock().ok()?.active().cloned()
    }

    pub async fn start(&self, goal: Goal) -> Result<Option<Goal>> {
        let previous = {
            let mut store = self.store.lock().map_err(|_| GoalError::Lock)?;
            store.start(goal.clone())
        };
        save_goal(&self.home, &goal).await?;
        Ok(previous)
    }

    pub async fn pause(&self) -> Result<Option<Goal>> {
        let goal = {
            let mut store = self.store.lock().map_err(|_| GoalError::Lock)?;
            store.pause()
        };
        if let Some(ref goal) = goal {
            save_goal(&self.home, goal).await?;
        }
        Ok(goal)
    }

    pub async fn resume(&self) -> Result<Option<Goal>> {
        let goal = {
            let mut store = self.store.lock().map_err(|_| GoalError::Lock)?;
            store.resume()
        };
        if let Some(ref goal) = goal {
            save_goal(&self.home, goal).await?;
        }
        Ok(goal)
    }

    pub async fn cancel(&self) -> Result<Option<Goal>> {
        let goal = {
            let mut store = self.store.lock().map_err(|_| GoalError::Lock)?;
            store.cancel()
        };
        if let Some(ref goal) = goal {
            delete_goal(&self.home, &goal.id).await?;
        }
        Ok(goal)
    }

    pub async fn replace(&self, goal: Goal) -> Result<Option<Goal>> {
        let previous = {
            let mut store = self.store.lock().map_err(|_| GoalError::Lock)?;
            store.replace(goal.clone())
        };
        if let Some(ref previous) = previous {
            delete_goal(&self.home, &previous.id).await?;
        }
        save_goal(&self.home, &goal).await?;
        Ok(previous)
    }

    pub async fn queue_next(&self, goal: Goal) -> Result<()> {
        {
            let mut store = self.store.lock().map_err(|_| GoalError::Lock)?;
            store.queue_next(goal.clone());
        }
        save_goal(&self.home, &goal).await?;
        Ok(())
    }

    #[must_use]
    pub fn dequeue_next(&self) -> Option<Goal> {
        let mut store = self.store.lock().ok()?;
        store.dequeue_next()
    }

    #[must_use]
    pub fn queue(&self) -> Vec<Goal> {
        let store = self.store.lock().ok();
        store
            .map(|store| store.queue().iter().cloned().collect())
            .unwrap_or_default()
    }

    pub async fn update_status(
        &self,
        status: GoalStatus,
        reason: Option<String>,
    ) -> Result<Option<Goal>> {
        let goal = {
            let mut store = self.store.lock().map_err(|_| GoalError::Lock)?;
            let Some(mut goal) = store.active_mut().cloned() else {
                return Ok(None);
            };
            goal.status = status;
            goal.blocked_reason = reason;
            goal.touch();
            if matches!(status, GoalStatus::Complete) {
                store.cancel()
            } else if let Some(active) = store.active_mut() {
                *active = goal.clone();
                Some(goal)
            } else {
                Some(goal)
            }
        };
        match status {
            GoalStatus::Complete => {
                if let Some(ref goal) = goal {
                    delete_goal(&self.home, &goal.id).await?;
                }
            }
            _ => {
                if let Some(ref goal) = goal {
                    save_goal(&self.home, goal).await?;
                }
            }
        }
        Ok(goal)
    }

    pub async fn increment_turn(&self) -> Result<()> {
        let goal = {
            let mut store = self.store.lock().map_err(|_| GoalError::Lock)?;
            let Some(goal) = store.active_mut() else {
                return Ok(());
            };
            goal.turn_count += 1;
            goal.touch();
            goal.clone()
        };
        save_goal(&self.home, &goal).await?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
enum GoalError {
    #[error("goal store lock poisoned")]
    Lock,
}

fn now_millis() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}
