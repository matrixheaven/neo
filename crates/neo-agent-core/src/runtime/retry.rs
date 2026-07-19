use std::time::Duration;

use neo_ai::AiError;
use rand::Rng as _;
use tokio_util::sync::CancellationToken;

use super::error::AgentRuntimeError;

const MAX_RETRY_AFTER: Duration = Duration::from_hours(24);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const MAX_BACKOFF_MS: u64 = 30_000;

pub(super) fn retry_delay(error: &AiError, retry: u32) -> Duration {
    let retry_after = match error {
        AiError::RateLimit { retry_after, .. } | AiError::Server { retry_after, .. } => {
            *retry_after
        }
        _ => None,
    };
    if let Some(delay) = retry_after {
        return delay.min(MAX_RETRY_AFTER);
    }

    let multiplier = 1_u64
        .checked_shl(retry.saturating_sub(1))
        .unwrap_or(u64::MAX);
    let base_ms = 500_u64.saturating_mul(multiplier);
    if base_ms >= MAX_BACKOFF_MS {
        return MAX_BACKOFF;
    }
    let jitter_ms = rand::rng().random_range(0..=base_ms / 4);
    Duration::from_millis(base_ms.saturating_add(jitter_ms)).min(MAX_BACKOFF)
}

pub(super) async fn wait_for_retry(
    delay: Duration,
    cancel_token: &CancellationToken,
) -> Result<(), AgentRuntimeError> {
    tokio::select! {
        biased;
        () = cancel_token.cancelled() => Err(AiError::Cancelled.into()),
        () = tokio::time::sleep(delay) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use neo_ai::AiError;
    use tokio_util::sync::CancellationToken;

    use super::{retry_delay, wait_for_retry};
    use crate::AgentRuntimeError;

    #[test]
    fn retry_after_is_exact_and_clamped() {
        assert_eq!(
            retry_delay(
                &AiError::RateLimit {
                    message: "slow down".into(),
                    retry_after: Some(Duration::from_secs(7)),
                },
                1,
            ),
            Duration::from_secs(7)
        );
        assert_eq!(
            retry_delay(
                &AiError::Server {
                    status: 503,
                    message: "later".into(),
                    retry_after: Some(Duration::from_hours(25)),
                },
                1,
            ),
            Duration::from_hours(24)
        );
    }

    #[test]
    fn exponential_delay_has_bounded_jitter_and_cap() {
        for _ in 0..32 {
            let delay = retry_delay(
                &AiError::Transport {
                    message: "eof".into(),
                },
                1,
            );
            assert!(delay >= Duration::from_millis(500));
            assert!(delay <= Duration::from_millis(625));
        }

        assert_eq!(
            retry_delay(
                &AiError::Transport {
                    message: "eof".into(),
                },
                u32::MAX,
            ),
            Duration::from_secs(30)
        );
    }

    #[tokio::test]
    async fn retry_wait_prefers_cancellation_over_ready_delay() {
        let cancel = CancellationToken::new();
        cancel.cancel();

        let error = wait_for_retry(Duration::ZERO, &cancel)
            .await
            .expect_err("cancellation should win when the delay is also ready");
        assert!(matches!(
            error,
            AgentRuntimeError::Model(AiError::Cancelled)
        ));
    }
}
