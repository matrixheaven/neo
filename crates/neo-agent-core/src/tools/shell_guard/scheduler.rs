use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use tokio::sync::oneshot;

/// Hard cap on concurrently running agent-background commands (background Bash +
/// Terminal Start). Independent of total capacity except when capacity is lower.
pub(crate) const MAX_AGENT_BACKGROUND_COMMANDS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellAdmissionClass {
    User,
    AgentForeground,
    AgentBackground,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellAdmissionRequest {
    pub owner: String,
    pub class: ShellAdmissionClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellAdmissionEvent {
    Queued,
    Position { position: usize, waiting: Duration },
    Started,
}

pub type ShellAdmissionCallback = Arc<dyn Fn(ShellAdmissionEvent) + Send + Sync>;

#[derive(Debug)]
pub(crate) struct ShellScheduler {
    capacity: usize,
    state: Mutex<SchedulerState>,
}

impl std::fmt::Debug for SchedulerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SchedulerState")
            .field("running_total", &self.running_total)
            .field("running_background", &self.running_background)
            .field("queued", &self.queued_count())
            .finish_non_exhaustive()
    }
}

struct SchedulerState {
    user: VecDeque<Waiter>,
    foreground_owners: HashMap<String, VecDeque<Waiter>>,
    foreground_ring: VecDeque<String>,
    background_owners: HashMap<String, VecDeque<Waiter>>,
    background_ring: VecDeque<String>,
    running_total: usize,
    running_background: usize,
    next_waiter_id: u64,
}

struct Waiter {
    id: u64,
    owner: String,
    class: ShellAdmissionClass,
    enqueued_at: Instant,
    grant_tx: Option<oneshot::Sender<ShellCommandPermit>>,
    callback: Option<ShellAdmissionCallback>,
    /// Set after the acquire path has emitted Queued + initial Position so
    /// concurrent mutations never deliver Position before Queued.
    ready: bool,
}

struct Grant {
    tx: oneshot::Sender<ShellCommandPermit>,
    permit: ShellCommandPermit,
}

struct PositionNotice {
    callback: ShellAdmissionCallback,
    position: usize,
    waiting: Duration,
}

pub(crate) struct ShellCommandPermit {
    scheduler: Arc<ShellScheduler>,
    class: ShellAdmissionClass,
}

impl std::fmt::Debug for ShellCommandPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellCommandPermit")
            .field("class", &self.class)
            .finish_non_exhaustive()
    }
}

impl Drop for ShellCommandPermit {
    fn drop(&mut self) {
        self.scheduler.release(self.class);
    }
}

/// Removes a still-queued waiter when the acquire future is dropped.
struct WaitRegistration {
    id: u64,
    scheduler: Weak<ShellScheduler>,
    armed: bool,
}

impl WaitRegistration {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for WaitRegistration {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(scheduler) = self.scheduler.upgrade() {
            scheduler.cancel_waiter(self.id);
        }
    }
}

