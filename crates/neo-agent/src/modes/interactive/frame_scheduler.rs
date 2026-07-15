use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub(super) struct FrameScheduler {
    last_frame: Instant,
    min_interval: Duration,
    immediate: bool,
    coalesced: bool,
    animation_deadline: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrameDue {
    ImmediateOrCoalesced,
    Animation,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum FrameRequest {
    #[default]
    None,
    Coalesced,
    Immediate,
}

impl FrameRequest {
    pub(super) fn merge(self, request: Self) -> Self {
        match (self, request) {
            (Self::Immediate, _) | (_, Self::Immediate) => Self::Immediate,
            (Self::Coalesced, _) | (_, Self::Coalesced) => Self::Coalesced,
            (Self::None, Self::None) => Self::None,
        }
    }

    pub(super) fn schedule(self, scheduler: &mut FrameScheduler) {
        match self {
            Self::None => {}
            Self::Coalesced => scheduler.request_coalesced(),
            Self::Immediate => scheduler.request_immediate(),
        }
    }
}

impl FrameScheduler {
    pub(super) const fn new(last_frame: Instant, min_interval: Duration) -> Self {
        Self {
            last_frame,
            min_interval,
            immediate: false,
            coalesced: false,
            animation_deadline: None,
        }
    }

    pub(super) fn request_immediate(&mut self) {
        self.immediate = true;
    }

    pub(super) fn request_coalesced(&mut self) {
        self.coalesced = true;
    }

    pub(super) fn replace_animation_deadline(&mut self, deadline: Option<Instant>) {
        self.animation_deadline = deadline.map(|deadline| {
            self.animation_deadline
                .map_or(deadline, |scheduled| scheduled.min(deadline))
        });
    }

    pub(super) fn take_due(&mut self, now: Instant) -> Option<FrameDue> {
        let coalesced_due =
            self.coalesced && now.saturating_duration_since(self.last_frame) >= self.min_interval;
        let animation_due = self
            .animation_deadline
            .is_some_and(|deadline| deadline <= now);
        if !self.immediate && !coalesced_due && !animation_due {
            return None;
        }

        self.immediate = false;
        self.coalesced = false;
        if animation_due {
            self.animation_deadline = None;
        }
        self.last_frame = now;
        Some(if animation_due {
            FrameDue::Animation
        } else {
            FrameDue::ImmediateOrCoalesced
        })
    }

    pub(super) fn poll_timeout(&self, now: Instant, maximum: Duration) -> Duration {
        if self.immediate {
            return Duration::ZERO;
        }

        let mut timeout = maximum;
        if self.coalesced {
            let deadline = self
                .last_frame
                .checked_add(self.min_interval)
                .unwrap_or(now);
            timeout = timeout.min(deadline.saturating_duration_since(now));
        }
        if let Some(deadline) = self.animation_deadline {
            timeout = timeout.min(deadline.saturating_duration_since(now));
        }
        timeout
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameDue, FrameRequest, FrameScheduler};
    use std::time::{Duration, Instant};

    #[test]
    fn idle_poll_does_not_request_a_frame() {
        let now = Instant::now();
        let mut scheduler = FrameScheduler::new(now, Duration::from_millis(33));

        assert_eq!(scheduler.take_due(now + Duration::from_secs(1)), None);
    }

    #[test]
    fn coalesced_requests_wait_but_immediate_requests_do_not() {
        let now = Instant::now();
        let mut scheduler = FrameScheduler::new(now, Duration::from_millis(33));

        scheduler.request_coalesced();
        assert_eq!(scheduler.take_due(now + Duration::from_millis(10)), None);
        scheduler.request_immediate();
        assert_eq!(
            scheduler.take_due(now + Duration::from_millis(10)),
            Some(FrameDue::ImmediateOrCoalesced)
        );
    }

    #[test]
    fn animation_deadline_is_due_once_and_replaced_by_the_next_frame() {
        let now = Instant::now();
        let mut scheduler = FrameScheduler::new(now, Duration::from_millis(33));
        let deadline = now + Duration::from_millis(100);

        scheduler.replace_animation_deadline(Some(deadline));
        assert_eq!(scheduler.take_due(deadline), Some(FrameDue::Animation));
        assert_eq!(scheduler.take_due(deadline + Duration::from_secs(1)), None);

        let replacement = deadline + Duration::from_millis(100);
        scheduler.replace_animation_deadline(Some(replacement));
        assert_eq!(scheduler.take_due(replacement), Some(FrameDue::Animation));
    }

    #[test]
    fn coalesced_frames_do_not_postpone_animation_deadline() {
        let start = Instant::now();
        let mut scheduler = FrameScheduler::new(start, Duration::from_millis(33));
        let animation_deadline = start + Duration::from_millis(100);
        scheduler.replace_animation_deadline(Some(animation_deadline));

        for elapsed_ms in [33, 66, 99] {
            let now = start + Duration::from_millis(elapsed_ms);
            scheduler.request_coalesced();
            assert_eq!(
                scheduler.take_due(now),
                Some(FrameDue::ImmediateOrCoalesced)
            );
            scheduler.replace_animation_deadline(Some(now + Duration::from_millis(100)));
        }

        assert_eq!(
            scheduler.take_due(animation_deadline),
            Some(FrameDue::Animation)
        );
    }

    #[test]
    fn frame_request_merge_preserves_strongest_request() {
        assert_eq!(
            FrameRequest::None.merge(FrameRequest::Coalesced),
            FrameRequest::Coalesced
        );
        assert_eq!(
            FrameRequest::Coalesced.merge(FrameRequest::Immediate),
            FrameRequest::Immediate
        );
        assert_eq!(
            FrameRequest::Immediate.merge(FrameRequest::None),
            FrameRequest::Immediate
        );
    }
}
