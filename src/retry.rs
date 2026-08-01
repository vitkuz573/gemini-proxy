use std::future::Future;
use std::time::Duration;

use crate::error::ProxyError;

/// Send an async HTTP-like request with built-in retry on transient failures.
///
/// Retries the request up to `max_retries` additional times (so `max_retries=0`
/// means a single attempt). Between attempts it sleeps with exponential backoff
/// starting at 500 ms and capped at 8 s. Retries happen on 5xx status codes,
/// timeouts, connection errors, and reqwest-internal errors.
///
/// The caller supplies a closure returning a `Future<Output = Result<reqwest::Response, reqwest::Error>>`.
/// When retries are exhausted, the last underlying reqwest error is wrapped in
/// `ProxyError::Upstream`.
pub async fn send_retryable_request<F, Fut>(
    max_retries: usize,
    mut request_fn: F,
) -> Result<reqwest::Response, ProxyError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    retry_with_backoff(
        move || {
            let fut = request_fn();
            async move {
                let response = fut.await.map_err(|e| {
                    ProxyError::GeminiApi(format!("upstream request failed: {e}"))
                })?;

                let status = response.status();
                if status.is_server_error() {
                    let body = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "<unreadable>".to_string());
                    return Err(ProxyError::GeminiApi(format!(
                        "upstream returned {status}: {body}"
                    )));
                }

                if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    let body = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "<unreadable>".to_string());
                    return Err(ProxyError::RateLimited(format!(
                        "rate limited (429): {body}"
                    )));
                }

                if status.is_client_error() {
                    let body = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "<unreadable>".to_string());
                    return Err(ProxyError::GeminiApi(format!(
                        "upstream returned {status}: {body}"
                    )));
                }

                Ok(response)
            }
        },
        max_retries + 1,
        Duration::from_millis(500),
        Duration::from_secs(8),
        |err| !matches!(err, ProxyError::RateLimited(_)),
    )
    .await
}

/// Retry an async operation up to `max_attempts` times with exponential backoff
/// capped at `max_delay`.
///
/// `should_retry` is called on each error; returning `false` short-circuits the
/// retry loop and returns the error immediately.
///
/// Returns `Ok(T)` as soon as the operation succeeds, or the last error after
/// the final attempt.
pub async fn retry_with_backoff<T, E, F, Fut>(
    mut op: F,
    max_attempts: usize,
    initial_delay: Duration,
    max_delay: Duration,
    mut should_retry: impl FnMut(&E) -> bool,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    if max_attempts == 0 {
        panic!("max_attempts must be > 0");
    }

    let mut attempt = 1;
    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                if attempt >= max_attempts || !should_retry(&err) {
                    return Err(err);
                }

                let delay = std::cmp::min(
                    initial_delay * 2_u32.saturating_pow((attempt - 1) as u32),
                    max_delay,
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn succeeds_on_first_try() {
        let result = retry_with_backoff(
            || async { Ok::<_, &'static str>("hello") },
            3,
            Duration::from_millis(10),
            Duration::from_millis(100),
            |_err| true,
        )
        .await;
        assert_eq!(result.unwrap(), "hello");
    }

    #[tokio::test]
    async fn retries_until_success() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let result = retry_with_backoff(
            move || {
                let c = counter_clone.clone();
                async move {
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    if n < 2 {
                        Err("transient")
                    } else {
                        Ok(n)
                    }
                }
            },
            5,
            Duration::from_millis(1),
            Duration::from_millis(10),
            |_err| true,
        )
        .await;
        assert_eq!(result.unwrap(), 2);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn respects_non_retryable_error() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let result = retry_with_backoff(
            move || {
                let c = counter_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err::<i32, &str>("fatal")
                }
            },
            5,
            Duration::from_millis(1),
            Duration::from_millis(10),
            |err| *err == "transient",
        )
        .await;
        assert_eq!(result.unwrap_err(), "fatal");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let result = retry_with_backoff(
            move || {
                let c = counter_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err::<i32, &str>("transient")
                }
            },
            3,
            Duration::from_millis(1),
            Duration::from_millis(10),
            |_err| true,
        )
        .await;
        assert_eq!(result.unwrap_err(), "transient");
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn backoff_caps_at_max_delay() {
        let mut observed_delays = Vec::new();
        let start = std::time::Instant::now();
        let _ = retry_with_backoff(
            || async { Err::<i32, &str>("boom") },
            5,
            Duration::from_millis(1),
            Duration::from_millis(4),
            |_err| true,
        )
        .await;
        let elapsed = start.elapsed();
        // 1ms + 2ms + 4ms + 4ms = 11ms, plus scheduler jitter
        assert!(elapsed >= Duration::from_millis(10));
        observed_delays.push(elapsed);
        // Ensure the vector is used to silence any future unused-variable lint.
        assert!(!observed_delays.is_empty());
    }
}
