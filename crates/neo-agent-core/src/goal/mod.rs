use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::session::{
    atomic_file::{
        AtomicWriteStatus, ensure_safe_directory_tree, reject_reparse_or_symlink_if_present,
        sync_directory, validate_safe_directory, validate_safe_directory_if_present,
        write_file_atomic, write_file_atomic_status,
    },
    main_agent_goals_dir,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    Queued,
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

    fn install(&mut self, goal: Goal) {
        self.active = Some(goal);
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
        let mut goal = self.queue.pop_front()?;
        goal.status = GoalStatus::Active;
        goal.touch();
        Some(goal)
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
}

fn goals_dir(session_dir: &Path) -> PathBuf {
    main_agent_goals_dir(session_dir)
}

fn active_store_path(session_dir: &Path) -> PathBuf {
    goals_dir(session_dir).join("active.json")
}

fn goal_artifact_dir(session_dir: &Path, id: &str) -> PathBuf {
    goals_dir(session_dir).join("runs").join(id)
}

fn ensure_goals_dir(session_dir: &Path) -> Result<PathBuf> {
    ensure_safe_directory_tree(session_dir)?;
    let mut current = session_dir.to_path_buf();
    for component in ["agents", "main", "goals"] {
        current.push(component);
        ensure_safe_directory_tree(&current)?;
    }
    Ok(current)
}

fn ensure_goal_artifacts(session_dir: &Path, goal: &mut Goal) -> Result<()> {
    let parsed_id = Uuid::parse_str(&goal.id).context("goal id must be a UUID")?;
    if parsed_id.hyphenated().to_string() != goal.id {
        return Err(anyhow!("goal id must use canonical hyphenated UUID form"));
    }
    let goals_dir = ensure_goals_dir(session_dir)?;
    let runs_dir = goals_dir.join("runs");
    ensure_safe_directory_tree(&runs_dir)?;
    let dir = goal_artifact_dir(session_dir, &goal.id);
    std::fs::create_dir(&dir).with_context(|| {
        format!(
            "goal artifact directory already exists or could not be created: {}",
            dir.display()
        )
    })?;
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

    if let Err(error) = write_goal_artifacts(&dir, goal) {
        return match std::fs::remove_dir_all(&dir) {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(error.context(format!(
                "failed to remove partially initialized goal artifacts: {cleanup_error}"
            ))),
        };
    }
    if let Err(error) = sync_directory(&runs_dir) {
        return match std::fs::remove_dir_all(&dir) {
            Ok(()) => Err(error.into()),
            Err(cleanup_error) => Err(anyhow!(error).context(format!(
                "failed to remove goal artifacts after directory sync failure: {cleanup_error}"
            ))),
        };
    }
    Ok(())
}

fn write_goal_artifacts(dir: &Path, goal: &Goal) -> Result<()> {
    let phases_dir = dir.join("phases");
    ensure_safe_directory_tree(&phases_dir)?;
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

    write_file_atomic(&dir.join("GOAL.md"), goal_md.as_bytes())?;
    write_file_atomic(&dir.join("ROADMAP.md"), roadmap.as_bytes())?;
    write_file_atomic(&dir.join("STATE.md"), state.as_bytes())?;
    write_file_atomic(&dir.join("THINKING.md"), thinking.as_bytes())?;
    write_file_atomic(&dir.join("PROTOCOL.md"), protocol.as_bytes())?;
    for (index, phase) in goal.phases.iter().enumerate() {
        let path = phases_dir.join(format!("phase-{}.md", index + 1));
        let content = format!("# Phase {}\n\n{}\n", index + 1, phase);
        write_file_atomic(&path, content.as_bytes())?;
    }
    Ok(())
}

fn load_goal_store_sync(session_dir: &Path) -> Result<GoalStore> {
    let _ = ensure_goals_dir(session_dir)?;
    let path = active_store_path(session_dir);
    reject_reparse_or_symlink_if_present(&path)?;
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(GoalStore::new());
        }
        Err(error) => return Err(error.into()),
    };
    let mut store: GoalStore = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse goal store {}", path.display()))?;
    let has_goals = store.active.is_some() || !store.queue.is_empty();
    let runs_dir = goals_dir(session_dir).join("runs");
    if has_goals {
        validate_safe_directory(&runs_dir)?;
    } else {
        validate_safe_directory_if_present(&runs_dir)?;
    }
    for goal in store.active.iter_mut().chain(store.queue.iter_mut()) {
        let parsed_id = Uuid::parse_str(&goal.id).context("stored goal id must be a UUID")?;
        if parsed_id.hyphenated().to_string() != goal.id {
            return Err(anyhow!(
                "stored goal id must use canonical hyphenated UUID form"
            ));
        }
        let artifact_dir = goal_artifact_dir(session_dir, &goal.id);
        validate_safe_directory(&artifact_dir)?;
        goal.artifact_dir = Some(artifact_dir);
    }
    Ok(store)
}

