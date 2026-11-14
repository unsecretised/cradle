use std::collections::HashSet;

use crate::{
    Auth, AuthMode, Backend, Capabilities, CliAgent, CliProvider, Error, Errors, HttpApi,
    HttpProvider, Kind, ProviderSpec, RawProvider,
};

/// Header names that carry credentials. Keys belong in env vars or the keychain.
const CREDENTIAL_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "api-key",
    "x-goog-api-key",
];

/// Prefixes used by common API keys and tokens.
const SECRET_PREFIXES: &[&str] = &[
    "sk-",
    "sk_",
    "pk_",
    "rk_",
    "ghp_",
    "gho_",
    "ghs_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "AIza",
    "Bearer ",
];

pub(crate) fn validate(providers: &[RawProvider]) -> Result<Vec<ProviderSpec>, Errors> {
    let mut errors = Vec::new();
    if let Err(Errors(dups)) = check_duplicate_ids(providers) {
        errors.extend(dups);
    }

    let mut specs = Vec::with_capacity(providers.len());
    for raw in providers {
        let mut ctx = Ctx {
            id: &raw.id,
            errors: &mut errors,
        };
        if let Some(spec) = validate_one(raw, &mut ctx) {
            specs.push(spec);
        }
    }

    if errors.is_empty() {
        Ok(specs)
    } else {
        Err(Errors(errors))
    }
}

pub(crate) fn check_duplicate_ids(providers: &[RawProvider]) -> Result<(), Errors> {
    let mut seen = HashSet::new();
    let errors: Vec<Error> = providers
        .iter()
        .filter(|p| !seen.insert(p.id.as_str()))
        .map(|p| Error {
            provider: Some(p.id.clone()),
            message: "declared more than once".into(),
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Errors(errors))
    }
}

struct Ctx<'a> {
    id: &'a str,
    errors: &'a mut Vec<Error>,
}

impl Ctx<'_> {
    fn error(&mut self, message: impl Into<String>) {
        self.errors.push(Error {
            provider: Some(self.id.to_owned()),
            message: message.into(),
        });
    }

    fn forbid<T>(&mut self, value: &Option<T>, field: &str, kind: Kind) {
        if value.is_some() {
            self.error(format!("`{field}` is not allowed for kind `{kind}`"));
        }
    }
}

fn validate_one(raw: &RawProvider, ctx: &mut Ctx<'_>) -> Option<ProviderSpec> {
    let before = ctx.errors.len();

    if raw.id.is_empty()
        || !raw
            .id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        ctx.error("`id` must be non-empty and use only a-z, 0-9, `-` and `_`");
    }

    let Some(kind) = raw.kind else {
        ctx.error("missing `kind` (anthropic | openai_compatible | claude_code | codex)");
        return None;
    };

    let (models, default_model) = validate_models(raw, ctx);

    let backend = if kind.is_cli() {
        validate_cli(raw, kind, models, default_model, ctx)
    } else {
        validate_http(raw, kind, models, default_model, ctx)
    };

    (ctx.errors.len() == before).then(|| ProviderSpec {
        id: raw.id.clone(),
        backend,
    })
}

fn validate_models(raw: &RawProvider, ctx: &mut Ctx<'_>) -> (Vec<String>, Option<String>) {
    let models = raw.models.clone().unwrap_or_default();
    let mut seen = HashSet::new();
    for model in &models {
        if model.trim().is_empty() {
            ctx.error("`models` contains an empty name");
        } else if !seen.insert(model.as_str()) {
            ctx.error(format!("`models` lists `{model}` more than once"));
        }
    }
    if let Some(default) = &raw.default_model
        && !models.is_empty()
        && !models.contains(default)
    {
        ctx.error(format!("`default_model` `{default}` is not in `models`"));
    }
    (models, raw.default_model.clone())
}

