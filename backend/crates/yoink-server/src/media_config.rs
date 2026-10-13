use std::{fmt, path::PathBuf, time::Duration};

use yoink_downloads::{DownloadConfig, DownloadNetworkConfig, PersonalCookieLimits};
use yoink_executor::ExecutorConfig;
use yoink_storage::{AdmissionPolicy, ClaimPolicy, QuotaLimits};

use crate::config::{ConfigError, invalid};

pub struct MediaConfig {
    pub executor: ExecutorConfig,
    pub downloads: DownloadConfig,
    pub network: DownloadNetworkConfig,
    pub cookie_web: CookieWebConfig,
    pub youtube_auth_adapter: bool,
}

#[derive(Debug)]
pub struct CookieWebConfig {
    pub origin: Option<url::Url>,
    pub oauth_timeout: Duration,
    pub account_timeout: Duration,
    pub oauth_poll: yoink_downloads::cookie_oauth::OAuthPollConfig,
    pub oauth_cleanup_interval: Duration,
}

impl fmt::Debug for MediaConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaConfig")
            .field("downloads", &self.downloads)
            .field("executor", &"confined worker launcher")
            .field("network", &self.network)
            .field("cookie_web", &self.cookie_web)
            .field("youtube_auth_adapter", &self.youtube_auth_adapter)
            .finish()
    }
}

