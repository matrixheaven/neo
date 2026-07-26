//! Global workflow admission: actual VM/worker/executor occupancy and storage.
//!
//! The admission component owns permits, not lifecycle. When a permit is
//! unavailable the durable run/item stays queued with a fair FIFO position —
//! no timeout is inferred and pause/stop remain available.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use super::error::{WorkflowError, WorkflowErrorCode};
use super::limits::WorkflowLimits;
use super::state::WorkflowId;

/// Why an admission request could not be granted immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionReason {
    ActiveVmsExhausted,
    ActiveWorkersExhausted,
    ActiveExecutorsExhausted,
    GlobalStorageExhausted,
    PendingRecordBytesExhausted,
}

impl AdmissionReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ActiveVmsExhausted => "active_vms_exhausted",
            Self::ActiveWorkersExhausted => "active_workers_exhausted",
            Self::ActiveExecutorsExhausted => "active_executors_exhausted",
            Self::GlobalStorageExhausted => "global_storage_exhausted",
            Self::PendingRecordBytesExhausted => "pending_record_bytes_exhausted",
        }
    }
}

impl std::fmt::Display for AdmissionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Outcome of a non-blocking worker admission attempt.
#[derive(Debug)]
pub enum AdmitOutcome {
    /// Occupancy granted; drop (or explicit release) returns the permits.
    Granted(WorkerPermit),
    /// Durable work stays queued; position is 1-based FIFO rank among waiters.
    Queued {
        position: usize,
        reason: AdmissionReason,
    },
}

/// Snapshot of live admission occupancy (descriptive; live grants remain authoritative).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionOccupancy {
    pub active_vms: usize,
    pub active_workers: usize,
    pub active_executors: usize,
    pub reserved_storage_bytes: u64,
    pub reserved_pending_bytes: u64,
    pub queued_workers: usize,
}

/// Host-owned global admission controller.
#[derive(Clone)]
pub struct WorkflowAdmission {
    inner: Arc<AdmissionInner>,
}

struct AdmissionInner {
    limits: WorkflowLimits,
    state: Mutex<AdmissionState>,
}

struct AdmissionState {
    active_vms: usize,
    active_workers: usize,
    active_executors: usize,
    reserved_storage_bytes: u64,
    reserved_pending_bytes: u64,
    storage_by_owner: HashMap<String, u64>,
    pending_by_owner: HashMap<String, u64>,
    /// Fair FIFO of run ids waiting for a worker+VM pair.
    worker_queue: VecDeque<String>,
    /// Runs currently holding a worker+VM permit.
    active_worker_runs: HashMap<String, ActiveHold>,
    /// Runs currently holding an executor permit.
    active_executor_runs: HashMap<String, usize>,
}

struct ActiveHold {
    holds_vm: bool,
    holds_worker: bool,
}

impl WorkflowAdmission {
    #[must_use]
    pub fn new(limits: WorkflowLimits) -> Self {
        Self {
            inner: Arc::new(AdmissionInner {
                limits,
                state: Mutex::new(AdmissionState {
                    active_vms: 0,
                    active_workers: 0,
                    active_executors: 0,
                    reserved_storage_bytes: 0,
                    reserved_pending_bytes: 0,
                    storage_by_owner: HashMap::new(),
                    pending_by_owner: HashMap::new(),
                    worker_queue: VecDeque::new(),
                    active_worker_runs: HashMap::new(),
                    active_executor_runs: HashMap::new(),
                }),
            }),
        }
    }

    #[must_use]
    pub fn limits(&self) -> &WorkflowLimits {
        &self.inner.limits
    }

    fn lock(&self) -> MutexGuard<'_, AdmissionState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Descriptive occupancy snapshot for `/tasks` and tests.
    #[must_use]
    pub fn occupancy(&self) -> AdmissionOccupancy {
        let state = self.lock();
        AdmissionOccupancy {
            active_vms: state.active_vms,
            active_workers: state.active_workers,
            active_executors: state.active_executors,
            reserved_storage_bytes: state.reserved_storage_bytes,
            reserved_pending_bytes: state.reserved_pending_bytes,
            queued_workers: state.worker_queue.len(),
        }
    }

    /// 1-based FIFO queue position when the run is waiting; `None` if not queued.
    #[must_use]
    pub fn worker_queue_position(&self, run_id: &WorkflowId) -> Option<usize> {
        let state = self.lock();
        state
            .worker_queue
            .iter()
            .position(|id| id == run_id.as_str())
            .map(|index| index + 1)
    }