impl ShellScheduler {
    #[must_use]
    pub(crate) fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            capacity: capacity.max(1),
            state: Mutex::new(SchedulerState {
                user: VecDeque::new(),
                foreground_owners: HashMap::new(),
                foreground_ring: VecDeque::new(),
                background_owners: HashMap::new(),
                background_ring: VecDeque::new(),
                running_total: 0,
                running_background: 0,
                next_waiter_id: 1,
            }),
        })
    }

    pub(crate) async fn acquire(
        self: &Arc<Self>,
        request: ShellAdmissionRequest,
        callback: Option<ShellAdmissionCallback>,
    ) -> ShellCommandPermit {
        let class = request.class;
        let (tx, rx) = oneshot::channel();
        let enqueued_at = Instant::now();
        let (waiter_id, granted_immediately, initial_position, grants, notices) = {
            let mut state = self.state.lock().expect("shell scheduler mutex");
            let id = state.next_waiter_id;
            state.next_waiter_id = state.next_waiter_id.saturating_add(1);
            state.enqueue(Waiter {
                id,
                owner: request.owner,
                class,
                enqueued_at,
                grant_tx: Some(tx),
                callback: callback.clone(),
                ready: false,
            });
            let grants = Self::dispatch(&mut state, self.capacity, self);
            let still_queued = state.contains_id(id);
            let granted_immediately = !still_queued;
            let initial_position = if still_queued {
                state.position_of(id, class)
            } else {
                None
            };
            let notices = state.position_notices_all();
            (id, granted_immediately, initial_position, grants, notices)
        };

        Self::deliver(grants, notices);

        if granted_immediately {
            return rx
                .await
                .expect("immediately granted shell permit must be delivered");
        }

        if let Some(callback) = &callback {
            callback(ShellAdmissionEvent::Queued);
            if let Some(position) = initial_position {
                callback(ShellAdmissionEvent::Position {
                    position,
                    waiting: enqueued_at.elapsed(),
                });
            }
        }

        // Mark ready and emit a correction if rank changed during the initial
        // Queued/Position notification window.
        let correction = {
            let mut state = self.state.lock().expect("shell scheduler mutex");
            if !state.contains_id(waiter_id) {
                None
            } else {
                if let Some(waiter) = state.waiter_mut(waiter_id) {
                    waiter.ready = true;
                }
                let waiting = state
                    .waiter_ref(waiter_id)
                    .map(|waiter| waiter.enqueued_at.elapsed())
                    .unwrap_or_else(|| enqueued_at.elapsed());
                state
                    .position_of(waiter_id, class)
                    .filter(|&position| Some(position) != initial_position)
                    .map(|position| (position, waiting))
            }
        };
        if let (Some(callback), Some((position, waiting))) = (&callback, correction) {
            callback(ShellAdmissionEvent::Position { position, waiting });
        }

        let mut registration = WaitRegistration {
            id: waiter_id,
            scheduler: Arc::downgrade(self),
            armed: true,
        };
        let permit = rx.await.expect("queued shell permit must be delivered");
        registration.disarm();
        permit
    }

    fn release(self: &Arc<Self>, class: ShellAdmissionClass) {
        let (grants, notices) = {
            let mut state = self.state.lock().expect("shell scheduler mutex");
            state.running_total = state.running_total.saturating_sub(1);
            if matches!(class, ShellAdmissionClass::AgentBackground) {
                state.running_background = state.running_background.saturating_sub(1);
            }
            let grants = Self::dispatch(&mut state, self.capacity, self);
            let notices = state.position_notices_all();
            (grants, notices)
        };
        Self::deliver(grants, notices);
    }

    fn cancel_waiter(self: &Arc<Self>, id: u64) {
        let notices = {
            let mut state = self.state.lock().expect("shell scheduler mutex");
            if !state.remove_waiter(id) {
                return;
            }
            state.position_notices_all()
        };
        Self::deliver(Vec::new(), notices);
    }

    fn dispatch(state: &mut SchedulerState, capacity: usize, scheduler: &Arc<Self>) -> Vec<Grant> {
        let mut grants = Vec::new();
        while state.running_total < capacity {
            let Some(mut waiter) = state.pop_next_eligible(capacity) else {
                break;
            };
            let class = waiter.class;
            let Some(tx) = waiter.grant_tx.take() else {
                continue;
            };
            state.running_total = state.running_total.saturating_add(1);
            if matches!(class, ShellAdmissionClass::AgentBackground) {
                state.running_background = state.running_background.saturating_add(1);
            }
            grants.push(Grant {
                tx,
                permit: ShellCommandPermit {
                    scheduler: Arc::clone(scheduler),
                    class,
                },
            });
        }
        grants
    }

    fn deliver(grants: Vec<Grant>, notices: Vec<PositionNotice>) {
        for Grant { tx, permit } in grants {
            if let Err(permit) = tx.send(permit) {
                // Receiver already dropped: permit Drop releases capacity and
                // redispatches under a fresh lock.
                drop(permit);
            }
        }
        for notice in notices {
            (notice.callback)(ShellAdmissionEvent::Position {
                position: notice.position,
                waiting: notice.waiting,
            });
        }
    }

    #[cfg(test)]
    pub(crate) fn running_counts(&self) -> (usize, usize) {
        let state = self.state.lock().expect("shell scheduler mutex");
        (state.running_total, state.running_background)
    }

    #[cfg(test)]
    pub(crate) fn queued_count(&self) -> usize {
        let state = self.state.lock().expect("shell scheduler mutex");
        state.queued_count()
    }
}