impl MediaConfig {
    pub fn from_lookup(
        read: &mut impl FnMut(&str) -> Result<Option<String>, ConfigError>,
        shutdown: Duration,
    ) -> Result<Self, ConfigError> {
        let concurrency = number(read, "DOWNLOAD_CONCURRENCY", 2, 1, 8)? as usize;
        let lease = Duration::from_secs(number(read, "DOWNLOAD_LEASE_SECS", 60, 3, 300)?);
        let heartbeat_interval =
            Duration::from_secs(number(read, "DOWNLOAD_HEARTBEAT_SECS", 5, 1, 100)?);
        let delivery_timeout =
            Duration::from_secs(number(read, "DELIVERY_TIMEOUT_SECS", 30, 1, 300)?);
        let cache_ttl = Duration::from_secs(number(
            read,
            "DOWNLOAD_CACHE_TTL_SECS",
            2_592_000,
            0,
            31_536_000,
        )?);
        if lease < heartbeat_interval * 3 || delivery_timeout > lease {
            return Err(invalid(
                "DOWNLOAD_LEASE_SECS",
                "must cover three heartbeats and the delivery timeout",
            ));
        }
        if shutdown <= delivery_timeout + heartbeat_interval {
            return Err(invalid(
                "SHUTDOWN_TIMEOUT_SECS",
                "must exceed delivery timeout plus heartbeat interval",
            ));
        }
        let max_artifact_bytes = number(
            read,
            "DOWNLOAD_MAX_BYTES",
            20 * 1024 * 1024,
            1,
            2_000_000_000,
        )?;
        let max_workspace_bytes = number(
            read,
            "DOWNLOAD_WORKSPACE_MAX_BYTES",
            100 * 1024 * 1024,
            max_artifact_bytes,
            8_000_000_000,
        )?;
        let workspace_root =
            PathBuf::from(read("DOWNLOAD_WORKSPACE")?.unwrap_or_else(|| "/tmp/yoink-work".into()));
        if !workspace_root.is_absolute() || workspace_root.parent().is_none() {
            return Err(invalid(
                "DOWNLOAD_WORKSPACE",
                "expected an absolute dedicated directory, not filesystem root",
            ));
        }
        let allowed_private_origins = origins(read, "WORKER_ALLOWED_PRIVATE_ORIGINS")?;
        let allowed_proxy_origins = origins(read, "WORKER_ALLOWED_PROXY_ORIGINS")?;
        let timeout = Duration::from_secs(number(read, "DOWNLOAD_TIMEOUT_SECS", 120, 1, 3600)?);
        let processing_timeout = Duration::from_secs(number(
            read,
            "WORKER_PROCESSING_TIMEOUT_SECS",
            timeout.as_secs().min(300),
            1,
            3600,
        )?);
        if processing_timeout > timeout {
            return Err(invalid(
                "WORKER_PROCESSING_TIMEOUT_SECS",
                "must not exceed DOWNLOAD_TIMEOUT_SECS",
            ));
        }
        // The Config consumer already adds YOINK_RUST_. Retain the old
        // doubled-prefix spelling as an alias, rejecting conflicting authority.
        let system_proxy = network_setting(read, "DOWNLOAD_PROXY_URL")?;
        let no_cookie_domains: Vec<String> = serde_json::from_str(
            &network_setting(read, "DOWNLOAD_NO_COOKIE_DOMAINS_JSON")?
                .unwrap_or_else(|| "[]".into()),
        )
        .map_err(|_| {
            invalid(
                "DOWNLOAD_NO_COOKIE_DOMAINS_JSON",
                "expected a bounded JSON array of hostnames",
            )
        })?;
        if no_cookie_domains.len() > 64 {
            return Err(invalid(
                "DOWNLOAD_NO_COOKIE_DOMAINS_JSON",
                "at most sixty-four domains are allowed",
            ));
        }
        let limits = PersonalCookieLimits {
            domains: number(read, "YOINK_RUST_PERSONAL_COOKIE_DOMAINS", 128, 1, 1000000)?,
            actor_bytes: number(
                read,
                "YOINK_RUST_PERSONAL_COOKIE_ACTOR_BYTES",
                64 * 1024 * 1024,
                1,
                i64::MAX as u64,
            )?,
            global_bytes: number(
                read,
                "YOINK_RUST_PERSONAL_COOKIE_GLOBAL_BYTES",
                1024 * 1024 * 1024,
                1,
                i64::MAX as u64,
            )?,
        };
        let network = DownloadNetworkConfig::new(system_proxy, no_cookie_domains)
            .map_err(|_| {
                invalid(
                    "DOWNLOAD_PROXY_URL",
                    "proxy or no-cookie domain configuration is invalid",
                )
            })?
            .with_personal_cookie_limits(limits)
            .map_err(|_| {
                invalid(
                    "YOINK_RUST_PERSONAL_COOKIE_GLOBAL_BYTES",
                    "invalid personal cookie storage limits",
                )
            })?;
        let origin = read("COOKIE_WEB_ORIGIN")?
            .map(|value| {
                let origin = url::Url::parse(&value)
                    .map_err(|_| invalid("COOKIE_WEB_ORIGIN", "expected an HTTPS origin"))?;
                if origin.scheme() != "https"
                    || origin.host_str().is_none()
                    || !origin.username().is_empty()
                    || origin.password().is_some()
                    || origin.path() != "/"
                    || origin.query().is_some()
                    || origin.fragment().is_some()
                {
                    return Err(invalid(
                        "COOKIE_WEB_ORIGIN",
                        "expected an HTTPS origin without credentials, path, query or fragment",
                    ));
                }
                Ok(origin)
            })
            .transpose()?;
        let oauth_poll = yoink_downloads::cookie_oauth::OAuthPollConfig {
            budget: Duration::from_secs(number(
                read,
                "COOKIE_OAUTH_POLL_BUDGET_SECS",
                120,
                61,
                294,
            )?),
            result_retention: Duration::from_secs(number(
                read,
                "COOKIE_OAUTH_RESULT_RETENTION_SECS",
                1800,
                60,
                86400,
            )?),
            max_inflight: number(read, "COOKIE_OAUTH_MAX_INFLIGHT", 128, 1, 10000)? as usize,
        };
        if shutdown <= oauth_poll.budget + Duration::from_secs(5) {
            return Err(invalid(
                "SHUTDOWN_TIMEOUT_SECS",
                "must exceed COOKIE_OAUTH_POLL_BUDGET_SECS by more than five seconds",
            ));
        }
        let cookie_web = CookieWebConfig {
            oauth_poll,
            oauth_cleanup_interval: Duration::from_secs(number(
                read,
                "COOKIE_OAUTH_CLEANUP_INTERVAL_SECS",
                60,
                1,
                3600,
            )?),
            origin,
            oauth_timeout: Duration::from_secs(number(
                read,
                "COOKIE_OAUTH_TIMEOUT_SECS",
                15,
                1,
                60,
            )?),
            account_timeout: Duration::from_secs(number(
                read,
                "COOKIE_ACCOUNT_TIMEOUT_SECS",
                10,
                1,
                60,
            )?),
        };
        let youtube_auth_adapter = match read("YOUTUBE_AUTH_ADAPTER_ENABLED")?
            .as_deref()
            .unwrap_or("false")
        {
            "true" => true,
            "false" => false,
            _ => {
                return Err(invalid(
                    "YOUTUBE_AUTH_ADAPTER_ENABLED",
                    "expected true or false",
                ));
            }
        };
        Ok(Self {
            network,
            cookie_web,
            youtube_auth_adapter,
            executor: ExecutorConfig {
                program: "/usr/local/bin/yoink-worker-launcher".into(),
                args: vec![],
                workspace_root,
                timeout,
                processing_timeout,
                socket_timeout: Duration::from_secs(number(
                    read,
                    "WORKER_SOCKET_TIMEOUT_SECS",
                    15,
                    1,
                    300,
                )?),
                max_artifact_bytes,
                max_workspace_bytes,
                max_output_bytes: number(
                    read,
                    "WORKER_MAX_OUTPUT_BYTES",
                    1024 * 1024,
                    65536,
                    4 * 1024 * 1024,
                )? as usize,
                max_artifacts: number(read, "DOWNLOAD_MAX_ITEMS", 20, 1, 100)? as usize,
                allowed_proxy_origins,
                allowed_private_origins,
            },
            downloads: DownloadConfig {
                admission: AdmissionPolicy {
                    max_attempts: number(read, "DOWNLOAD_MAX_ATTEMPTS", 3, 1, 10)? as u32,
                    retry_delay: Duration::from_secs(number(
                        read,
                        "DOWNLOAD_RETRY_DELAY_SECS",
                        5,
                        0,
                        300,
                    )?),
                    max_pending_global: number(read, "DOWNLOAD_MAX_PENDING", 100, 1, 10000)? as u32,
                    max_pending_actor: number(read, "DOWNLOAD_MAX_PENDING_PER_USER", 3, 1, 1000)?
                        as u32,
                },
                quotas: QuotaLimits {
                    minute: number(read, "DOWNLOAD_QUOTA_MINUTE", 5, 1, 1_000_000)? as u32,
                    hour: number(read, "DOWNLOAD_QUOTA_HOUR", 30, 1, 1_000_000)? as u32,
                    day: number(read, "DOWNLOAD_QUOTA_DAY", 100, 1, 1_000_000)? as u32,
                },
                claim: ClaimPolicy {
                    max_active_actor: number(read, "DOWNLOAD_MAX_ACTIVE_PER_USER", 3, 1, 100)?
                        as u32,
                },
                concurrency,
                lease,
                heartbeat_interval,
                poll_interval: Duration::from_millis(number(
                    read,
                    "DOWNLOAD_POLL_INTERVAL_MS",
                    250,
                    10,
                    10000,
                )?),
                delivery_timeout,
                cache_ttl,
            },
        })
    }
}