    /// Non-blocking fair worker+VM admission.
    ///
    /// When capacity is unavailable the run stays registered in the FIFO queue
    /// and the caller must leave durable state `queued` (no timeout, no failure).
    pub fn try_admit_worker(&self, run_id: &WorkflowId) -> AdmitOutcome {
        let mut state = self.lock();
        let id = run_id.as_str().to_owned();

        if state.active_worker_runs.contains_key(&id) {
            // Idempotent re-admit while already holding: re-issue a permit that
            // only releases once (the held occupancy is shared).
            return AdmitOutcome::Granted(WorkerPermit {
                admission: self.clone(),
                run_id: id,
                holds_vm: true,
                holds_worker: true,
                active: true,
            });
        }

        if !state.worker_queue.iter().any(|queued| queued == &id) {
            state.worker_queue.push_back(id.clone());
        }

        let position = state
            .worker_queue
            .iter()
            .position(|queued| queued == &id)
            .map(|index| index + 1)
            .unwrap_or(1);

        let reason = worker_capacity_reason(&state, &self.inner.limits);
        let is_head = state.worker_queue.front().is_some_and(|front| front == &id);
        if reason.is_some() || !is_head {
            return AdmitOutcome::Queued {
                position,
                reason: reason.unwrap_or(AdmissionReason::ActiveWorkersExhausted),
            };
        }

        // Head of queue and capacity available.
        state.worker_queue.pop_front();
        state.active_vms = state.active_vms.saturating_add(1);
        state.active_workers = state.active_workers.saturating_add(1);
        state.active_worker_runs.insert(
            id.clone(),
            ActiveHold {
                holds_vm: true,
                holds_worker: true,
            },
        );

        AdmitOutcome::Granted(WorkerPermit {
            admission: self.clone(),
            run_id: id,
            holds_vm: true,
            holds_worker: true,
            active: true,
        })
    }

    /// Remove a still-queued waiter (rollback / cancel before start).
    pub fn dequeue_worker(&self, run_id: &WorkflowId) {
        let mut state = self.lock();
        state.worker_queue.retain(|id| id != run_id.as_str());
    }

    /// Explicit release of a worker+VM hold. Safe to call multiple times.
    pub fn release_worker(&self, run_id: &WorkflowId) {
        let mut state = self.lock();
        release_worker_locked(&mut state, run_id.as_str());
    }

    /// Try to acquire one executor slot for child/effect work.
    pub fn try_admit_executor(
        &self,
        run_id: &WorkflowId,
    ) -> Result<ExecutorPermit, AdmissionReason> {
        let mut state = self.lock();
        if state.active_executors >= self.inner.limits.max_active_executors {
            return Err(AdmissionReason::ActiveExecutorsExhausted);
        }
        state.active_executors = state.active_executors.saturating_add(1);
        let id = run_id.as_str().to_owned();
        *state.active_executor_runs.entry(id.clone()).or_insert(0) += 1;
        Ok(ExecutorPermit {
            admission: self.clone(),
            run_id: id,
            active: true,
        })
    }

    pub fn release_executor(&self, run_id: &WorkflowId) {
        let mut state = self.lock();
        release_executor_locked(&mut state, run_id.as_str());
    }

    /// Atomically reserve global storage bytes for an owner (typically a run id).
    ///
    /// Concurrent callers serialize on the admission mutex so total reserved
    /// bytes never exceed `global_storage_bytes`.
    pub fn try_reserve_storage(
        &self,
        owner: &str,
        bytes: u64,
    ) -> Result<StorageReservation, WorkflowError> {
        if bytes == 0 {
            return Ok(StorageReservation {
                admission: self.clone(),
                owner: owner.to_owned(),
                bytes: 0,
                active: false,
            });
        }
        let mut state = self.lock();
        let next = state
            .reserved_storage_bytes
            .checked_add(bytes)
            .ok_or_else(|| {
                WorkflowError::coded(
                    WorkflowErrorCode::StorageAdmissionDenied,
                    "global storage reservation overflow",
                )
            })?;
        if next > self.inner.limits.global_storage_bytes {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::StorageAdmissionDenied,
                format!(
                    "global storage reservation of {bytes} bytes denied (reserved={}, limit={})",
                    state.reserved_storage_bytes, self.inner.limits.global_storage_bytes
                ),
            ));
        }
        state.reserved_storage_bytes = next;
        *state.storage_by_owner.entry(owner.to_owned()).or_insert(0) += bytes;
        Ok(StorageReservation {
            admission: self.clone(),
            owner: owner.to_owned(),
            bytes,
            active: true,
        })
    }

    /// Release all storage held by an owner (terminal prune / rollback).
    pub fn release_storage_owner(&self, owner: &str) {
        let mut state = self.lock();
        if let Some(bytes) = state.storage_by_owner.remove(owner) {
            state.reserved_storage_bytes = state.reserved_storage_bytes.saturating_sub(bytes);
        }
    }

    /// Atomically reserve pending (in-flight) record bytes.
    pub fn try_reserve_pending(&self, owner: &str, bytes: u64) -> Result<(), AdmissionReason> {
        if bytes == 0 {
            return Ok(());
        }
        let mut state = self.lock();
        let next = state
            .reserved_pending_bytes
            .checked_add(bytes)
            .ok_or(AdmissionReason::PendingRecordBytesExhausted)?;
        if next > self.inner.limits.pending_record_bytes {
            return Err(AdmissionReason::PendingRecordBytesExhausted);
        }
        state.reserved_pending_bytes = next;
        *state.pending_by_owner.entry(owner.to_owned()).or_insert(0) += bytes;
        Ok(())
    }

    pub fn release_pending_owner(&self, owner: &str) {
        let mut state = self.lock();
        if let Some(bytes) = state.pending_by_owner.remove(owner) {
            state.reserved_pending_bytes = state.reserved_pending_bytes.saturating_sub(bytes);
        }
    }

    /// Release every occupancy class held by a run (worker/VM, executor, pending).
    /// Storage is retained until explicit prune/rollback of the durable owner.
    pub fn release_run_occupancy(&self, run_id: &WorkflowId) {
        let mut state = self.lock();
        let id = run_id.as_str();
        release_worker_locked(&mut state, id);
        while state
            .active_executor_runs
            .get(id)
            .is_some_and(|count| *count > 0)
        {
            release_executor_locked(&mut state, id);
        }
        if let Some(bytes) = state.pending_by_owner.remove(id) {
            state.reserved_pending_bytes = state.reserved_pending_bytes.saturating_sub(bytes);
        }
        state.worker_queue.retain(|queued| queued != id);
    }
}