impl SchedulerState {
    fn enqueue(&mut self, waiter: Waiter) {
        match waiter.class {
            ShellAdmissionClass::User => self.user.push_back(waiter),
            ShellAdmissionClass::AgentForeground => {
                let owner = waiter.owner.clone();
                let queue = self.foreground_owners.entry(owner.clone()).or_default();
                let was_empty = queue.is_empty();
                queue.push_back(waiter);
                if was_empty {
                    self.foreground_ring.push_back(owner);
                }
            }
            ShellAdmissionClass::AgentBackground => {
                let owner = waiter.owner.clone();
                let queue = self.background_owners.entry(owner.clone()).or_default();
                let was_empty = queue.is_empty();
                queue.push_back(waiter);
                if was_empty {
                    self.background_ring.push_back(owner);
                }
            }
        }
    }

    fn background_limit(capacity: usize) -> usize {
        capacity.min(MAX_AGENT_BACKGROUND_COMMANDS)
    }

    fn pop_next_eligible(&mut self, capacity: usize) -> Option<Waiter> {
        if !self.user.is_empty() {
            return self.user.pop_front();
        }
        if let Some(waiter) =
            Self::pop_owner_class(&mut self.foreground_owners, &mut self.foreground_ring)
        {
            return Some(waiter);
        }
        if self.running_background < Self::background_limit(capacity) {
            return Self::pop_owner_class(&mut self.background_owners, &mut self.background_ring);
        }
        None
    }

    fn pop_owner_class(
        owners: &mut HashMap<String, VecDeque<Waiter>>,
        ring: &mut VecDeque<String>,
    ) -> Option<Waiter> {
        while let Some(owner) = ring.pop_front() {
            let Some(queue) = owners.get_mut(&owner) else {
                continue;
            };
            let Some(waiter) = queue.pop_front() else {
                owners.remove(&owner);
                continue;
            };
            if queue.is_empty() {
                owners.remove(&owner);
            } else {
                ring.push_back(owner);
            }
            return Some(waiter);
        }
        None
    }

    fn contains_id(&self, id: u64) -> bool {
        self.user.iter().any(|waiter| waiter.id == id)
            || self
                .foreground_owners
                .values()
                .any(|queue| queue.iter().any(|waiter| waiter.id == id))
            || self
                .background_owners
                .values()
                .any(|queue| queue.iter().any(|waiter| waiter.id == id))
    }

    fn waiter_mut(&mut self, id: u64) -> Option<&mut Waiter> {
        if let Some(waiter) = self.user.iter_mut().find(|waiter| waiter.id == id) {
            return Some(waiter);
        }
        for queue in self.foreground_owners.values_mut() {
            if let Some(waiter) = queue.iter_mut().find(|waiter| waiter.id == id) {
                return Some(waiter);
            }
        }
        for queue in self.background_owners.values_mut() {
            if let Some(waiter) = queue.iter_mut().find(|waiter| waiter.id == id) {
                return Some(waiter);
            }
        }
        None
    }

    fn waiter_ref(&self, id: u64) -> Option<&Waiter> {
        if let Some(waiter) = self.user.iter().find(|waiter| waiter.id == id) {
            return Some(waiter);
        }
        for queue in self.foreground_owners.values() {
            if let Some(waiter) = queue.iter().find(|waiter| waiter.id == id) {
                return Some(waiter);
            }
        }
        for queue in self.background_owners.values() {
            if let Some(waiter) = queue.iter().find(|waiter| waiter.id == id) {
                return Some(waiter);
            }
        }
        None
    }