fn network_setting(
    read: &mut impl FnMut(&str) -> Result<Option<String>, ConfigError>,
    key: &str,
) -> Result<Option<String>, ConfigError> {
    let current = read(key)?;
    let legacy = read(&format!("YOINK_RUST_{key}"))?;
    if current
        .as_ref()
        .zip(legacy.as_ref())
        .is_some_and(|(a, b)| a != b)
    {
        return Err(invalid(
            key,
            "conflicting current and legacy network configuration; set only one spelling",
        ));
    }
    Ok(current.or(legacy))
}

fn origins(
    read: &mut impl FnMut(&str) -> Result<Option<String>, ConfigError>,
    key: &str,
) -> Result<Vec<String>, ConfigError> {
    let values: Vec<String> = serde_json::from_str(&read(key)?.unwrap_or_else(|| "[]".into()))
        .map_err(|_| invalid(key, "expected a JSON array of HTTP(S) origins"))?;
    if values.len() > 16 {
        return Err(invalid(key, "at most sixteen origins are allowed"));
    }
    for origin in &values {
        let parsed = url::Url::parse(origin).map_err(|_| invalid(key, "invalid origin"))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || origin.contains("@")
            || parsed.password().is_some()
            || parsed.port() == Some(0)
            || !matches!(parsed.path(), "" | "/")
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || origin
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err(invalid(
                key,
                "origins must not contain credentials, paths, queries or fragments and must use a nonzero port",
            ));
        }
    }
    Ok(values)
}

