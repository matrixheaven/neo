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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_phase: Option<usize>,
    #[serde(default)]
    pub failure_strikes: u8,
    #[serde(default)]
    pub audit_rounds: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_ref: Option<String>,
}

impl Goal {
    #[must_use]
    pub fn new(objective: impl Into<String>) -> Self {
        let now = now_millis();
        let objective = objective.into();
        Self {
            id: Uuid::new_v4().to_string(),
            objective: objective.clone(),
            completion_criterion: None,
            status: GoalStatus::Active,
            created_at: now,
            updated_at: now,
            session_id: None,
            blocked_reason: None,
            artifact_dir: None,
            raw_prompt: Some(objective.clone()),
            approved_text: Some(objective.clone()),
            phases: vec![objective],
            current_phase: Some(0),
            failure_strikes: 0,
            audit_rounds: 0,
            baseline_ref: None,
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

fn goals_dir(session_dir: &Path) -> PathBuf {
    session_dir.join("goals")
}

fn goal_path(session_dir: &Path, id: &str) -> PathBuf {
    goals_dir(session_dir).join(format!("{id}.json"))
}

fn goal_artifact_dir(session_dir: &Path, id: &str) -> PathBuf {
    goals_dir(session_dir).join("runs").join(id)
}

async fn ensure_goal_artifacts(session_dir: &Path, goal: &mut Goal) -> Result<()> {
    let dir = goal
        .artifact_dir
        .clone()
        .unwrap_or_else(|| goal_artifact_dir(session_dir, &goal.id));
    let phases_dir = dir.join("phases");
    fs::create_dir_all(&phases_dir).await?;
    goal.artifact_dir = Some(dir.clone());
    if goal.raw_prompt.is_none() {
        goal.raw_prompt = Some(goal.objective.clone());
    }
    if goal.approved_text.is_none() {
        goal.approved_text = Some(goal.objective.clone());
    }
    if goal.phases.is_empty() {
        goal.phases.push(goal.objective.clone());
    }
    if goal.current_phase.is_none() {
        goal.current_phase = Some(0);
    }

    let criterion = goal
        .completion_criterion
        .as_deref()
        .unwrap_or("No separate completion criterion was provided.");
    let goal_md = format!(
        "# Goal\n\n{}\n\n## Completion Criterion\n\n{}\n",
        goal.objective, criterion
    );
    let roadmap =
        goal.phases
            .iter()
            .enumerate()
            .fold(String::new(), |mut output, (index, phase)| {
                use std::fmt::Write as _;
                let _ = writeln!(output, "{}. {}", index + 1, phase);
                output
            });
    let state = format!(
        "# State\n\nstatus: {:?}\ncurrent_phase: {}\nfailure_strikes: {}\naudit_rounds: {}\n",
        goal.status,
        goal.current_phase.map_or(0, |phase| phase + 1),
        goal.failure_strikes,
        goal.audit_rounds
    );
    let thinking = "# Thinking\n\nRisks, assumptions, and recon notes go here.\n";
    let protocol = "# Protocol\n\nProceed phase by phase. Retry once, write a focused fix spec on the second failure, and block with handoff details on the third failure. Run a final audit before completion.\n";

    fs::write(dir.join("GOAL.md"), goal_md).await?;
    fs::write(dir.join("ROADMAP.md"), roadmap).await?;
    fs::write(dir.join("STATE.md"), state).await?;
    fs::write(dir.join("THINKING.md"), thinking).await?;
    fs::write(dir.join("PROTOCOL.md"), protocol).await?;
    for (index, phase) in goal.phases.iter().enumerate() {
        let path = phases_dir.join(format!("phase-{}.md", index + 1));
        fs::write(path, format!("# Phase {}\n\n{}\n", index + 1, phase)).await?;
    }
    Ok(())
}

pub async fn load_goal_store(session_dir: &Path) -> Result<GoalStore> {
    let dir = goals_dir(session_dir);
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

pub async fn save_goal(session_dir: &Path, goal: &Goal) -> Result<()> {
    let path = goal_path(session_dir, &goal.id);
    let goals_dir = goals_dir(session_dir);
    fs::create_dir_all(&goals_dir).await?;
    let content = serde_json::to_string_pretty(goal)?;
    fs::write(&path, content).await?;
    Ok(())
}

pub async fn delete_goal(session_dir: &Path, id: &str) -> Result<()> {
    let path = goal_path(session_dir, id);
    if path.exists() {
        fs::remove_file(&path).await?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct GoalManager {
    session_dir: PathBuf,
    store: Arc<Mutex<GoalStore>>,
}

impl GoalManager {
    pub async fn load(session_dir: PathBuf) -> Result<Self> {
        let store = load_goal_store(&session_dir).await?;
        Ok(Self {
            session_dir,
            store: Arc::new(Mutex::new(store)),
        })
    }

    #[must_use]
    pub fn active(&self) -> Option<Goal> {
        self.store.lock().ok()?.active().cloned()
    }

    pub async fn start(&self, mut goal: Goal) -> Result<Option<Goal>> {
        ensure_goal_artifacts(&self.session_dir, &mut goal).await?;
        let previous = {
            let mut store = self.store.lock().map_err(|_| GoalError::Lock)?;
            store.start(goal.clone())
        };
        save_goal(&self.session_dir, &goal).await?;
        Ok(previous)
    }

    pub async fn pause(&self) -> Result<Option<Goal>> {
        let goal = {
            let mut store = self.store.lock().map_err(|_| GoalError::Lock)?;
            store.pause()
        };
        if let Some(ref goal) = goal {
            save_goal(&self.session_dir, goal).await?;
        }
        Ok(goal)
    }

    pub async fn resume(&self) -> Result<Option<Goal>> {
        let goal = {
            let mut store = self.store.lock().map_err(|_| GoalError::Lock)?;
            store.resume()
        };
        if let Some(ref goal) = goal {
            save_goal(&self.session_dir, goal).await?;
        }
        Ok(goal)
    }

    pub async fn cancel(&self) -> Result<Option<Goal>> {
        let goal = {
            let mut store = self.store.lock().map_err(|_| GoalError::Lock)?;
            store.cancel()
        };
        if let Some(ref goal) = goal {
            delete_goal(&self.session_dir, &goal.id).await?;
        }
        Ok(goal)
    }

    pub async fn replace(&self, mut goal: Goal) -> Result<Option<Goal>> {
        ensure_goal_artifacts(&self.session_dir, &mut goal).await?;
        let previous = {
            let mut store = self.store.lock().map_err(|_| GoalError::Lock)?;
            store.replace(goal.clone())
        };
        if let Some(ref previous) = previous {
            delete_goal(&self.session_dir, &previous.id).await?;
        }
        save_goal(&self.session_dir, &goal).await?;
        Ok(previous)
    }

    pub async fn queue_next(&self, mut goal: Goal) -> Result<()> {
        ensure_goal_artifacts(&self.session_dir, &mut goal).await?;
        {
            let mut store = self.store.lock().map_err(|_| GoalError::Lock)?;
            store.queue_next(goal.clone());
        }
        save_goal(&self.session_dir, &goal).await?;
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
                let _ = store.cancel();
                Some(goal)
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
                    delete_goal(&self.session_dir, &goal.id).await?;
                }
            }
            _ => {
                if let Some(ref goal) = goal {
                    save_goal(&self.session_dir, goal).await?;
                }
            }
        }
        Ok(goal)
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