fn validate_http(
    raw: &RawProvider,
    kind: Kind,
    models: Vec<String>,
    default_model: Option<String>,
    ctx: &mut Ctx<'_>,
) -> Backend {
    let api = match kind {
        Kind::Anthropic => HttpApi::Anthropic,
        _ => HttpApi::OpenaiCompatible,
    };

    ctx.forbid(&raw.binary, "binary", kind);
    ctx.forbid(&raw.version, "version", kind);

    let base_url = match &raw.base_url {
        None => {
            ctx.error(format!("`base_url` is required for kind `{kind}`"));
            String::new()
        }
        Some(s) => {
            check_base_url(s, ctx);
            s.clone()
        }
    };

    let auth = match (raw.auth.unwrap_or(AuthMode::Key), &raw.api_key_env) {
        (AuthMode::None, None) => Auth::None,
        (AuthMode::None, Some(_)) => {
            ctx.error("`api_key_env` is set but `auth = \"none\"`");
            Auth::None
        }
        (AuthMode::Key, None) => {
            ctx.error("`api_key_env` is required (or set `auth = \"none\"`)");
            Auth::None
        }
        (AuthMode::Key, Some(env)) => {
            check_env_name(env, ctx);
            Auth::ApiKey { env: env.clone() }
        }
    };

    let mut headers = Vec::new();
    for (name, value) in raw.headers.iter().flatten() {
        if CREDENTIAL_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
            ctx.error(format!(
                "header `{name}` carries credentials; use `api_key_env` instead"
            ));
        } else if looks_like_secret(value) {
            ctx.error(format!(
                "header `{name}` looks like it contains a secret; secrets must not be compiled in"
            ));
        }
        headers.push((name.clone(), value.clone()));
    }

    let mut capabilities = Capabilities::defaults_for(api);
    if let Some(caps) = &raw.capabilities {
        capabilities.tools = caps.tools.unwrap_or(capabilities.tools);
        capabilities.streaming = caps.streaming.unwrap_or(capabilities.streaming);
        capabilities.prompt_caching = caps.prompt_caching.unwrap_or(capabilities.prompt_caching);
        capabilities.reasoning = caps.reasoning.unwrap_or(capabilities.reasoning);
    }
    if api == HttpApi::OpenaiCompatible && capabilities.prompt_caching {
        ctx.error("`prompt_caching` is not supported for kind `openai_compatible`");
    }

    Backend::Http(HttpProvider {
        api,
        base_url,
        auth,
        models,
        default_model,
        headers,
        capabilities,
    })
}

fn validate_cli(
    raw: &RawProvider,
    kind: Kind,
    models: Vec<String>,
    default_model: Option<String>,
    ctx: &mut Ctx<'_>,
) -> Backend {
    let agent = match kind {
        Kind::ClaudeCode => CliAgent::ClaudeCode,
        _ => CliAgent::Codex,
    };

    ctx.forbid(&raw.base_url, "base_url", kind);
    ctx.forbid(&raw.api_key_env, "api_key_env", kind);
    ctx.forbid(&raw.auth, "auth", kind);
    ctx.forbid(&raw.headers, "headers", kind);
    ctx.forbid(&raw.capabilities, "capabilities", kind);

    let binary = match &raw.binary {
        Some(b) if b.trim().is_empty() => {
            ctx.error("`binary` must not be empty");
            b.clone()
        }
        Some(b) => b.clone(),
        None => agent.default_binary().to_owned(),
    };

    if let Some(req) = &raw.version
        && let Err(e) = semver::VersionReq::parse(req)
    {
        ctx.error(format!(
            "`version` `{req}` is not a valid semver range: {e}"
        ));
    }

    Backend::Cli(CliProvider {
        agent,
        binary,
        version: raw.version.clone(),
        models,
        default_model,
    })
}

fn check_base_url(s: &str, ctx: &mut Ctx<'_>) {
    match url::Url::parse(s) {
        Err(e) => ctx.error(format!("`base_url` `{s}` is not a valid URL: {e}")),
        Ok(url) => {
            if !matches!(url.scheme(), "http" | "https") {
                ctx.error("`base_url` must use http or https");
            }
            if !url.username().is_empty() || url.password().is_some() {
                ctx.error("`base_url` must not contain credentials");
            }
            if url.query_pairs().any(|(_, v)| looks_like_secret(&v)) {
                ctx.error("`base_url` query looks like it contains a secret");
            }
        }
    }
}

fn check_env_name(env: &str, ctx: &mut Ctx<'_>) {
    let valid = env
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
        && env
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if looks_like_secret(env) {
        ctx.error("`api_key_env` looks like an actual key; put the env var *name* here");
    } else if !valid {
        ctx.error(format!(
            "`api_key_env` `{env}` must be an env var name like `ANTHROPIC_API_KEY`"
        ));
    }
}