pub async fn load_goal_store(session_dir: &Path) -> Result<GoalStore> {
    let session_dir = session_dir.to_path_buf();
    tokio::task::spawn_blocking(move || load_goal_store_sync(&session_dir))
        .await
        .context("goal store load task failed")?
}

fn save_goal_store(session_dir: &Path, store: &GoalStore) -> Result<AtomicWriteStatus> {
    let path = active_store_path(session_dir);
    let _ = ensure_goals_dir(session_dir)?;
    let content = serde_json::to_string_pretty(store)?;
    Ok(write_file_atomic_status(&path, content.as_bytes())?)
}

#[derive(Debug)]
struct GoalManagerState {
    session_dir: PathBuf,
    store: Mutex<GoalStore>,
    operation_lock: tokio::sync::Mutex<()>,
}

fn manager_registry() -> &'static Mutex<HashMap<PathBuf, Weak<GoalManagerState>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<GoalManagerState>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone)]
pub struct GoalManager {
    state: Arc<GoalManagerState>,
}

impl GoalManager {
    pub async fn load(session_dir: PathBuf) -> Result<Self> {
        let store = load_goal_store(&session_dir).await?;
        let session_dir = tokio::task::spawn_blocking(move || std::fs::canonicalize(session_dir))
            .await
            .context("goal session path normalization task failed")??;
        let mut registry = manager_registry().lock().map_err(|_| GoalError::Lock)?;
        if let Some(state) = registry.get(&session_dir).and_then(Weak::upgrade) {
            return Ok(Self { state });
        }
        let state = Arc::new(GoalManagerState {
            session_dir: session_dir.clone(),
            store: Mutex::new(store),
            operation_lock: tokio::sync::Mutex::new(()),
        });
        registry.insert(session_dir, Arc::downgrade(&state));
        Ok(Self { state })
    }

    #[must_use]
    pub fn active(&self) -> Option<Goal> {
        self.state.store.lock().ok()?.active().cloned()
    }

    fn snapshot(&self) -> Result<GoalStore> {
        Ok(self
            .state
            .store
            .lock()
            .map_err(|_| GoalError::Lock)?
            .clone())
    }

    fn publish(&self, store: GoalStore) -> Result<()> {
        *self.state.store.lock().map_err(|_| GoalError::Lock)? = store;
        Ok(())
    }

    fn contains_goal(&self, id: &str) -> bool {
        self.state.store.lock().is_ok_and(|store| {
            store.active().is_some_and(|goal| goal.id == id)
                || store.queue().iter().any(|goal| goal.id == id)
        })
    }

    fn commit_store(&self, proposed: GoalStore) -> Result<()> {
        match save_goal_store(&self.state.session_dir, &proposed) {
            Ok(AtomicWriteStatus::Durable) => self.publish(proposed),
            Ok(AtomicWriteStatus::CommittedUnsynced(error)) => {
                self.publish(proposed)?;
                Err(GoalError::CommittedUnsynced(error).into())
            }
            Err(write_error) => match load_goal_store_sync(&self.state.session_dir) {
                Ok(observed) => {
                    self.publish(observed)?;
                    Err(anyhow!(
                        "goal store write failed: {write_error:#}; in-memory state was reconciled from active.json"
                    ))
                }
                Err(reload_error) => Err(anyhow!(
                    "goal store write failed: {write_error:#}; active.json could not be reconciled: {reload_error:#}"
                )),
            },
        }
    }

    fn commit_new_goal(&self, proposed: GoalStore, goal: &Goal) -> Result<()> {
        if let Err(error) = self.commit_store(proposed) {
            if !self.contains_goal(&goal.id)
                && let Some(artifact_dir) = goal.artifact_dir.as_ref()
                && let Err(cleanup_error) = std::fs::remove_dir_all(artifact_dir)
                && cleanup_error.kind() != std::io::ErrorKind::NotFound
            {
                return Err(error.context(format!(
                    "failed to remove uncommitted goal artifacts: {cleanup_error}"
                )));
            }
            return Err(error);
        }
        Ok(())
    }