    fn remove_waiter(&mut self, id: u64) -> bool {
        if let Some(index) = self.user.iter().position(|waiter| waiter.id == id) {
            self.user.remove(index);
            return true;
        }
        if Self::remove_from_owner_class(&mut self.foreground_owners, &mut self.foreground_ring, id)
        {
            return true;
        }
        Self::remove_from_owner_class(&mut self.background_owners, &mut self.background_ring, id)
    }

    fn remove_from_owner_class(
        owners: &mut HashMap<String, VecDeque<Waiter>>,
        ring: &mut VecDeque<String>,
        id: u64,
    ) -> bool {
        let mut found: Option<(String, bool)> = None;
        for (owner, queue) in owners.iter_mut() {
            if let Some(index) = queue.iter().position(|waiter| waiter.id == id) {
                queue.remove(index);
                found = Some((owner.clone(), queue.is_empty()));
                break;
            }
        }
        match found {
            Some((owner, empty)) => {
                if empty {
                    owners.remove(&owner);
                    ring.retain(|candidate| candidate != &owner);
                }
                true
            }
            None => false,
        }
    }

    fn position_of(&self, id: u64, class: ShellAdmissionClass) -> Option<usize> {
        self.ordered_ids(class)
            .into_iter()
            .position(|candidate| candidate == id)
            .map(|index| index + 1)
    }

    fn ordered_ids(&self, class: ShellAdmissionClass) -> Vec<u64> {
        match class {
            ShellAdmissionClass::User => self.user.iter().map(|waiter| waiter.id).collect(),
            ShellAdmissionClass::AgentForeground => {
                Self::round_robin_ids(&self.foreground_owners, &self.foreground_ring)
            }
            ShellAdmissionClass::AgentBackground => {
                Self::round_robin_ids(&self.background_owners, &self.background_ring)
            }
        }
    }

    fn round_robin_ids(
        owners: &HashMap<String, VecDeque<Waiter>>,
        ring: &VecDeque<String>,
    ) -> Vec<u64> {
        let mut ring = ring.clone();
        let mut queues: HashMap<String, VecDeque<u64>> = owners
            .iter()
            .map(|(owner, queue)| {
                (
                    owner.clone(),
                    queue.iter().map(|waiter| waiter.id).collect(),
                )
            })
            .collect();
        let mut ordered = Vec::new();
        while let Some(owner) = ring.pop_front() {
            let Some(queue) = queues.get_mut(&owner) else {
                continue;
            };
            let Some(id) = queue.pop_front() else {
                continue;
            };
            ordered.push(id);
            if !queue.is_empty() {
                ring.push_back(owner);
            }
        }
        ordered
    }

    fn position_notices_all(&self) -> Vec<PositionNotice> {
        let mut notices = Vec::new();
        for class in [
            ShellAdmissionClass::User,
            ShellAdmissionClass::AgentForeground,
            ShellAdmissionClass::AgentBackground,
        ] {
            for (index, id) in self.ordered_ids(class).into_iter().enumerate() {
                let Some(waiter) = self.waiter_ref(id) else {
                    continue;
                };
                if !waiter.ready {
                    continue;
                }
                let Some(callback) = waiter.callback.clone() else {
                    continue;
                };
                notices.push(PositionNotice {
                    callback,
                    position: index + 1,
                    waiting: waiter.enqueued_at.elapsed(),
                });
            }
        }
        notices
    }

    fn queued_count(&self) -> usize {
        self.user.len()
            + self
                .foreground_owners
                .values()
                .map(VecDeque::len)
                .sum::<usize>()
            + self
                .background_owners
                .values()
                .map(VecDeque::len)
                .sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ShellAdmissionCallback, ShellAdmissionClass, ShellAdmissionEvent, ShellAdmissionRequest,
        ShellCommandPermit, ShellScheduler,
    };
    use crate::tools::shell_guard::{ShellLimits, ShellRuntime};
    use ShellAdmissionClass::{AgentBackground, AgentForeground, User};
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    fn agent(owner: &str, class: ShellAdmissionClass) -> ShellAdmissionRequest {
        ShellAdmissionRequest {
            owner: owner.to_owned(),
            class,
        }
    }

