//! Insight settings are read exclusively through the shared prefixed lookup.
use crate::config::{ConfigError, invalid};
use std::{path::PathBuf, time::Duration};

pub struct InsightConfig {
    pub service: yoink_insight::Config,
    pub executor: yoink_executor::ExecutorConfig,
    pub content_timeout: Duration,
    pub text_timeout: Duration,
    pub research_timeout: Duration,
    pub response_bytes: usize,
    pub content_proxy: Option<String>,
    pub read_client: String,
    pub write_client: Option<String>,
    pub oauth: yoink_insight::oauth::OAuthOptions,
}
impl std::fmt::Debug for InsightConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InsightRuntimeConfig([redacted])")
    }
}
fn number(
    read: &mut impl FnMut(&str) -> Result<Option<String>, ConfigError>,
    key: &str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, ConfigError> {
    let value = read(key)?
        .map(|v| v.parse::<u64>())
        .transpose()
        .map_err(|_| invalid(key, "expected bounded integer"))?
        .unwrap_or(default);
    if !(min..=max).contains(&value) {
        return Err(invalid(key, "outside supported bounds"));
    }
    Ok(value)
}
pub fn completion_config(
    read: &mut impl FnMut(&str) -> Result<Option<String>, ConfigError>,
    mut service: yoink_insight::Config,
) -> Result<yoink_insight::Config, ConfigError> {
    for (key, field) in [
        ("INSIGHT_GATEWAY_BASE", &mut service.gateway_base),
        ("INSIGHT_GATEWAY_KEY", &mut service.gateway_key),
        ("INSIGHT_OPENROUTER_BASE", &mut service.openrouter_base),
        ("INSIGHT_OPENROUTER_KEY", &mut service.openrouter_key),
        ("INSIGHT_MODE", &mut service.mode),
        ("INSIGHT_MODEL", &mut service.model),
        ("INSIGHT_GEMINI_KEY", &mut service.gemini_key),
        ("INSIGHT_GEMINI_MODEL", &mut service.gemini_model),
        ("INSIGHT_SEARCH_MODEL", &mut service.search_model),
        ("INSIGHT_DEFAULT_LANG", &mut service.default_lang),
    ] {
        if let Some(v) = read(key)? {
            if v.len() > 8192 {
                return Err(invalid(key, "value too long"));
            }
            *field = v;
        }
    }
    if !matches!(service.mode.as_str(), "auto" | "a2a" | "openrouter") {
        return Err(invalid("INSIGHT_MODE", "expected auto, a2a or openrouter"));
    }
    let proxy = read("INSIGHT_AI_PROXY")?
        .or(read("INSIGHT_OPENROUTER_PROXY")?)
        .ok_or_else(|| {
            invalid(
                "INSIGHT_AI_PROXY",
                "requires an explicit SOCKS5 tunnel; direct AI egress is disabled",
            )
        })?;
    let parsed = url::Url::parse(&proxy)
        .map_err(|_| invalid("INSIGHT_AI_PROXY", "expected a SOCKS5 URL with local DNS"))?;
    if proxy.len() > 2048
        || parsed.scheme() != "socks5"
        || parsed.host_str().is_none()
        || !matches!(parsed.path(), "" | "/")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(invalid(
            "INSIGHT_AI_PROXY",
            "expected a SOCKS5 URL with local DNS; HTTP CONNECT and socks5h cannot preserve BYOK DNS pins",
        ));
    }
    service.ai_proxy = Some(proxy);
    if !matches!(
        service.default_lang.as_str(),
        "en" | "ru" | "uk" | "de" | "fr" | "es" | "zh" | "ja"
    ) {
        return Err(invalid("INSIGHT_DEFAULT_LANG", "unsupported language"));
    }
    for (key, base) in [
        ("INSIGHT_GATEWAY_BASE", &service.gateway_base),
        ("INSIGHT_OPENROUTER_BASE", &service.openrouter_base),
    ] {
        if !base.is_empty() {
            endpoint(key, base, false)?;
        }
    }
    if service.openrouter_base.trim_end_matches('/') == "https://openrouter.ai" {
        service.openrouter_base = "https://openrouter.ai/api/v1".into();
    }
    service.summary_limit = number(read, "INSIGHT_SUMMARY_DAILY_LIMIT", 50, 0, 1000000)? as i64;
    service.tldr_limit = number(read, "INSIGHT_TLDR_DAILY_LIMIT", 20, 0, 1000000)? as i64;
    if let Some(v) = read("INSIGHT_TRANSCRIPT_LANGUAGES")? {
        service.transcript_languages = serde_json::from_str(&v).map_err(|_| {
            invalid(
                "INSIGHT_TRANSCRIPT_LANGUAGES",
                "expected JSON language array",
            )
        })?;
        if service.transcript_languages.is_empty()
            || service.transcript_languages.len() > 32
            || service.transcript_languages.iter().any(|s| {
                s.is_empty()
                    || s.len() > 32
                    || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            })
        {
            return Err(invalid(
                "INSIGHT_TRANSCRIPT_LANGUAGES",
                "invalid language array",
            ));
        }
    }
    Ok(service)
}
fn endpoint(key: &str, value: &str, oauth: bool) -> Result<url::Url, ConfigError> {
    let u = url::Url::parse(value).map_err(|_| invalid(key, "invalid trusted HTTP(S) endpoint"))?;
    let local = u.host_str().is_some_and(|h| {
        h == "localhost"
            || h.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if value.len() > 2048
        || !matches!(u.scheme(), "http" | "https")
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || (oauth && u.scheme() == "http" && !local)
    {
        return Err(invalid(key, "invalid trusted HTTP(S) endpoint"));
    }
    Ok(u)
}
impl InsightConfig {
    pub fn from_lookup(
        read: &mut impl FnMut(&str) -> Result<Option<String>, ConfigError>,
        shutdown: Duration,
        db: bool,
    ) -> Result<Option<Self>, ConfigError> {
        match read("INSIGHT_ENABLED")?.as_deref().unwrap_or("false") {
            "false" => return Ok(None),
            "true" => (),
            _ => return Err(invalid("INSIGHT_ENABLED", "expected true or false")),
        }
        if !db {
            return Err(invalid("INSIGHT_ENABLED", "requires explicit database"));
        }
        let secs = |read: &mut _, key, default| {
            number(read, key, default, 1, 300).map(Duration::from_secs)
        };
        let deadline = secs(read, "INSIGHT_REQUEST_TIMEOUT_SECS", 120)?;
        let content_timeout = secs(read, "INSIGHT_CONTENT_TIMEOUT_SECS", 60)?;
        let text_timeout =
            Duration::from_secs(number(read, "INSIGHT_TEXT_TIMEOUT_SECS", 15, 1, 60)?);
        let research_timeout = secs(read, "INSIGHT_RESEARCH_TIMEOUT_SECS", 90)?;
        let socket = secs(read, "INSIGHT_SOCKET_TIMEOUT_SECS", 15)?;
        let oauth_request = secs(read, "INSIGHT_OAUTH_REQUEST_TIMEOUT_SECS", 15)?;
        let username = secs(read, "INSIGHT_OAUTH_USERNAME_TIMEOUT_SECS", 10)?;
        if content_timeout > deadline
            || text_timeout > deadline
            || research_timeout > deadline
            || socket > content_timeout
            || username > oauth_request
            || deadline >= shutdown
            || oauth_request + username >= shutdown
        {
            return Err(invalid(
                "INSIGHT_REQUEST_TIMEOUT_SECS",
                "adapter budgets must fit request and shutdown deadlines",
            ));
        }
        let mut service = completion_config(read, yoink_insight::Config::default())?;
        service.request_timeout = deadline;
        service.max_chars = number(read, "INSIGHT_CONTENT_MAX_CHARS", 40000, 100, 200000)? as usize;
        let workspace_root = PathBuf::from(
            read("INSIGHT_WORKSPACE")?.unwrap_or_else(|| "/tmp/yoink-insight".into()),
        );
        if !workspace_root.is_absolute()
            || workspace_root.parent().is_none()
            || workspace_root
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(invalid(
                "INSIGHT_WORKSPACE",
                "requires dedicated absolute directory",
            ));
        }
        let response_bytes =
            number(read, "INSIGHT_RESPONSE_MAX_BYTES", 1048576, 1024, 4194304)? as usize;
        let content_proxy = read("INSIGHT_CONTENT_PROXY")?;
        let mut allowed_proxy_origins = vec![];
        if let Some(p) = &content_proxy {
            let key = "INSIGHT_CONTENT_PROXY";
            let u =
                url::Url::parse(p).map_err(|_| invalid(key, "invalid operator HTTP(S) proxy"))?;
            if p.len() > 2048
                || !matches!(u.scheme(), "http" | "https")
                || u.host_str().is_none()
                || u.path() != "/"
                || u.query().is_some()
                || u.fragment().is_some()
            {
                return Err(invalid(key, "invalid operator HTTP(S) proxy"));
            }
            allowed_proxy_origins.push(u.origin().ascii_serialization());
        }
        let device_base_url =
            read("INSIGHT_OAUTH_DEVICE_BASE")?.unwrap_or_else(|| "https://github.com".into());
        let user_url =
            read("INSIGHT_OAUTH_USER_URL")?.unwrap_or_else(|| "https://api.github.com/user".into());
        endpoint("INSIGHT_OAUTH_DEVICE_BASE", &device_base_url, true)?;
        endpoint("INSIGHT_OAUTH_USER_URL", &user_url, true)?;
        let read_client =
            read("INSIGHT_GITHUB_CLIENT_ID")?.unwrap_or_else(|| "Iv1.b507a08c87ecfe98".into());
        let write_client = read("INSIGHT_GITHUB_PUBLIC_REPO_CLIENT_ID")?;
        if read_client.is_empty()
            || read_client.len() > 256
            || write_client
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 256)
        {
            return Err(invalid("INSIGHT_GITHUB_CLIENT_ID", "invalid client ID"));
        }
        Ok(Some(Self {
            service,
            executor: yoink_executor::ExecutorConfig {
                program: "/usr/local/bin/yoink-worker-launcher".into(),
                args: vec![],
                workspace_root,
                timeout: content_timeout,
                processing_timeout: content_timeout,
                socket_timeout: socket,
                max_artifact_bytes: response_bytes as u64,
                max_workspace_bytes: (response_bytes * 2) as u64,
                max_output_bytes: response_bytes.min(4 * 1024 * 1024),
                max_artifacts: 1,
                allowed_proxy_origins,
                allowed_private_origins: vec![],
            },
            content_timeout,
            text_timeout,
            research_timeout,
            response_bytes,
            content_proxy,
            read_client,
            write_client,
            oauth: yoink_insight::oauth::OAuthOptions {
                request_timeout: oauth_request,
                username_timeout: username,
                device_base_url,
                user_url,
            },
        }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn load(
        entries: &[(&str, &str)],
        shutdown: u64,
        db: bool,
    ) -> Result<Option<InsightConfig>, ConfigError> {
        InsightConfig::from_lookup(
            &mut |k| {
                Ok(entries
                    .iter()
                    .find(|(key, _)| *key == k)
                    .map(|(_, v)| v.to_string())
                    .or_else(|| (k == "INSIGHT_AI_PROXY").then(|| "socks5://127.0.0.1:9".into())))
            },
            Duration::from_secs(shutdown),
            db,
        )
    }
    #[test]
    fn official_openrouter_origin_uses_its_api_prefix() {
        let config = load(
            &[
                ("INSIGHT_ENABLED", "true"),
                ("INSIGHT_OPENROUTER_BASE", "https://openrouter.ai/"),
            ],
            150,
            true,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            config.service.openrouter_base,
            "https://openrouter.ai/api/v1"
        );
        let custom = load(
            &[
                ("INSIGHT_ENABLED", "true"),
                ("INSIGHT_OPENROUTER_BASE", "https://custom.example/v1"),
            ],
            150,
            true,
        )
        .unwrap()
        .unwrap();
        assert_eq!(custom.service.openrouter_base, "https://custom.example/v1");
    }

    #[test]
    fn enabled_ai_requires_a_locally_resolving_tunnel() {
        let missing = InsightConfig::from_lookup(
            &mut |key| Ok((key == "INSIGHT_ENABLED").then(|| "true".into())),
            Duration::from_secs(150),
            true,
        )
        .unwrap_err();
        assert!(missing.to_string().contains("YOINK_RUST_INSIGHT_AI_PROXY"));
        for proxy in [
            "http://127.0.0.1:8080",
            "socks5h://127.0.0.1:1080",
            "socks5://127.0.0.1:1080/path",
        ] {
            assert!(
                load(
                    &[("INSIGHT_ENABLED", "true"), ("INSIGHT_AI_PROXY", proxy)],
                    150,
                    true
                )
                .is_err()
            );
        }
        let legacy = InsightConfig::from_lookup(
            &mut |key| {
                Ok(match key {
                    "INSIGHT_ENABLED" => Some("true".into()),
                    "INSIGHT_OPENROUTER_PROXY" => Some("socks5://127.0.0.1:1080".into()),
                    _ => None,
                })
            },
            Duration::from_secs(150),
            true,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            legacy.service.ai_proxy.as_deref(),
            Some("socks5://127.0.0.1:1080")
        );
    }

    #[test]
    fn independent_and_bounded() {
        assert!(load(&[], 10, false).unwrap().is_none());
        assert!(
            load(&[("INSIGHT_ENABLED", "true")], 150, true)
                .unwrap()
                .is_some()
        );
        assert!(load(&[("INSIGHT_ENABLED", "true")], 150, false).is_err());
        assert!(load(&[("INSIGHT_ENABLED", "true")], 120, true).is_err());
        for (k, v) in [
            ("INSIGHT_REQUEST_TIMEOUT_SECS", "0"),
            ("INSIGHT_TEXT_TIMEOUT_SECS", "0"),
            ("INSIGHT_TEXT_TIMEOUT_SECS", "61"),
            ("INSIGHT_DEFAULT_LANG", "it"),
            ("INSIGHT_MODE", "bad"),
            ("INSIGHT_RESPONSE_MAX_BYTES", "4194305"),
            ("INSIGHT_WORKSPACE", "/"),
            ("INSIGHT_OAUTH_USER_URL", "http://example.com/user"),
        ] {
            assert!(
                load(&[("INSIGHT_ENABLED", "true"), (k, v)], 150, true).is_err(),
                "{k}"
            );
        }
        for lang in ["en", "ru", "uk", "de", "fr", "es", "zh", "ja"] {
            assert!(
                load(
                    &[("INSIGHT_ENABLED", "true"), ("INSIGHT_DEFAULT_LANG", lang)],
                    150,
                    true
                )
                .is_ok()
            );
        }
        let maximum = load(
            &[
                ("INSIGHT_ENABLED", "true"),
                ("INSIGHT_RESPONSE_MAX_BYTES", "4194304"),
            ],
            150,
            true,
        )
        .unwrap()
        .unwrap();
        assert!(
            yoink_insight::provider::HttpCompletion::new(
                maximum.service.request_timeout,
                maximum.response_bytes
            )
            .is_ok()
        );
        assert!(
            yoink_insight::research::ResponsesResearch::new(
                maximum.research_timeout,
                maximum.response_bytes
            )
            .is_ok()
        );
    }
}