    async fn run_operation<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(GoalManager) -> Result<T> + Send + 'static,
    {
        let manager = self.clone();
        tokio::spawn(async move {
            let state = Arc::clone(&manager.state);
            let _operation = state.operation_lock.lock().await;
            tokio::task::spawn_blocking(move || operation(manager))
                .await
                .context("goal operation task failed")?
        })
        .await
        .context("goal operation coordinator failed")?
    }

    pub async fn start(&self, mut goal: Goal) -> Result<()> {
        self.run_operation(move |manager| {
            let mut proposed = manager.snapshot()?;
            if let Some(active) = proposed.active() {
                return Err(GoalError::ActiveGoal {
                    id: active.id.clone(),
                }
                .into());
            }
            ensure_goal_artifacts(&manager.state.session_dir, &mut goal)?;
            proposed.install(goal.clone());
            manager.commit_new_goal(proposed, &goal)
        })
        .await
    }

    pub async fn pause(&self) -> Result<Option<Goal>> {
        self.run_operation(|manager| {
            let mut proposed = manager.snapshot()?;
            let goal = proposed.pause();
            if goal.is_some() {
                manager.commit_store(proposed)?;
            }
            Ok(goal)
        })
        .await
    }

    pub async fn resume(&self) -> Result<Option<Goal>> {
        self.run_operation(|manager| {
            let mut proposed = manager.snapshot()?;
            let goal = proposed.resume();
            if goal.is_some() {
                manager.commit_store(proposed)?;
            }
            Ok(goal)
        })
        .await
    }

    pub async fn cancel(&self) -> Result<Option<Goal>> {
        self.run_operation(|manager| {
            let mut proposed = manager.snapshot()?;
            let goal = proposed.cancel();
            if goal.is_some() {
                manager.commit_store(proposed)?;
            }
            Ok(goal)
        })
        .await
    }

    pub async fn replace(&self, mut goal: Goal) -> Result<Option<Goal>> {
        self.run_operation(move |manager| {
            ensure_goal_artifacts(&manager.state.session_dir, &mut goal)?;
            let mut proposed = manager.snapshot()?;
            let previous = proposed.replace(goal.clone());
            manager.commit_new_goal(proposed, &goal)?;
            Ok(previous)
        })
        .await
    }

    pub async fn queue_next(&self, mut goal: Goal) -> Result<()> {
        self.run_operation(move |manager| {
            ensure_goal_artifacts(&manager.state.session_dir, &mut goal)?;
            let mut proposed = manager.snapshot()?;
            if proposed.active().is_some() {
                goal.status = GoalStatus::Queued;
                goal.touch();
                proposed.queue_next(goal.clone());
            } else {
                proposed.install(goal.clone());
            }
            manager.commit_new_goal(proposed, &goal)
        })
        .await
    }

    #[must_use]
    pub fn queue(&self) -> Vec<Goal> {
        let store = self.state.store.lock().ok();
        store
            .map(|store| store.queue().iter().cloned().collect())
            .unwrap_or_default()
    }

    pub async fn update_status(
        &self,
        status: GoalStatus,
        reason: Option<String>,
    ) -> Result<Option<Goal>> {
        self.run_operation(move |manager| {
            let mut proposed = manager.snapshot()?;
            let goal = {
                let Some(mut goal) = proposed.active_mut().cloned() else {
                    return Ok(None);
                };
                goal.status = status;
                goal.blocked_reason = reason;
                goal.touch();
                if matches!(status, GoalStatus::Complete) {
                    let _ = proposed.cancel();
                    let next = proposed.dequeue_next();
                    if let Some(next) = next {
                        proposed.install(next);
                    }
                    goal
                } else {
                    let Some(active) = proposed.active_mut() else {
                        return Err(anyhow!("active goal disappeared during status update"));
                    };
                    *active = goal.clone();
                    goal
                }
            };
            manager.commit_store(proposed)?;
            Ok(Some(goal))
        })
        .await
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GoalError {
    #[error("active goal `{id}` already exists; use replace to supersede it")]
    ActiveGoal { id: String },
    #[error("goal state was committed, but its directory entry could not be synchronized: {0}")]
    CommittedUnsynced(std::io::Error),
    #[error("goal store lock poisoned")]
    Lock,
}

impl GoalError {
    #[must_use]
    pub fn is_committed_unsynced(error: &anyhow::Error) -> bool {
        matches!(
            error.downcast_ref::<Self>(),
            Some(Self::CommittedUnsynced(_))
        )
    }
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