/// Heuristic: known key prefixes, or a long unbroken token mixing letters and digits.
fn looks_like_secret(s: &str) -> bool {
    if SECRET_PREFIXES.iter().any(|p| s.starts_with(p)) {
        return true;
    }
    s.len() >= 32
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && s.chars().any(|c| c.is_ascii_digit())
        && s.chars().any(|c| c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use crate::{Auth, Backend, Kind, parse_and_validate};

    fn errors(src: &str) -> Vec<String> {
        parse_and_validate(src)
            .expect_err("expected validation errors")
            .0
            .into_iter()
            .map(|e| e.to_string())
            .collect()
    }

    fn assert_error(src: &str, needle: &str) {
        let errs = errors(src);
        assert!(
            errs.iter().any(|e| e.contains(needle)),
            "expected an error containing {needle:?}, got {errs:#?}"
        );
    }

    #[test]
    fn valid_file_with_all_kinds() {
        let specs = parse_and_validate(
            r#"
            [[provider]]
            id = "anthropic"
            kind = "anthropic"
            base_url = "https://api.anthropic.com"
            api_key_env = "ANTHROPIC_API_KEY"
            models = ["a", "b"]
            default_model = "b"

            [[provider]]
            id = "ollama"
            kind = "openai_compatible"
            base_url = "http://localhost:11434/v1"
            auth = "none"
            [provider.capabilities]
            tools = false

            [[provider]]
            id = "claude-code"
            kind = "claude_code"
            version = ">=2.0, <3.0"

            [[provider]]
            id = "codex"
            kind = "codex"
            binary = "/opt/codex"
            "#,
        )
        .unwrap();

        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].kind(), Kind::Anthropic);
        let Backend::Http(ollama) = &specs[1].backend else {
            panic!("ollama should be http")
        };
        assert_eq!(ollama.auth, Auth::None);
        assert!(!ollama.capabilities.tools);
        assert!(
            ollama.capabilities.streaming,
            "unset capabilities use kind defaults"
        );
        let Backend::Cli(claude) = &specs[2].backend else {
            panic!("claude-code should be cli")
        };
        assert_eq!(claude.binary, "claude", "binary defaults per agent");
    }

    #[test]
    fn rejects_unknown_kind_and_fields() {
        assert!(parse_and_validate("[[provider]]\nid = \"x\"\nkind = \"gemini\"").is_err());
        assert!(
            parse_and_validate("[[provider]]\nid = \"x\"\nkind = \"codex\"\napi_key = \"k\"")
                .is_err()
        );
    }

    #[test]
    fn rejects_duplicate_ids() {
        assert_error(
            "[[provider]]\nid = \"x\"\nkind = \"codex\"\n[[provider]]\nid = \"x\"\nkind = \"codex\"",
            "declared more than once",
        );
    }

    #[test]
    fn rejects_bad_urls_and_missing_auth() {
        assert_error(
            "[[provider]]\nid = \"x\"\nkind = \"anthropic\"\napi_key_env = \"K\"",
            "`base_url` is required",
        );
        assert_error(
            "[[provider]]\nid = \"x\"\nkind = \"anthropic\"\nbase_url = \"not a url\"\napi_key_env = \"K\"",
            "not a valid URL",
        );
        assert_error(
            "[[provider]]\nid = \"x\"\nkind = \"anthropic\"\nbase_url = \"https://u:p@h\"\napi_key_env = \"K\"",
            "must not contain credentials",
        );
        assert_error(
            "[[provider]]\nid = \"x\"\nkind = \"anthropic\"\nbase_url = \"https://h\"",
            "`api_key_env` is required",
        );
    }

    #[test]
    fn rejects_default_model_not_in_models() {
        assert_error(
            "[[provider]]\nid = \"x\"\nkind = \"codex\"\nmodels = [\"a\"]\ndefault_model = \"b\"",
            "is not in `models`",
        );
    }

    #[test]
    fn rejects_fields_from_other_kinds() {
        assert_error(
            "[[provider]]\nid = \"x\"\nkind = \"claude_code\"\nbase_url = \"https://h\"",
            "`base_url` is not allowed",
        );
        assert_error(
            "[[provider]]\nid = \"x\"\nkind = \"anthropic\"\nbase_url = \"https://h\"\napi_key_env = \"K\"\nbinary = \"b\"",
            "`binary` is not allowed",
        );
    }

    #[test]
    fn rejects_prompt_caching_on_openai_compatible() {
        assert_error(
            "[[provider]]\nid = \"x\"\nkind = \"openai_compatible\"\nbase_url = \"https://h\"\nauth = \"none\"\n[provider.capabilities]\nprompt_caching = true",
            "`prompt_caching` is not supported",
        );
    }

    #[test]
    fn rejects_secrets() {
        assert_error(
            "[[provider]]\nid = \"x\"\nkind = \"anthropic\"\nbase_url = \"https://h\"\napi_key_env = \"sk-ant-abc123\"",
            "looks like an actual key",
        );
        assert_error(
            "[[provider]]\nid = \"x\"\nkind = \"anthropic\"\nbase_url = \"https://h\"\napi_key_env = \"K\"\nheaders = { Authorization = \"Bearer x\" }",
            "carries credentials",
        );
        assert_error(
            "[[provider]]\nid = \"x\"\nkind = \"openai_compatible\"\nbase_url = \"https://h\"\nauth = \"none\"\nheaders = { X-Custom = \"a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6\" }",
            "looks like it contains a secret",
        );
    }

    #[test]
    fn rejects_bad_version_range() {
        assert_error(
            "[[provider]]\nid = \"x\"\nkind = \"codex\"\nversion = \"latest\"",
            "not a valid semver range",
        );
    }

    #[test]
    fn reports_all_errors_at_once() {
        let errs = errors(
            "[[provider]]\nid = \"BAD\"\nkind = \"anthropic\"\n[[provider]]\nid = \"y\"\nkind = \"codex\"\nauth = \"none\"",
        );
        assert!(errs.len() >= 3, "{errs:#?}");
    }
}
