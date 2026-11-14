//! Which models a provider offers. HTTP providers are asked live (Anthropic's Models API,
//! `GET /models` on OpenAI-compatible servers); the configured list comes first and is the
//! fallback. CLI providers use the configured list (aliases such as `opus`).

use std::time::Duration;

use cradle_provider_spec::{Auth, Backend as SpecBackend, HttpApi, ProviderSpec};
use serde_json::Value;

use crate::{BackendError, credentials, http};

const TIMEOUT: Duration = Duration::from_secs(8);

/// Models for `spec`, configured ones first. Live lookup failures are returned so the
/// caller can show them, together with the configured list.
pub async fn available_models(spec: &ProviderSpec) -> (Vec<String>, Option<BackendError>) {
    let (configured, http) = match &spec.backend {
        SpecBackend::Cli(cli) => return (cli.models.clone(), None),
        SpecBackend::Http(http) => (http.models.clone(), http),
    };
    let key = match &http.auth {
        Auth::None => None,
        Auth::ApiKey { env } => match credentials::find_key(env) {
            Some((key, _)) => Some(key),
            None => return (configured, Some(BackendError::MissingKey(env.clone()))),
        },
    };
    let live = tokio::time::timeout(TIMEOUT, fetch(http.api, &http.base_url, key, &http.headers))
        .await
        .unwrap_or_else(|_| Err(BackendError::Protocol("listing models timed out".into())));
    match live {
        Ok(live) => (merge(configured, live), None),
        Err(e) => (configured, Some(e)),
    }
}

async fn fetch(
    api: HttpApi,
    base_url: &str,
    key: Option<String>,
    headers: &[(String, String)],
) -> Result<Vec<String>, BackendError> {
    let client = http::client()?;
    let base = base_url.trim_end_matches('/');
    // One attempt: a picker should not wait through retries for a server that is down.
    let send = || {
        let mut builder = match api {
            HttpApi::Anthropic => {
                let mut b = client
                    .get(format!("{base}/v1/models?limit=100"))
                    .header("anthropic-version", crate::anthropic::API_VERSION);
                if let Some(key) = &key {
                    b = b.header("x-api-key", key);
                }
                b
            }
            HttpApi::OpenaiCompatible => {
                let mut b = client.get(format!("{base}/models"));
                if let Some(key) = &key {
                    b = b.bearer_auth(key);
                }
                b
            }
        };
        for (name, value) in headers {
            builder = builder.header(name, value);
        }
        builder
    };
    let resp = send().send().await?;
    if !resp.status().is_success() {
        return Err(http::api_error(resp).await);
    }
    let body: Value = resp.json().await?;
    Ok(parse_ids(&body))
}

/// `{"data": [{"id": "..."}, ...]}`, the shape both APIs use.
fn parse_ids(body: &Value) -> Vec<String> {
    body.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

fn merge(mut configured: Vec<String>, live: Vec<String>) -> Vec<String> {
    for id in live {
        if !configured.contains(&id) {
            configured.push(id);
        }
    }
    configured
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_and_merges_ids() {
        let ids = parse_ids(&json!({"data": [{"id": "b"}, {"id": "a"}, {"name": "x"}]}));
        assert_eq!(ids, ["b", "a"]);
        assert_eq!(merge(vec!["a".into()], ids), ["a", "b"]);
    }
}
