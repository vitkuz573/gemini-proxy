use std::future::Future;
use std::time::Duration;

use reqwest::{Response, StatusCode};
use tracing::warn;

use crate::error::{ProxyError, Result};

/// Retry an HTTP request on 429 and 5xx status codes with exponential backoff.
///
/// - `max_retries` is the number of *extra* attempts after the first request.
/// - Base backoff is 500 ms, doubling each attempt, capped at 8 s.
/// - If the response carries a `Retry-After` header with a positive integer
///   (seconds), that value is used instead of the exponential delay.
/// - If every attempt fails with a retryable status, [`ProxyError::RateLimited`]
///   is returned and surfaced as HTTP 429 to the caller.
pub async fn send_retryable_request<F, Fut>(max_retries: u32, build_request: F) -> Result<Response>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = reqwest::Result<Response>> + Send,
{
    let mut last_error: Option<String> = None;

    for attempt in 0..=max_retries {
        let request = build_request();
        let response = request.await.map_err(ProxyError::Reqwest)?;
        let status = response.status();

        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|h| h.to_str().ok().and_then(|s| s.parse::<u64>().ok()));

        let body = response.text().await.unwrap_or_default();
        last_error = Some(format!("HTTP {status}: {body}"));

        if status.is_success() || !is_retryable_status(status) {
            return Ok(Response::from(
                http::Response::builder()
                    .status(status)
                    .body(body)
                    .unwrap(),
            ));
        }

        if attempt == max_retries {
            break;
        }

        let delay = retry_after
            .filter(|s| *s > 0)
            .map(Duration::from_secs)
            .unwrap_or_else(|| exponential_backoff(attempt));

        warn!(
            status = %status,
            attempt = attempt + 1,
            max_attempts = max_retries + 1,
            ?delay,
            body = %body,
            "Upstream request rate-limited or errored; retrying"
        );

        tokio::time::sleep(delay).await;
    }

    Err(ProxyError::RateLimited(
        last_error.unwrap_or_else(|| "Upstream rate limit or server error persisted".into()),
    ))
}

fn is_retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}


fn exponential_backoff(attempt: u32) -> Duration {
    let base_ms: u64 = 500;
    let capped_attempt = attempt.min(4); // 500, 1000, 2000, 4000, 8000
    let delay_ms = base_ms * 2u64.pow(capped_attempt);
    Duration::from_millis(delay_ms.min(8000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_statuses() {
        assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(is_retryable_status(StatusCode::BAD_GATEWAY));
        assert!(is_retryable_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(!is_retryable_status(StatusCode::OK));
        assert!(!is_retryable_status(StatusCode::BAD_REQUEST));
        assert!(!is_retryable_status(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn exponential_backoff_values() {
        assert_eq!(exponential_backoff(0), Duration::from_millis(500));
        assert_eq!(exponential_backoff(1), Duration::from_millis(1000));
        assert_eq!(exponential_backoff(2), Duration::from_millis(2000));
        assert_eq!(exponential_backoff(3), Duration::from_millis(4000));
        assert_eq!(exponential_backoff(4), Duration::from_millis(8000));
        assert_eq!(exponential_backoff(10), Duration::from_millis(8000));
    }

    #[tokio::test]
    async fn succeeds_on_first_success() {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let result = send_retryable_request(2, move || {
            let attempts = attempts_clone.clone();
            async move {
                attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok::<_, reqwest::Error>(
                    http::Response::builder()
                        .status(200)
                        .body("")
                        .unwrap()
                        .into(),
                )
            }
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let result = send_retryable_request(3, move || {
            let attempts = attempts_clone.clone();
            async move {
                let count = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if count < 3 {
                    Ok::<_, reqwest::Error>(
                        http::Response::builder()
                            .status(503)
                            .body("temporarily unavailable")
                            .unwrap()
                            .into(),
                    )
                } else {
                    Ok::<_, reqwest::Error>(
                        http::Response::builder()
                            .status(200)
                            .body("ok")
                            .unwrap()
                            .into(),
                    )
                }
            }
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn rate_limited_after_exhaustion() {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let result = send_retryable_request(1, move || {
            let attempts = attempts_clone.clone();
            async move {
                attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok::<_, reqwest::Error>(
                    http::Response::builder()
                        .status(429)
                        .header("Retry-After", "1")
                        .body("slow down")
                        .unwrap()
                        .into(),
                )
            }
        })
        .await;
        match result {
            Err(ProxyError::RateLimited(msg)) => {
                assert!(msg.contains("429"));
                assert!(msg.contains("slow down"));
            }
            other => panic!("expected RateLimited error, got {other:?}"),
        }
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
