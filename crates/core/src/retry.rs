// SPDX-License-Identifier: MIT

//! Exponential-backoff retry (port of `retry_utils.py`).
//!
//! - initial 1s, ×2 each attempt, capped at 10min, max total 24h.
//! - which responses to retry is the caller's validator (upstream: only 5xx; the 17Lands
//!   transport also retries rate limiting).
//! - retry on transport/connection errors only; re-raise others.
//! - ⚠️ `ureq` maps non-2xx to `Err(Status)` - the caller (`services::seventeenlands::transport`) normalizes those
//!   back into `Ok(Response)` so HTTP responses (incl. 4xx/5xx) reach the
//!   `response_validator`; only genuine transport errors arrive here as `Err`.

#![allow(dead_code)]

use std::time::{Duration, Instant};

pub const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(1);
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(10 * 60);
pub const MAX_TOTAL_RETRY_DURATION: Duration = Duration::from_secs(24 * 60 * 60);

/// Terminal outcome of an exhausted retry loop (port of the two raise sites in
/// `retry_until_successful`).
#[derive(Debug)]
pub enum RetryError<E> {
    /// Response stayed invalid until the total-duration deadline (`RetryLimitExceededError`).
    LimitExceeded,
    /// The caller asked to stop (not in upstream): nothing more was attempted.
    Cancelled,
    /// A non-retryable error, or a retryable one that survived to the deadline. Python
    /// re-raises the original exception; we carry it out unchanged.
    Fatal(E),
}

/// Retry `callback` until `response_validator` accepts its result, with exponential
/// backoff (port of `retry_utils.retry_until_successful`).
///
/// `callback` returns `Ok(T)` for any completed call (including non-2xx HTTP responses,
/// which the caller normalizes) and `Err(E)` only for transport-level failures.
/// `error_validator` decides whether such an `Err(E)` is retryable.
pub fn retry_until_successful<T, E, F, V, G>(
    callback: F,
    response_validator: V,
    error_validator: G,
    initial_retry_delay: Duration,
    max_retry_delay: Option<Duration>,
    max_total_retry_duration: Option<Duration>,
) -> Result<T, RetryError<E>>
where
    F: FnMut() -> Result<T, E>,
    V: Fn(&T) -> bool,
    G: Fn(&E) -> bool,
{
    retry_until_cancelled(
        callback,
        response_validator,
        error_validator,
        || false,
        initial_retry_delay,
        max_retry_delay,
        max_total_retry_duration,
    )
}

/// How often a backoff sleep looks at the cancel flag.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// [`retry_until_successful`] with a way out: `cancelled` is consulted before every attempt
/// and every [`CANCEL_POLL`] during the backoff sleep, and ends the loop with
/// [`RetryError::Cancelled`]. Without it a caller waiting on this thread (the desktop app
/// joining its follower) would hang for a whole backoff delay - up to ten minutes.
pub fn retry_until_cancelled<T, E, F, V, G, C>(
    mut callback: F,
    response_validator: V,
    error_validator: G,
    cancelled: C,
    initial_retry_delay: Duration,
    max_retry_delay: Option<Duration>,
    max_total_retry_duration: Option<Duration>,
) -> Result<T, RetryError<E>>
where
    F: FnMut() -> Result<T, E>,
    V: Fn(&T) -> bool,
    G: Fn(&E) -> bool,
    C: Fn() -> bool,
{
    // Python: last_call_at = utcnow() + max_total_retry_duration (when set).
    let deadline = max_total_retry_duration.map(|d| Instant::now() + d);
    let mut next_retry_delay = initial_retry_delay;

    loop {
        if cancelled() {
            return Err(RetryError::Cancelled);
        }
        // is_last_call flips true only once the total deadline has passed.
        let is_last_call = deadline.is_some_and(|dl| dl < Instant::now());

        match callback() {
            Ok(result) => {
                if response_validator(&result) {
                    return Ok(result);
                } else if is_last_call {
                    return Err(RetryError::LimitExceeded);
                }
            }
            Err(e) => {
                if is_last_call || !error_validator(&e) {
                    return Err(RetryError::Fatal(e));
                }
            }
        }

        let wake_at = Instant::now() + next_retry_delay;
        loop {
            if cancelled() {
                return Err(RetryError::Cancelled);
            }
            let left = wake_at.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(CANCEL_POLL));
        }
        next_retry_delay = next_retry_delay.saturating_mul(2);
        if let Some(max) = max_retry_delay
            && max < next_retry_delay
        {
            next_retry_delay = max;
        }
    }
}

