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
    mut callback: F,
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
    // Python: last_call_at = utcnow() + max_total_retry_duration (when set).
    let deadline = max_total_retry_duration.map(|d| Instant::now() + d);
    let mut next_retry_delay = initial_retry_delay;

    loop {
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

        std::thread::sleep(next_retry_delay);
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
pub fn retry_api_call<T, E, F, V>(callback: F, response_validator: V) -> Result<T, RetryError<E>>
where
    F: FnMut() -> Result<T, E>,
    V: Fn(&T) -> bool,
{
    retry_until_successful(
        callback,
        response_validator,
        |_e| true,
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