    fn spawn_waiter(
        scheduler: &Arc<ShellScheduler>,
        owner: &'static str,
        class: ShellAdmissionClass,
        label: &'static str,
        order: Arc<Mutex<Vec<&'static str>>>,
    ) -> tokio::task::JoinHandle<ShellCommandPermit> {
        let scheduler = scheduler.clone();
        tokio::spawn(async move {
            let permit = scheduler.acquire(agent(owner, class), None).await;
            order.lock().expect("order lock").push(label);
            permit
        })
    }

    async fn wait_for_queued(scheduler: &ShellScheduler, expected: usize) {
        while scheduler.queued_count() != expected {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn immediate_admission_does_not_emit_queue_events() {
        let scheduler = ShellScheduler::new(1);
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = events.clone();
        let callback: ShellAdmissionCallback = Arc::new(move |event| {
            observed.lock().expect("events lock").push(event);
        });
        let permit = scheduler
            .acquire(agent("a", AgentForeground), Some(callback))
            .await;
        assert!(events.lock().expect("events lock").is_empty());
        drop(permit);
    }

    #[tokio::test]
    async fn waits_at_capacity_and_grants_after_drop() {
        let scheduler = ShellScheduler::new(1);
        let first = scheduler.acquire(agent("a", AgentForeground), None).await;
        let second = tokio::spawn({
            let scheduler = scheduler.clone();
            async move { scheduler.acquire(agent("b", AgentForeground), None).await }
        });
        wait_for_queued(&scheduler, 1).await;
        assert!(!second.is_finished());
        drop(first);
        let permit = second.await.expect("waiter task");
        drop(permit);
        assert_eq!(scheduler.running_counts(), (0, 0));
    }

    #[tokio::test]
    async fn user_then_foreground_then_background_and_owner_round_robin() {
        let scheduler = ShellScheduler::new(1);
        let held = scheduler
            .acquire(agent("hold", AgentForeground), None)
            .await;
        let order = Arc::new(Mutex::new(Vec::new()));
        let bg_a1 = spawn_waiter(&scheduler, "a", AgentBackground, "bg-a1", order.clone());
        wait_for_queued(&scheduler, 1).await;
        let bg_a2 = spawn_waiter(&scheduler, "a", AgentBackground, "bg-a2", order.clone());
        wait_for_queued(&scheduler, 2).await;
        let bg_b1 = spawn_waiter(&scheduler, "b", AgentBackground, "bg-b1", order.clone());
        wait_for_queued(&scheduler, 3).await;
        let fg_a1 = spawn_waiter(&scheduler, "a", AgentForeground, "fg-a1", order.clone());
        wait_for_queued(&scheduler, 4).await;
        let fg_b1 = spawn_waiter(&scheduler, "b", AgentForeground, "fg-b1", order.clone());
        wait_for_queued(&scheduler, 5).await;
        let user = spawn_waiter(&scheduler, "user", User, "user", order.clone());
        wait_for_queued(&scheduler, 6).await;

        drop(held);
        drop(user.await.expect("user grant"));
        drop(fg_a1.await.expect("foreground a grant"));
        drop(fg_b1.await.expect("foreground b grant"));
        drop(bg_a1.await.expect("background a1 grant"));
        drop(bg_b1.await.expect("background b1 grant"));
        drop(bg_a2.await.expect("background a2 grant"));
        assert_eq!(
            *order.lock().expect("order lock"),
            ["user", "fg-a1", "fg-b1", "bg-a1", "bg-b1", "bg-a2"]
        );
    }

    #[tokio::test]
    async fn fourth_background_waits_while_same_owner_foreground_uses_fourth_slot() {
        let scheduler = ShellScheduler::new(4);
        let bg1 = scheduler.acquire(agent("a", AgentBackground), None).await;
        let bg2 = scheduler.acquire(agent("b", AgentBackground), None).await;
        let bg3 = scheduler.acquire(agent("c", AgentBackground), None).await;
        let bg4 = spawn_waiter(
            &scheduler,
            "d",
            AgentBackground,
            "bg4",
            Arc::new(Mutex::new(Vec::new())),
        );
        wait_for_queued(&scheduler, 1).await;
        assert!(!bg4.is_finished());
        let foreground = scheduler.acquire(agent("d", AgentForeground), None).await;
        assert_eq!(scheduler.running_counts(), (4, 3));
        drop(foreground);
        assert!(!bg4.is_finished());
        drop(bg1);
        drop(bg4.await.expect("fourth background grant"));
        drop(bg2);
        drop(bg3);
        assert_eq!(scheduler.running_counts(), (0, 0));
    }

    #[tokio::test]
    async fn dropping_waiter_during_grant_never_leaks_capacity() {
        let scheduler = ShellScheduler::new(1);
        for release_first in [false, true].into_iter().cycle().take(64) {
            let held = scheduler
                .acquire(agent("hold", AgentForeground), None)
                .await;
            let waiter = spawn_waiter(
                &scheduler,
                "cancelled",
                AgentForeground,
                "cancelled",
                Arc::new(Mutex::new(Vec::new())),
            );
            wait_for_queued(&scheduler, 1).await;
            if release_first {
                drop(held);
                tokio::task::yield_now().await;
                waiter.abort();
            } else {
                waiter.abort();
                drop(held);
            }
            let _ = waiter.await;
            let probe = scheduler
                .acquire(agent("probe", AgentForeground), None)
                .await;
            drop(probe);
            assert_eq!(scheduler.running_counts(), (0, 0));
        }
    }

    #[tokio::test]
    async fn positions_follow_class_local_owner_round_robin() {
        let scheduler = ShellScheduler::new(1);
        let held = scheduler
            .acquire(agent("hold", AgentForeground), None)
            .await;
        let positions = Arc::new(Mutex::new(HashMap::<&'static str, usize>::new()));
        let mut waiters = Vec::new();
        for (index, (owner, label)) in [("a", "a1"), ("b", "b1"), ("a", "a2")]
            .into_iter()
            .enumerate()
        {
            let observed = positions.clone();
            let callback: ShellAdmissionCallback = Arc::new(move |event| {
                if let ShellAdmissionEvent::Position { position, .. } = event {
                    observed
                        .lock()
                        .expect("positions lock")
                        .insert(label, position);
                }
            });
            let queued = scheduler.clone();
            waiters.push(tokio::spawn(async move {
                queued
                    .acquire(agent(owner, AgentForeground), Some(callback))
                    .await
            }));
            wait_for_queued(&scheduler, index + 1).await;
        }
        loop {
            let ready = positions.lock().expect("positions lock").len() == 3;
            if ready {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            *positions.lock().expect("positions lock"),
            HashMap::from([("a1", 1), ("b1", 2), ("a2", 3)])
        );
        drop(held);
        let first = waiters.remove(0).await.expect("first grant");
        loop {
            let ready = {
                let positions = positions.lock().expect("positions lock");
                positions.get("b1") == Some(&1) && positions.get("a2") == Some(&2)
            };
            if ready {
                break;
            }
            tokio::task::yield_now().await;
        }
        drop(first);
        for waiter in waiters {
            waiter.abort();
            let _ = waiter.await;
        }
        assert_eq!(scheduler.running_counts(), (0, 0));
    }

    #[tokio::test]
    async fn shell_runtime_clones_share_scheduler() {
        let runtime = ShellRuntime::for_tests(ShellLimits {
            max_active_commands: 1,
            ..ShellLimits::default()
        });
        let held = runtime.acquire(agent("a", AgentForeground), None).await;
        let clone = runtime.clone();
        let queued =
            tokio::spawn(async move { clone.acquire(agent("b", AgentForeground), None).await });
        while runtime.scheduler.queued_count() != 1 {
            tokio::task::yield_now().await;
        }
        assert!(!queued.is_finished());
        drop(held);
        drop(queued.await.expect("clone grant"));
        assert_eq!(runtime.scheduler.running_counts(), (0, 0));
    }
}
