use crate::config::{ConfigError, invalid};
use std::{path::PathBuf, time::Duration};

pub struct InboxConfig {
    pub executor: yoink_executor::ExecutorConfig,
    pub insight: yoink_insight::Config,
    pub github_endpoint: Option<url::Url>,
    pub job: Duration,
    pub drain: Duration,
    pub lease: Duration,
    pub interval: Duration,
    pub github_timeout: Duration,
    pub text_timeout: Duration,
    pub batch: usize,
    pub max_attempts: i64,
    pub notifications: yoink_telegram::InboxRuntimeConfig,
}
impl InboxConfig {
    #[cfg(test)]
    pub fn from_lookup(
        read: &mut impl FnMut(&str) -> Result<Option<String>, ConfigError>,
        shutdown: Duration,
        database: bool,
    ) -> Result<Option<Self>, ConfigError> {
        Self::from_lookup_with_insight(read, shutdown, database, None)
    }
    pub fn from_lookup_with_insight(
        read: &mut impl FnMut(&str) -> Result<Option<String>, ConfigError>,
        shutdown: Duration,
        database: bool,
        shared: Option<&yoink_insight::Config>,
    ) -> Result<Option<Self>, ConfigError> {
        match read("INBOX_ENABLED")?.as_deref().unwrap_or("false") {
            "false" => return Ok(None),
            "true" => (),
            _ => return Err(invalid("INBOX_ENABLED", "expected true or false")),
        }
        if !database {
            return Err(invalid("INBOX_ENABLED", "requires an explicit database"));
        }
        fn n(
            read: &mut impl FnMut(&str) -> Result<Option<String>, ConfigError>,
            key: &str,
            default: u64,
            max: u64,
        ) -> Result<u64, ConfigError> {
            let v = read(key)?
                .map(|v| v.parse::<u64>())
                .transpose()
                .map_err(|_| invalid(key, "expected bounded integer"))?
                .unwrap_or(default);
            if v == 0 || v > max {
                return Err(invalid(key, "outside supported bounds"));
            }
            Ok(v)
        }
        let job = Duration::from_secs(n(read, "INBOX_JOB_TIMEOUT_SECS", 60, 240)?);
        let drain = Duration::from_secs(n(read, "INBOX_DRAIN_TIMEOUT_SECS", 5, 30)?);
        let lease = Duration::from_secs(n(read, "INBOX_LEASE_SECS", 120, 3600)?);
        let text_timeout = Duration::from_secs(n(read, "INBOX_TEXT_TIMEOUT_SECS", 5, 60)?);
        if job + drain >= lease || job + drain >= shutdown || text_timeout + drain >= shutdown {
            return Err(invalid(
                "INBOX_JOB_TIMEOUT_SECS",
                "job plus drain must be below lease and shutdown budgets; text plus drain must be below shutdown",
            ));
        }
        let stage = Duration::from_secs(n(read, "INBOX_EXECUTOR_TIMEOUT_SECS", 20, 60)?);
        let socket = Duration::from_secs(n(read, "INBOX_SOCKET_TIMEOUT_SECS", 15, 60)?);
        let completion = Duration::from_secs(n(read, "INBOX_COMPLETION_TIMEOUT_SECS", 30, 120)?);
        let github_timeout = Duration::from_secs(n(read, "INBOX_GITHUB_TIMEOUT_SECS", 15, 60)?);
        if stage > job || socket > stage || completion > job || github_timeout > job {
            return Err(invalid(
                "INBOX_JOB_TIMEOUT_SECS",
                "must cover adapter deadlines",
            ));
        }
        let workspace_root =
            PathBuf::from(read("INBOX_WORKSPACE")?.unwrap_or_else(|| "/tmp/yoink-inbox".into()));
        if !workspace_root.is_absolute() || workspace_root.parent().is_none() {
            return Err(invalid(
                "INBOX_WORKSPACE",
                "requires dedicated absolute directory",
            ));
        }
        let github_endpoint = read("INBOX_GITHUB_ENDPOINT")?
            .map(|s| {
                url::Url::parse(&s).map_err(|_| {
                    invalid("INBOX_GITHUB_ENDPOINT", "expected trusted HTTP(S) endpoint")
                })
            })
            .transpose()?;
        if github_endpoint.as_ref().is_some_and(|u| {
            !matches!(u.scheme(), "http" | "https")
                || u.host_str().is_none()
                || !u.username().is_empty()
                || u.password().is_some()
                || u.query().is_some()
                || u.fragment().is_some()
        }) {
            return Err(invalid(
                "INBOX_GITHUB_ENDPOINT",
                "expected trusted HTTP(S) endpoint without credentials",
            ));
        }
        let mut insight = yoink_insight::Config {
            request_timeout: completion,
            max_chars: n(read, "INBOX_CONTENT_MAX_CHARS", 40000, 128000)? as usize,
            ..Default::default()
        };
        if insight.max_chars < 100 {
            return Err(invalid("INBOX_CONTENT_MAX_CHARS", "minimum is 100"));
        }
        insight = match shared {
            Some(config) => config.clone(),
            None => crate::insight_config::completion_config(read, insight)?,
        };
        let notification_lease =
            Duration::from_secs(n(read, "INBOX_NOTIFICATION_LEASE_SECS", 120, 3600)?);
        let recovery_batch = n(read, "INBOX_RECOVERY_BATCH", 64, 256)? as usize;
        let dispatch_batch = n(read, "INBOX_DISPATCH_BATCH", 1, 16)? as usize;
        if notification_lease <= text_timeout
            || text_timeout * dispatch_batch as u32 + drain >= shutdown
        {
            return Err(invalid(
                "INBOX_NOTIFICATION_LEASE_SECS",
                "must exceed request deadline and dispatch must fit shutdown",
            ));
        }
        Ok(Some(Self {
            notifications: yoink_telegram::InboxRuntimeConfig {
                notification_lease,
                recovery_batch,
                dispatch_batch,
            },
            executor: yoink_executor::ExecutorConfig {
                program: "/usr/local/bin/yoink-worker-launcher".into(),
                args: vec![],
                workspace_root,
                timeout: stage,
                processing_timeout: stage,
                socket_timeout: socket,
                max_artifact_bytes: 1024 * 1024,
                max_workspace_bytes: 2 * 1024 * 1024,
                max_output_bytes: 1024 * 1024,
                max_artifacts: 1,
                allowed_proxy_origins: vec![],
                allowed_private_origins: vec![],
            },
            insight,
            github_endpoint,
            job,
            drain,
            lease,
            text_timeout,
            github_timeout,
            interval: Duration::from_millis(n(read, "INBOX_INTERVAL_MS", 1000, 60000)?),
            batch: n(read, "INBOX_BATCH", 1, 16)? as usize,
            max_attempts: n(read, "INBOX_MAX_ATTEMPTS", 3, 10)? as i64,
        }))
    }
}
impl std::fmt::Debug for InboxConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InboxConfig([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(
        entries: &[(&str, &str)],
        shutdown: u64,
        db: bool,
    ) -> Result<Option<InboxConfig>, ConfigError> {
        InboxConfig::from_lookup(
            &mut |key| {
                Ok(entries
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.to_string()))
            },
            Duration::from_secs(shutdown),
            db,
        )
    }
    #[test]
    fn disabled_needs_no_database() {
        assert!(config(&[], 10, false).unwrap().is_none());
    }
    #[test]
    fn enabled_is_independent_of_media() {
        let c = config(&[("INBOX_ENABLED", "true")], 90, true)
            .unwrap()
            .unwrap();
        assert_eq!(c.batch, 1);
        assert!(c.executor.allowed_private_origins.is_empty());
    }
    #[test]
    fn invalid_budgets_and_bounds() {
        for (key, value) in [
            ("INBOX_JOB_TIMEOUT_SECS", "0"),
            ("INBOX_BATCH", "17"),
            ("INBOX_INTERVAL_MS", "60001"),
            ("INBOX_CONTENT_MAX_CHARS", "99"),
            ("INBOX_LEASE_SECS", "65"),
            ("INBOX_SOCKET_TIMEOUT_SECS", "60"),
            ("INBOX_WORKSPACE", "/"),
            ("INBOX_GITHUB_ENDPOINT", "https://secret@github.invalid/"),
        ] {
            assert!(
                config(&[("INBOX_ENABLED", "true"), (key, value)], 90, true).is_err(),
                "{key}"
            );
        }
        assert!(config(&[("INBOX_ENABLED", "true")], 65, true).is_err());
        assert!(config(&[("INBOX_ENABLED", "true")], 90, false).is_err());
    }
    #[test]
    fn secrets_are_redacted() {
        let c = config(
            &[
                ("INBOX_ENABLED", "true"),
                ("INSIGHT_GATEWAY_KEY", "secret-value"),
            ],
            90,
            true,
        )
        .unwrap()
        .unwrap();
        assert!(!format!("{c:?}").contains("secret-value"));
    }
}