fn worker_capacity_reason(
    state: &AdmissionState,
    limits: &WorkflowLimits,
) -> Option<AdmissionReason> {
    if state.active_workers >= limits.max_active_workers {
        Some(AdmissionReason::ActiveWorkersExhausted)
    } else if state.active_vms >= limits.max_active_vms {
        Some(AdmissionReason::ActiveVmsExhausted)
    } else {
        None
    }
}

fn release_worker_locked(state: &mut AdmissionState, run_id: &str) {
    if let Some(hold) = state.active_worker_runs.remove(run_id) {
        if hold.holds_worker {
            state.active_workers = state.active_workers.saturating_sub(1);
        }
        if hold.holds_vm {
            state.active_vms = state.active_vms.saturating_sub(1);
        }
    }
}

fn release_executor_locked(state: &mut AdmissionState, run_id: &str) {
    let Some(count) = state.active_executor_runs.get_mut(run_id) else {
        return;
    };
    if *count == 0 {
        state.active_executor_runs.remove(run_id);
        return;
    }
    *count -= 1;
    state.active_executors = state.active_executors.saturating_sub(1);
    if *count == 0 {
        state.active_executor_runs.remove(run_id);
    }
}

/// RAII worker+VM permit. Drop releases occupancy on every exit path.
#[derive(Debug)]
pub struct WorkerPermit {
    admission: WorkflowAdmission,
    run_id: String,
    holds_vm: bool,
    holds_worker: bool,
    active: bool,
}

impl WorkerPermit {
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Disarm without releasing (when ownership moves to RunState and another
    /// path will call `release_run_occupancy`). Prefer Drop for normal paths.
    pub fn disarm(&mut self) {
        self.active = false;
    }

    pub fn release(mut self) {
        self.release_now();
    }

    fn release_now(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let mut state = self.admission.lock();
        // Only release if this permit still matches the active hold.
        if let Some(hold) = state.active_worker_runs.get(&self.run_id)
            && hold.holds_vm == self.holds_vm
            && hold.holds_worker == self.holds_worker
        {
            release_worker_locked(&mut state, &self.run_id);
        }
    }
}

impl Drop for WorkerPermit {
    fn drop(&mut self) {
        self.release_now();
    }
}

/// RAII executor permit.
#[derive(Debug)]
pub struct ExecutorPermit {
    admission: WorkflowAdmission,
    run_id: String,
    active: bool,
}

impl ExecutorPermit {
    pub fn release(mut self) {
        self.release_now();
    }

    fn release_now(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let mut state = self.admission.lock();
        release_executor_locked(&mut state, &self.run_id);
    }
}

impl Drop for ExecutorPermit {
    fn drop(&mut self) {
        self.release_now();
    }
}

/// RAII global storage reservation for one owner increment.
#[derive(Debug)]
pub struct StorageReservation {
    admission: WorkflowAdmission,
    owner: String,
    bytes: u64,
    active: bool,
}

impl StorageReservation {
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Keep the reservation for the durable run lifetime (do not release on drop).
    pub fn commit(mut self) {
        self.active = false;
    }

    pub fn release(mut self) {
        self.release_now();
    }

    fn release_now(&mut self) {
        if !self.active || self.bytes == 0 {
            self.active = false;
            return;
        }
        self.active = false;
        let mut state = self.admission.lock();
        let Some(held) = state.storage_by_owner.get(&self.owner).copied() else {
            return;
        };
        let release = held.min(self.bytes);
        let remaining = held.saturating_sub(release);
        if remaining == 0 {
            state.storage_by_owner.remove(&self.owner);
        } else {
            state.storage_by_owner.insert(self.owner.clone(), remaining);
        }
        state.reserved_storage_bytes = state.reserved_storage_bytes.saturating_sub(release);
    }
}

impl Drop for StorageReservation {
    fn drop(&mut self) {
        self.release_now();
    }
}

impl std::fmt::Debug for WorkflowAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let occupancy = self.occupancy();
        f.debug_struct("WorkflowAdmission")
            .field("occupancy", &occupancy)
            .finish_non_exhaustive()
    }
}
