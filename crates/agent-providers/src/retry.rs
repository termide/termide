//! Transport retry shared by the providers: a stream attempt is retried with
//! exponential backoff only while it has produced no content, so a partial
//! answer is never silently duplicated. Only failures that pass within
//! moments are retried here; a limit that resets later ends the call with
//! its reset time, for the panel to wait out or ask about.

use std::time::Duration;

use termide_agent_core::{
    AssistantMessage, CancelToken, Failure as Class, FailureKind, StopReason, StreamEvent,
};

/// A failed attempt and what kind of failure it was.
pub struct Failure {
    pub message: String,
    pub class: Class,
}

impl Failure {
    /// An HTTP error response: `body` is the response text, `retry_after`
    /// its `Retry-After` header.
    pub fn http(code: u16, body: &str, retry_after: Option<&str>, message: String) -> Self {
        Self {
            message: format!("HTTP {code}: {message}"),
            class: Class::from_http(code, body, retry_after),
        }
    }

    /// The connection failed or broke before any content arrived.
    pub fn transport(message: String) -> Self {
        Self {
            message,
            class: Class::new(FailureKind::Transient),
        }
    }

    /// An error the stream itself carried (`raw`, its JSON) before any
    /// content arrived.
    pub fn in_stream(raw: &str, message: String) -> Self {
        Self {
            message,
            class: Class::from_http(0, raw, None),
        }
    }
}

/// The longest wait for a stated reset that is retried here; a longer one
/// ends the call, so the run does not hang silently on a limit.
const MAX_INLINE_WAIT: Duration = Duration::from_secs(60);

/// How many times a request may be retried before content arrives, and how
/// long to wait first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(1),
        }
    }
}

/// How long to wait before the next attempt after `failure`, the backoff
/// being `backoff`; `None` when it is not retried here.
fn inline_wait(failure: &Class, backoff: Duration) -> Option<Duration> {
    if !matches!(
        failure.kind,
        FailureKind::Transient | FailureKind::RateLimited
    ) {
        return None;
    }
    let Some(at) = failure.resets_at else {
        return Some(backoff);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let until = Duration::from_secs(at.saturating_sub(now));
    (until <= MAX_INLINE_WAIT).then(|| until.max(backoff))
}

/// Run `attempt` until it succeeds, fails unretryably, or the attempts run
/// out. Between tries it emits a `Retry` event and sleeps, waking often to
/// notice a cancel. `attempt` gets the live event sink for its deltas. The
/// message of a final failure carries its classification.
pub fn with_retries(
    provider: &str,
    model: &str,
    policy: RetryPolicy,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(StreamEvent),
    mut attempt: impl FnMut(&mut dyn FnMut(StreamEvent)) -> Result<AssistantMessage, Failure>,
) -> AssistantMessage {
    let max_attempts = policy.max_attempts.max(1);
    let mut n = 1;
    loop {
        if cancel.is_cancelled() {
            return AssistantMessage::failed(provider, model, StopReason::Aborted, "aborted");
        }
        let failure = match attempt(on_event) {
            Ok(mut message) => {
                // A reply that broke off after its first content is not
                // retried here, but it is classified for the panel.
                if message.stop_reason == StopReason::Error && message.failure.is_none() {
                    message.failure = message.classify_failure();
                }
                return message;
            }
            Err(failure) => failure,
        };
        let backoff = policy.base_delay * 2u32.saturating_pow(n - 1);
        match inline_wait(&failure.class, backoff).filter(|_| n < max_attempts) {
            Some(delay) => {
                log::warn!(
                    "{provider} request failed (attempt {n}/{max_attempts}): {}; retrying in {delay:?}",
                    failure.message
                );
                on_event(StreamEvent::Retry {
                    attempt: n,
                    max_attempts,
                    delay_ms: delay.as_millis() as u64,
                    error: failure.message,
                });
                if !sleep_unless_cancelled(delay, cancel) {
                    return AssistantMessage::failed(
                        provider,
                        model,
                        StopReason::Aborted,
                        "aborted",
                    );
                }
                n += 1;
            }
            None => {
                return AssistantMessage::failed(
                    provider,
                    model,
                    StopReason::Error,
                    failure.message,
                )
                .with_failure(failure.class)
            }
        }
    }
}

/// Sleep in slices so an abort is noticed; `false` if cancelled.
pub fn sleep_unless_cancelled(total: Duration, cancel: &CancelToken) -> bool {
    let slice = Duration::from_millis(50);
    let mut slept = Duration::ZERO;
    while slept < total {
        if cancel.is_cancelled() {
            return false;
        }
        let step = slice.min(total - slept);
        std::thread::sleep(step);
        slept += step;
    }
    !cancel.is_cancelled()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn only_quick_failures_are_retried_inline() {
        let backoff = Duration::from_secs(1);
        let transient = Class::new(FailureKind::Transient);
        assert_eq!(inline_wait(&transient, backoff), Some(backoff));
        let soon = Class::new(FailureKind::RateLimited).with_reset(Some(now() + 20));
        let wait = inline_wait(&soon, backoff).expect("a short reset is waited for");
        assert!(wait > Duration::from_secs(15) && wait <= Duration::from_secs(20));
        let later = Class::new(FailureKind::RateLimited).with_reset(Some(now() + 3_600));
        assert_eq!(inline_wait(&later, backoff), None);
        let quota = Class::new(FailureKind::QuotaExhausted);
        assert_eq!(inline_wait(&quota, backoff), None);
        assert_eq!(inline_wait(&Class::new(FailureKind::Auth), backoff), None);
    }

    #[test]
    fn a_final_failure_carries_its_class() {
        let reply = with_retries(
            "p",
            "m",
            RetryPolicy {
                max_attempts: 3,
                base_delay: Duration::from_millis(1),
            },
            &CancelToken::new(),
            &mut |_| {},
            |_| {
                Err(Failure::http(
                    429,
                    r#"{"error":{"code":"insufficient_quota"}}"#,
                    None,
                    "quota".into(),
                ))
            },
        );
        assert_eq!(reply.stop_reason, StopReason::Error);
        assert_eq!(
            reply.failure.map(|f| f.kind),
            Some(FailureKind::QuotaExhausted)
        );
    }
}