fn number(
    read: &mut impl FnMut(&str) -> Result<Option<String>, ConfigError>,
    key: &str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, ConfigError> {
    let value = read(key)?
        .map(|s| s.parse::<u64>())
        .transpose()
        .map_err(|_| invalid(key, "expected a bounded nonnegative integer"))?
        .unwrap_or(default);
    if !(min..=max).contains(&value) {
        return Err(invalid(key, "integer is outside the supported range"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(entries: &[(&str, &str)]) -> Result<MediaConfig, ConfigError> {
        MediaConfig::from_lookup(
            &mut |key| {
                Ok(entries
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.to_string()))
            },
            Duration::from_secs(135),
        )
    }

    #[test]
    fn cookie_controls_have_bounded_defaults_and_explicit_origin() {
        let default = parse(&[]).unwrap();
        assert!(default.cookie_web.origin.is_none());
        assert!(!default.youtube_auth_adapter);
        assert_eq!(default.cookie_web.oauth_timeout, Duration::from_secs(15));
        assert_eq!(default.cookie_web.account_timeout, Duration::from_secs(10));
        let configured = parse(&[
            ("COOKIE_WEB_ORIGIN", "https://yoink.example"),
            ("COOKIE_OAUTH_TIMEOUT_SECS", "60"),
            ("COOKIE_ACCOUNT_TIMEOUT_SECS", "1"),
            ("YOUTUBE_AUTH_ADAPTER_ENABLED", "true"),
            ("COOKIE_OAUTH_POLL_BUDGET_SECS", "61"),
            ("COOKIE_OAUTH_RESULT_RETENTION_SECS", "60"),
            ("COOKIE_OAUTH_MAX_INFLIGHT", "1"),
            ("COOKIE_OAUTH_CLEANUP_INTERVAL_SECS", "1"),
        ])
        .unwrap();
        assert!(configured.youtube_auth_adapter);
        assert_eq!(configured.cookie_web.oauth_poll.budget.as_secs(), 61);
        assert_eq!(
            configured.cookie_web.oauth_poll.result_retention.as_secs(),
            60
        );
        assert_eq!(configured.cookie_web.oauth_poll.max_inflight, 1);
        assert_eq!(configured.cookie_web.oauth_cleanup_interval.as_secs(), 1);
        assert_eq!(
            configured.cookie_web.origin.unwrap().as_str(),
            "https://yoink.example/"
        );
        assert_eq!(configured.cookie_web.oauth_timeout, Duration::from_secs(60));
        assert_eq!(
            configured.cookie_web.account_timeout,
            Duration::from_secs(1)
        );
    }

    #[test]
    fn cookie_controls_reject_unsafe_origins_and_unbounded_deadlines() {
        for origin in [
            "http://yoink.example",
            "https://user@yoink.example",
            "https://yoink.example/path",
            "https://yoink.example/?query=x",
            "https://yoink.example/#fragment",
            "",
        ] {
            let error = parse(&[("COOKIE_WEB_ORIGIN", origin)])
                .unwrap_err()
                .to_string();
            assert!(error.contains("YOINK_RUST_COOKIE_WEB_ORIGIN"));
        }
        for name in ["COOKIE_OAUTH_TIMEOUT_SECS", "COOKIE_ACCOUNT_TIMEOUT_SECS"] {
            for value in ["0", "61", "-1", "garbage"] {
                let error = parse(&[(name, value)]).unwrap_err().to_string();
                assert!(error.contains(name));
            }
        }
    }

    #[test]
    fn cookie_poll_lifecycle_rejects_invalid_limits_and_short_shutdown() {
        for (name, value) in [
            ("COOKIE_OAUTH_POLL_BUDGET_SECS", "60"),
            ("COOKIE_OAUTH_POLL_BUDGET_SECS", "295"),
            ("COOKIE_OAUTH_RESULT_RETENTION_SECS", "59"),
            ("COOKIE_OAUTH_RESULT_RETENTION_SECS", "86401"),
            ("COOKIE_OAUTH_MAX_INFLIGHT", "0"),
            ("COOKIE_OAUTH_MAX_INFLIGHT", "10001"),
            ("COOKIE_OAUTH_CLEANUP_INTERVAL_SECS", "0"),
            ("COOKIE_OAUTH_CLEANUP_INTERVAL_SECS", "3601"),
        ] {
            assert!(
                parse(&[(name, value)])
                    .unwrap_err()
                    .to_string()
                    .contains(name)
            );
        }
        assert!(
            MediaConfig::from_lookup(
                &mut |key| Ok((key == "COOKIE_OAUTH_POLL_BUDGET_SECS").then(|| "294".to_owned())),
                Duration::from_secs(300),
            )
            .is_ok()
        );
        let error = MediaConfig::from_lookup(&mut |_| Ok(None), Duration::from_secs(125))
            .unwrap_err()
            .to_string();
        assert!(error.contains("SHUTDOWN_TIMEOUT_SECS"));
        assert!(error.contains("COOKIE_OAUTH_POLL_BUDGET_SECS"));
    }

    #[test]
    fn personal_cookie_limits_are_finite_configurable_and_secret_free() {
        assert!(
            parse(&[
                ("YOINK_RUST_PERSONAL_COOKIE_DOMAINS", "2"),
                ("YOINK_RUST_PERSONAL_COOKIE_ACTOR_BYTES", "1024"),
                ("YOINK_RUST_PERSONAL_COOKIE_GLOBAL_BYTES", "2048")
            ])
            .is_ok()
        );
        for key in [
            "YOINK_RUST_PERSONAL_COOKIE_DOMAINS",
            "YOINK_RUST_PERSONAL_COOKIE_ACTOR_BYTES",
            "YOINK_RUST_PERSONAL_COOKIE_GLOBAL_BYTES",
        ] {
            for bad in ["0", "-1", "9223372036854775808", "secret-cookie-value"] {
                let error = parse(&[(key, bad)]).unwrap_err();
                assert!(!format!("{error:?}").contains("secret-cookie-value"));
            }
        }
    }

    #[test]
    fn bounded_defaults_and_fixed_worker_program() {
        let config = parse(&[]).unwrap();
        assert_eq!(config.downloads.concurrency, 2);
        assert_eq!(config.downloads.quotas, QuotaLimits::default());
        assert_eq!(config.downloads.claim, ClaimPolicy::default());
        assert_eq!(config.downloads.cache_ttl, Duration::from_secs(2_592_000));
        assert_eq!(
            config.executor.program,
            PathBuf::from("/usr/local/bin/yoink-worker-launcher")
        );
        assert!(config.executor.allowed_private_origins.is_empty());
        assert!(config.executor.allowed_proxy_origins.is_empty());
        assert_eq!(config.executor.args, Vec::<String>::new());
        assert_eq!(config.executor.max_artifacts, 20);
        assert_eq!(config.executor.timeout, Duration::from_secs(120));
        assert_eq!(config.executor.processing_timeout, Duration::from_secs(120));
    }

    #[test]
    fn rejects_unsafe_or_unbounded_configuration_without_echoing_values() {
        for entries in [
            vec![("DOWNLOAD_CONCURRENCY", "0")],
            vec![("YOUTUBE_AUTH_ADAPTER_ENABLED", "yes")],
            vec![("DOWNLOAD_QUOTA_MINUTE", "0")],
            vec![("DOWNLOAD_QUOTA_HOUR", "-1")],
            vec![("DOWNLOAD_QUOTA_DAY", "1000001")],
            vec![("DOWNLOAD_QUOTA_MINUTE", "secret")],
            vec![("DOWNLOAD_MAX_ACTIVE_PER_USER", "0")],
            vec![("DOWNLOAD_MAX_ACTIVE_PER_USER", "101")],
            vec![("DOWNLOAD_MAX_BYTES", "2000000001")],
            vec![("DOWNLOAD_WORKSPACE", "/")],
            vec![("DOWNLOAD_WORKSPACE", "relative")],
            vec![("DOWNLOAD_LEASE_SECS", "10")],
            vec![("DOWNLOAD_HEARTBEAT_SECS", "21")],
            vec![("WORKER_PROCESSING_TIMEOUT_SECS", "121")],
            vec![("WORKER_PROCESSING_TIMEOUT_SECS", "0")],
            vec![("DOWNLOAD_MAX_ITEMS", "101")],
            vec![("DOWNLOAD_CACHE_TTL_SECS", "-1")],
            vec![("DOWNLOAD_CACHE_TTL_SECS", "31536001")],
            vec![("WORKER_ALLOWED_PROXY_ORIGINS", "[\"http://host:0\"]")],
            vec![("WORKER_ALLOWED_PRIVATE_ORIGINS", "[\"http://@host\"]")],
            vec![("WORKER_ALLOWED_PROXY_ORIGINS", "[\"http://host\\n.evil\"]")],
            vec![(
                "WORKER_ALLOWED_PRIVATE_ORIGINS",
                "[\"http://user:secret@host\"]",
            )],
            vec![(
                "WORKER_ALLOWED_PRIVATE_ORIGINS",
                "[\"https://host/path?secret\"]",
            )],
            vec![("WORKER_ALLOWED_PRIVATE_ORIGINS", "secret")],
        ] {
            let error = parse(&entries).unwrap_err();
            assert!(!format!("{error:?} {error}").contains("secret"));
        }
        assert!(MediaConfig::from_lookup(&mut |_| Ok(None), Duration::from_secs(30)).is_err());
    }

    #[test]
    fn quotas_and_active_limits_accept_bounds_independently() {
        let config = parse(&[
            ("DOWNLOAD_QUOTA_MINUTE", "1"),
            ("DOWNLOAD_QUOTA_HOUR", "1000000"),
            ("DOWNLOAD_QUOTA_DAY", "2"),
            ("DOWNLOAD_MAX_ACTIVE_PER_USER", "100"),
        ])
        .unwrap();
        assert_eq!(
            config.downloads.quotas,
            QuotaLimits {
                minute: 1,
                hour: 1_000_000,
                day: 2
            }
        );
        assert_eq!(config.downloads.claim.max_active_actor, 100);
        // Pending admission defaults are intentionally not normalized to library values.
        assert_eq!(config.downloads.admission.max_pending_global, 100);
        assert_eq!(config.downloads.admission.max_pending_actor, 3);
    }

    #[test]
    fn cache_ttl_accepts_disabled_and_maximum_values() {
        assert_eq!(
            parse(&[("DOWNLOAD_CACHE_TTL_SECS", "0")])
                .unwrap()
                .downloads
                .cache_ttl,
            Duration::ZERO
        );
        assert_eq!(
            parse(&[("DOWNLOAD_CACHE_TTL_SECS", "31536000")])
                .unwrap()
                .downloads
                .cache_ttl,
            Duration::from_secs(31_536_000)
        );
    }

    #[test]
    fn private_origin_exception_is_explicit_and_redacted() {
        let config = parse(&[(
            "WORKER_ALLOWED_PRIVATE_ORIGINS",
            "[\"http://127.0.0.1:19876\"]",
        )])
        .unwrap();
        assert_eq!(config.executor.allowed_private_origins.len(), 1);
        assert!(!format!("{config:?}").contains("19876"));
        let config = parse(&[
            (
                "WORKER_ALLOWED_PRIVATE_ORIGINS",
                "[\"http://private.example:8080\"]",
            ),
            (
                "WORKER_ALLOWED_PROXY_ORIGINS",
                "[\"https://proxy.example:9443\"]",
            ),
            ("DOWNLOAD_TIMEOUT_SECS", "600"),
        ])
        .unwrap();
        assert_eq!(
            config.executor.allowed_private_origins,
            ["http://private.example:8080"]
        );
        assert_eq!(
            config.executor.allowed_proxy_origins,
            ["https://proxy.example:9443"]
        );
        assert_eq!(config.executor.processing_timeout, Duration::from_secs(300));
        assert_eq!(config.executor.max_artifacts, 20);
    }
}