/// Retry an API call with the standard delays (port of `retry_api_call`).
///
/// All errors reaching this layer are transport errors (the caller has already mapped
/// non-2xx HTTP statuses to `Ok`), so every `Err` is retryable - mirroring Python's
/// `_should_retry_error` returning `True` for `ConnectionError`.
pub fn retry_api_call<T, E, F, V, C>(
    callback: F,
    response_validator: V,
    cancelled: C,
) -> Result<T, RetryError<E>>
where
    F: FnMut() -> Result<T, E>,
    V: Fn(&T) -> bool,
    C: Fn() -> bool,
{
    retry_until_cancelled(
        callback,
        response_validator,
        |_e| true,
        cancelled,
        INITIAL_RETRY_DELAY,
        Some(MAX_RETRY_DELAY),
        Some(MAX_TOTAL_RETRY_DURATION),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const FAST: Duration = Duration::from_millis(0);

    #[test]
    fn returns_immediately_on_valid_response() {
        let calls = Cell::new(0);
        let r: Result<i32, RetryError<()>> = retry_until_successful(
            || {
                calls.set(calls.get() + 1);
                Ok(200)
            },
            |&s| !(500..600).contains(&s),
            |_| true,
            FAST,
            Some(FAST),
            Some(Duration::from_secs(10)),
        );
        assert!(matches!(r, Ok(200)));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn does_not_retry_4xx() {
        // 404 is "valid" (status < 500) - returned as-is, no retry.
        let calls = Cell::new(0);
        let r: Result<u16, RetryError<()>> = retry_until_successful(
            || {
                calls.set(calls.get() + 1);
                Ok(404)
            },
            |&s| !(500..600).contains(&s),
            |_| true,
            FAST,
            Some(FAST),
            Some(Duration::from_secs(10)),
        );
        assert!(matches!(r, Ok(404)));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn retries_5xx_until_success() {
        let attempt = Cell::new(0u16);
        let r: Result<u16, RetryError<()>> = retry_until_successful(
            || {
                let n = attempt.get();
                attempt.set(n + 1);
                Ok(if n < 2 { 503 } else { 200 })
            },
            |&s| !(500..600).contains(&s),
            |_| true,
            FAST,
            Some(FAST),
            Some(Duration::from_secs(10)),
        );
        assert!(matches!(r, Ok(200)));
        assert_eq!(attempt.get(), 3); // 503, 503, 200
    }

    #[test]
    fn retries_retryable_transport_error_then_succeeds() {
        let attempt = Cell::new(0);
        let r: Result<u16, RetryError<&str>> = retry_until_successful(
            || {
                let n = attempt.get();
                attempt.set(n + 1);
                if n == 0 {
                    Err("connection reset")
                } else {
                    Ok(200)
                }
            },
            |&s| !(500..600).contains(&s),
            |_| true, // retryable
            FAST,
            Some(FAST),
            Some(Duration::from_secs(10)),
        );
        assert!(matches!(r, Ok(200)));
        assert_eq!(attempt.get(), 2);
    }

    #[test]
    fn non_retryable_error_is_fatal_immediately() {
        let calls = Cell::new(0);
        let r: Result<u16, RetryError<&str>> = retry_until_successful(
            || {
                calls.set(calls.get() + 1);
                Err("fatal")
            },
            |&s| !(500..600).contains(&s),
            |_| false, // not retryable
            FAST,
            Some(FAST),
            Some(Duration::from_secs(10)),
        );
        assert!(matches!(r, Err(RetryError::Fatal("fatal"))));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn limit_exceeded_when_response_stays_invalid_past_deadline() {
        // Zero total duration → first iteration is already the last call.
        let r: Result<u16, RetryError<()>> = retry_until_successful(
            || Ok(503),
            |&s| !(500..600).contains(&s),
            |_| true,
            FAST,
            Some(FAST),
            Some(Duration::from_secs(0)),
        );
        assert!(matches!(r, Err(RetryError::LimitExceeded)));
    }

    #[test]
    fn cancelling_cuts_the_backoff_sleep_short() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let cancel = Arc::new(AtomicBool::new(false));
        let setter = cancel.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            setter.store(true, Ordering::Relaxed);
        });
        let calls = Cell::new(0);
        let started = Instant::now();
        let r: Result<u16, RetryError<()>> = retry_until_cancelled(
            || {
                calls.set(calls.get() + 1);
                Ok(503)
            },
            |&s| !(500..600).contains(&s),
            |_| true,
            || cancel.load(Ordering::Relaxed),
            Duration::from_secs(600),
            None,
            None,
        );
        t.join().unwrap();
        assert!(matches!(r, Err(RetryError::Cancelled)));
        assert_eq!(calls.get(), 1);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn an_already_cancelled_call_attempts_nothing() {
        let calls = Cell::new(0);
        let r: Result<u16, RetryError<()>> = retry_until_cancelled(
            || {
                calls.set(calls.get() + 1);
                Ok(200)
            },
            |_| true,
            |_| true,
            || true,
            FAST,
            None,
            None,
        );
        assert!(matches!(r, Err(RetryError::Cancelled)));
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn transport_error_past_deadline_is_fatal() {
        let r: Result<u16, RetryError<&str>> = retry_until_successful(
            || Err("boom"),
            |&s| !(500..600).contains(&s),
            |_| true, // retryable, but deadline already passed
            FAST,
            Some(FAST),
            Some(Duration::from_secs(0)),
        );
        assert!(matches!(r, Err(RetryError::Fatal("boom"))));
    }
}
