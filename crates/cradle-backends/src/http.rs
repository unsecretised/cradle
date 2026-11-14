//! Shared HTTP plumbing: client construction and retries before any output arrives.

use std::time::Duration;

use reqwest::{RequestBuilder, Response};
use serde_json::Value;

use crate::BackendError;

const MAX_RETRIES: u32 = 3;
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

pub(crate) fn client() -> Result<reqwest::Client, BackendError> {
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        // Long thinking pauses are normal; only give up on a stream that goes silent.
        .read_timeout(Duration::from_secs(300))
        .build()?)
}

/// Send, retrying rate limits, overloads and connection failures with backoff.
/// Retries only happen before the response starts, so no output is ever duplicated.
pub(crate) async fn send(build: impl Fn() -> RequestBuilder) -> Result<Response, BackendError> {
    let mut attempt = 0;
    loop {
        let result = build().send().await;
        let retry_after = match &result {
            Ok(resp) if resp.status().is_success() => return Ok(result?),
            Ok(resp) if BackendError::retryable_status(resp.status().as_u16()) => resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs),
            Ok(_) => return Err(api_error(result?).await),
            Err(e) if e.is_connect() || e.is_timeout() => None,
            Err(_) => return Err(result.unwrap_err().into()),
        };

        if attempt == MAX_RETRIES {
            return match result {
                Ok(resp) => Err(api_error(resp).await),
                Err(e) => Err(e.into()),
            };
        }
        let backoff = Duration::from_secs(1 << attempt);
        tokio::time::sleep(retry_after.unwrap_or(backoff).min(MAX_RETRY_DELAY)).await;
        attempt += 1;
    }
}

/// Build an error from a non-success response, using the provider's message if present.
pub(crate) async fn api_error(resp: Response) -> BackendError {
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    let message = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.chars().take(500).collect());
    BackendError::Api { status, message }
}
