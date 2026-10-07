use std::{fmt, net::SocketAddr, path::PathBuf, time::Duration};

use thiserror::Error;
use url::Url;

const PREFIX: &str = "YOINK_RUST_";

/// Never prints its contents, including through the parent configuration's Debug.
pub struct SecretToken(String);

impl SecretToken {
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Debug for SecretToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

#[derive(Debug)]
pub struct DatabaseConfig {
    pub path: PathBuf,
    pub acquire_timeout: Duration,
    pub health_interval: Duration,
}

impl DatabaseConfig {
    pub async fn open(
        &self,
        create_if_missing: bool,
    ) -> Result<yoink_storage::Store, yoink_storage::StoreError> {
        let create = create_if_missing
            && !self
                .path
                .try_exists()
                .map_err(|_| yoink_storage::StoreError::Database)?;
        yoink_storage::Store::open(&self.path, create, self.acquire_timeout).await
    }
}

#[derive(Debug)]
pub struct TelegramConfig {
    pub token: SecretToken,
    pub owner_id: u64,
    pub api_url: Option<Url>,
    pub poll_timeout: Duration,
    pub progress: yoink_telegram::ProgressConfig,
    pub inline_search: yoink_telegram::InlineSearchConfig,
    pub cookie_upload: yoink_telegram::CookieUploadConfig,
    pub cookie_url: yoink_telegram::CookieUrlConfig,
    pub groups_enabled: bool,
}

#[derive(Debug)]
pub struct Config {
    pub inbox: Option<crate::inbox_config::InboxConfig>,
    pub user_session: Option<yoink_user_session::Config>,
    pub auth: Option<crate::auth_config::AuthHttpConfig>,
    pub http_bind: SocketAddr,
    pub telegram: Option<TelegramConfig>,
    pub startup_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub database: Option<DatabaseConfig>,
    pub media: Option<crate::media_config::MediaConfig>,
}

#[derive(Debug, Error)]
#[error("invalid configuration {key}: {reason}")]
pub struct ConfigError {
    key: String,
    reason: &'static str,
}

pub(crate) fn invalid(key: &str, reason: &'static str) -> ConfigError {
    ConfigError {
        key: format!("{PREFIX}{key}"),
        reason,
    }
}

impl Config {
    /// Does not load .env or read the legacy Python configuration names.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| match std::env::var(key) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError {
                key: key.to_owned(),
                reason: "must contain Unicode text",
            }),
        })
    }

    fn from_lookup(
        mut lookup: impl FnMut(&str) -> Result<Option<String>, ConfigError>,
    ) -> Result<Self, ConfigError> {
        if lookup("DATABASE_URL")?.is_some()
            || lookup("MAX_CONNECTIONS")?.is_some()
            || lookup("YOINK_RUST_DATABASE_URL")?.is_some()
            || lookup("YOINK_RUST_DATABASE_MAX_CONNECTIONS")?.is_some()
            || lookup("YOINK_RUST_MAX_CONNECTIONS")?.is_some()
        {
            return Err(invalid(
                "DATABASE_PATH",
                "legacy DATABASE_URL and MAX_CONNECTIONS are unsupported; use YOINK_RUST_DATABASE_PATH",
            ));
        }
        let mut read = |key: &str| lookup(&format!("{PREFIX}{key}"));
        let http_bind = read("HTTP_BIND")?
            .unwrap_or_else(|| "127.0.0.1:8088".into())
            .parse()
            .map_err(|_| invalid("HTTP_BIND", "expected an IP address and port"))?;
        let startup_timeout = timeout(read("STARTUP_TIMEOUT_SECS")?, "STARTUP_TIMEOUT_SECS", 15)?;
        let media_enabled = match read("DOWNLOADS_ENABLED")?.as_deref().unwrap_or("false") {
            "true" => true,
            "false" => false,
            _ => return Err(invalid("DOWNLOADS_ENABLED", "expected true or false")),
        };
        let shutdown_timeout = timeout(
            read("SHUTDOWN_TIMEOUT_SECS")?,
            "SHUTDOWN_TIMEOUT_SECS",
            if media_enabled { 135 } else { 10 },
        )?;
        let enabled = match read("TELEGRAM_ENABLED")?.as_deref().unwrap_or("false") {
            "true" => true,
            "false" => false,
            _ => return Err(invalid("TELEGRAM_ENABLED", "expected true or false")),
        };
        let telegram = if enabled {
            let token = read("TELEGRAM_TOKEN")?
                .ok_or_else(|| invalid("TELEGRAM_TOKEN", "required when Telegram is enabled"))?;
            if token.trim().is_empty() || token.chars().any(char::is_whitespace) {
                return Err(invalid(
                    "TELEGRAM_TOKEN",
                    "must be nonempty and contain no whitespace",
                ));
            }
            let owner_id: u64 = read("OWNER_ID")?
                .ok_or_else(|| invalid("OWNER_ID", "required when Telegram is enabled"))?
                .parse()
                .map_err(|_| invalid("OWNER_ID", "expected a positive Telegram user ID"))?;
            if owner_id == 0 || owner_id > i64::MAX as u64 {
                return Err(invalid(
                    "OWNER_ID",
                    "expected a positive Telegram user ID within signed 64-bit range",
                ));
            }
            let api_url = read("BOT_API_URL")?
                .map(|value| validate_api_url(&value))
                .transpose()?;
            let poll_timeout = timeout(
                read("TELEGRAM_POLL_TIMEOUT_SECS")?,
                "TELEGRAM_POLL_TIMEOUT_SECS",
                30,
            )?;
            if poll_timeout.as_secs() > 50 {
                return Err(invalid(
                    "TELEGRAM_POLL_TIMEOUT_SECS",
                    "expected seconds in range 1..=50",
                ));
            }
            let inline_number = |key: &str,
                                 value: Option<String>,
                                 default: u64,
                                 min: u64,
                                 max: u64|
             -> Result<u64, ConfigError> {
                let value = value
                    .map(|value| value.parse::<u64>())
                    .transpose()
                    .map_err(|_| invalid(key, "expected a bounded integer"))?
                    .unwrap_or(default);
                if !(min..=max).contains(&value) {
                    return Err(invalid(key, "integer is outside the supported range"));
                }
                Ok(value)
            };
            let inline_search = yoink_telegram::InlineSearchConfig {
                concurrency: inline_number(
                    "TELEGRAM_INLINE_CONCURRENCY",
                    read("TELEGRAM_INLINE_CONCURRENCY")?,
                    3,
                    1,
                    8,
                )? as usize,
                timeout: Duration::from_secs(inline_number(
                    "TELEGRAM_INLINE_TIMEOUT_SECS",
                    read("TELEGRAM_INLINE_TIMEOUT_SECS")?,
                    20,
                    1,
                    120,
                )?),
                requests_per_second: inline_number(
                    "TELEGRAM_INLINE_REQUESTS_PER_SECOND",
                    read("TELEGRAM_INLINE_REQUESTS_PER_SECOND")?,
                    10,
                    1,
                    100,
                )? as u32,
                actor_interval: Duration::from_millis(inline_number(
                    "TELEGRAM_INLINE_ACTOR_INTERVAL_MS",
                    read("TELEGRAM_INLINE_ACTOR_INTERVAL_MS")?,
                    300,
                    1,
                    10000,
                )?),
            };
            let progress_interval = timeout(
                read("TELEGRAM_PROGRESS_INTERVAL_SECS")?,
                "TELEGRAM_PROGRESS_INTERVAL_SECS",
                2,
            )?;
            let progress_timeout = timeout(
                read("TELEGRAM_PROGRESS_TIMEOUT_SECS")?,
                "TELEGRAM_PROGRESS_TIMEOUT_SECS",
                10,
            )?;
            for (key, duration) in [
                ("TELEGRAM_PROGRESS_INTERVAL_SECS", progress_interval),
                ("TELEGRAM_PROGRESS_TIMEOUT_SECS", progress_timeout),
            ] {
                if duration.as_secs() > 60 {
                    return Err(invalid(key, "expected seconds in range 1..=60"));
                }
            }
            let progress = yoink_telegram::ProgressConfig::new(progress_interval, progress_timeout)
                .map_err(|_| {
                    invalid(
                        "TELEGRAM_PROGRESS_INTERVAL_SECS",
                        "invalid progress configuration",
                    )
                })?;
            let cookie_upload_timeout = timeout(
                read("TELEGRAM_COOKIE_UPLOAD_TIMEOUT_SECS")?,
                "TELEGRAM_COOKIE_UPLOAD_TIMEOUT_SECS",
                30,
            )?;
            let mut cookie_upload = yoink_telegram::CookieUploadConfig::new(cookie_upload_timeout)
                .map_err(|_| {
                    invalid(
                        "TELEGRAM_COOKIE_UPLOAD_TIMEOUT_SECS",
                        "expected seconds in range 1..=60",
                    )
                })?;
            match (
                read("BOT_API_FILE_ROOT")?,
                read("TELEGRAM_LOCAL_FILE_ROOT")?,
            ) {
                (None, None) => {}
                (Some(remote), Some(local)) if api_url.is_some() => {
                    cookie_upload = cookie_upload
                        .with_local_files(PathBuf::from(remote), PathBuf::from(local))
                        .map_err(|_| {
                            invalid(
                                "TELEGRAM_LOCAL_FILE_ROOT",
                                "expected absolute, non-root directories without traversal",
                            )
                        })?;
                }
                _ => {
                    return Err(invalid(
                        "TELEGRAM_LOCAL_FILE_ROOT",
                        "set both file roots and BOT_API_URL to enable local file reads",
                    ));
                }
            }
            let cookie_url_timeout = timeout(
                read("COOKIE_IMPORT_TIMEOUT_SECS")?,
                "COOKIE_IMPORT_TIMEOUT_SECS",
                30,
            )?;
            if cookie_url_timeout.as_secs() > 60 {
                return Err(invalid(
                    "COOKIE_IMPORT_TIMEOUT_SECS",
                    "expected seconds in range 1..=60",
                ));
            }
            let private_origins: Vec<String> = serde_json::from_str(
                &read("COOKIE_IMPORT_PRIVATE_ORIGINS")?.unwrap_or_else(|| "[]".into()),
            )
            .map_err(|_| {
                invalid(
                    "COOKIE_IMPORT_PRIVATE_ORIGINS",
                    "expected a JSON array of exact HTTP(S) origins",
                )
            })?;
            let cookie_url = yoink_telegram::CookieUrlConfig::new(cookie_url_timeout, private_origins)
                .map_err(|_| invalid("COOKIE_IMPORT_PRIVATE_ORIGINS", "expected at most 16 literal-IP origins with explicit ports and no credentials, paths, queries or fragments"))?;
            let groups_enabled = match read("TELEGRAM_GROUPS_ENABLED")?
                .as_deref()
                .unwrap_or("false")
            {
                "true" => true,
                "false" => false,
                _ => return Err(invalid("TELEGRAM_GROUPS_ENABLED", "expected true or false")),
            };
            Some(TelegramConfig {
                token: SecretToken(token),
                owner_id,
                api_url,
                poll_timeout,
                progress,
                inline_search,
                cookie_upload,
                cookie_url,
                groups_enabled,
            })
        } else {
            None
        };
        let database = if let Some(value) = read("DATABASE_PATH")? {
            let path = PathBuf::from(&value);
            if value.trim().is_empty() || !path.is_absolute() {
                return Err(invalid(
                    "DATABASE_PATH",
                    "expected a nonempty absolute local file path",
                ));
            }
            Some(DatabaseConfig {
                path,
                acquire_timeout: timeout(
                    read("DATABASE_ACQUIRE_TIMEOUT_SECS")?,
                    "DATABASE_ACQUIRE_TIMEOUT_SECS",
                    5,
                )?,
                health_interval: timeout(
                    read("DATABASE_HEALTH_INTERVAL_SECS")?,
                    "DATABASE_HEALTH_INTERVAL_SECS",
                    5,
                )?,
            })
        } else {
            None
        };
        let user_session = crate::user_session_config::from_lookup(&mut read)?;
        let auth = crate::auth_config::AuthHttpConfig::from_lookup(&mut read, database.is_some())?;
        let media = if media_enabled {
            if telegram.is_none() || database.is_none() {
                return Err(invalid(
                    "DOWNLOADS_ENABLED",
                    "requires explicit Telegram and database configuration",
                ));
            }
            Some(crate::media_config::MediaConfig::from_lookup(
                &mut read,
                shutdown_timeout,
            )?)
        } else {
            None
        };
        let inbox = crate::inbox_config::InboxConfig::from_lookup(
            &mut read,
            shutdown_timeout,
            database.is_some(),
        )?;
        if let (Some(inbox), Some(database)) = (&inbox, &database) {
            let recovery = database.acquire_timeout * 4;
            if inbox.job + inbox.drain + recovery >= inbox.lease
                || inbox.job + inbox.drain + recovery >= shutdown_timeout
                || inbox.text_timeout + recovery >= shutdown_timeout
            {
                return Err(invalid(
                    "INBOX_LEASE_SECS",
                    "job, cancellation drain and four DB acquisition budgets must fit strictly below lease and shutdown",
                ));
            }
        }
        Ok(Self {
            inbox,
            user_session,
            auth,
            http_bind,
            telegram,
            startup_timeout,
            shutdown_timeout,
            database,
            media,
        })
    }
}

fn timeout(value: Option<String>, key: &str, default: u64) -> Result<Duration, ConfigError> {
    let seconds = value
        .map(|s| s.parse::<u64>())
        .transpose()
        .map_err(|_| invalid(key, "expected seconds in range 1..=300"))?
        .unwrap_or(default);
    if !(1..=300).contains(&seconds) {
        return Err(invalid(key, "expected seconds in range 1..=300"));
    }
    Ok(Duration::from_secs(seconds))
}

fn validate_api_url(value: &str) -> Result<Url, ConfigError> {
    let url =
        Url::parse(value).map_err(|_| invalid("BOT_API_URL", "expected an HTTP(S) origin"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(invalid(
            "BOT_API_URL",
            "expected an HTTP(S) origin without credentials, path, query or fragment",
        ));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(values: &[(&str, &str)]) -> Result<Config, ConfigError> {
        Config::from_lookup(|key| {
            Ok(values
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.to_string()))
        })
    }

    #[test]
    fn defaults_are_local_and_offline() {
        let config = config(&[]).unwrap();
        assert!(config.http_bind.ip().is_loopback());
        assert!(config.telegram.is_none());
        assert_eq!(config.shutdown_timeout, Duration::from_secs(10));
    }

    #[test]
    fn legacy_token_does_not_enable_telegram() {
        assert!(
            config(&[("bot_token", "legacy-secret"), ("owner_id", "42")])
                .unwrap()
                .telegram
                .is_none()
        );
    }

    #[test]
    fn enabled_requires_both_credentials_and_owner() {
        let enabled = ("YOINK_RUST_TELEGRAM_ENABLED", "true");
        assert!(config(&[enabled]).is_err());
        assert!(config(&[enabled, ("YOINK_RUST_TELEGRAM_TOKEN", "secret")]).is_err());
        assert!(
            config(&[
                enabled,
                ("YOINK_RUST_TELEGRAM_TOKEN", "secret"),
                ("YOINK_RUST_OWNER_ID", "0")
            ])
            .is_err()
        );
    }

    #[test]
    fn group_access_is_separately_opted_in_with_strict_boolean_configuration() {
        let base = [
            ("YOINK_RUST_TELEGRAM_ENABLED", "true"),
            ("YOINK_RUST_TELEGRAM_TOKEN", "fixture-token"),
            ("YOINK_RUST_OWNER_ID", "42"),
        ];
        assert!(!config(&base).unwrap().telegram.unwrap().groups_enabled);
        for (value, expected) in [("true", true), ("false", false)] {
            let mut values = base.to_vec();
            values.push(("YOINK_RUST_TELEGRAM_GROUPS_ENABLED", value));
            assert_eq!(
                config(&values).unwrap().telegram.unwrap().groups_enabled,
                expected
            );
        }
        for value in ["yes", "1", "", "TRUE"] {
            let mut values = base.to_vec();
            values.push(("YOINK_RUST_TELEGRAM_GROUPS_ENABLED", value));
            assert!(config(&values).is_err());
        }
    }

    #[test]
    fn cookie_local_files_require_explicit_paired_roots_and_api_override() {
        let base = [
            ("YOINK_RUST_TELEGRAM_ENABLED", "true"),
            ("YOINK_RUST_TELEGRAM_TOKEN", "fixture-token"),
            ("YOINK_RUST_OWNER_ID", "42"),
        ];
        for extra in [
            vec![("YOINK_RUST_BOT_API_FILE_ROOT", "/remote")],
            vec![("YOINK_RUST_TELEGRAM_LOCAL_FILE_ROOT", "/local")],
            vec![
                ("YOINK_RUST_BOT_API_FILE_ROOT", "/remote"),
                ("YOINK_RUST_TELEGRAM_LOCAL_FILE_ROOT", "/local"),
            ],
            vec![
                ("YOINK_RUST_BOT_API_URL", "http://127.0.0.1:8082"),
                ("YOINK_RUST_BOT_API_FILE_ROOT", "/"),
                ("YOINK_RUST_TELEGRAM_LOCAL_FILE_ROOT", "/local"),
            ],
            vec![
                ("YOINK_RUST_BOT_API_URL", "http://127.0.0.1:8082"),
                ("YOINK_RUST_BOT_API_FILE_ROOT", "/remote"),
                ("YOINK_RUST_TELEGRAM_LOCAL_FILE_ROOT", "relative"),
            ],
        ] {
            let mut values = base.to_vec();
            values.extend(extra);
            assert!(config(&values).is_err());
        }
        let mut values = base.to_vec();
        values.extend([
            ("YOINK_RUST_BOT_API_URL", "http://127.0.0.1:8082"),
            ("YOINK_RUST_BOT_API_FILE_ROOT", "/remote/credential-marker"),
            (
                "YOINK_RUST_TELEGRAM_LOCAL_FILE_ROOT",
                "/local/credential-marker",
            ),
        ]);
        let loaded = config(&values).unwrap();
        assert!(!format!("{loaded:?}").contains("credential-marker"));
    }

    #[test]
    fn cookie_import_configuration_is_bounded_and_redacted() {
        let base = [
            ("YOINK_RUST_TELEGRAM_ENABLED", "true"),
            ("YOINK_RUST_TELEGRAM_TOKEN", "fixture-token"),
            ("YOINK_RUST_OWNER_ID", "42"),
        ];
        assert!(config(&base).is_ok());
        for value in ["0", "61", "1.5", "invalid", "", "-1"] {
            let mut values = base.to_vec();
            values.push(("YOINK_RUST_COOKIE_IMPORT_TIMEOUT_SECS", value));
            assert!(config(&values).is_err());
        }
        for value in ["1", "30", "60"] {
            let mut values = base.to_vec();
            values.push(("YOINK_RUST_COOKIE_IMPORT_TIMEOUT_SECS", value));
            assert!(config(&values).is_ok());
        }
        for value in [
            "{}",
            "null",
            "[1]",
            "not-json",
            r#"["http://localhost:8899"]"#,
            r#"["http://127.0.0.1"]"#,
            r#"["http://127.0.0.1:0"]"#,
            r#"["file:///credential-marker"]"#,
            r#"["http://user:credential-marker@127.0.0.1:8899"]"#,
            r#"["http://127.0.0.1:8899/credential-marker"]"#,
            r#"["http://127.0.0.1:8899?token=credential-marker"]"#,
            r#"["http://127.0.0.1:8899#credential-marker"]"#,
        ] {
            let mut values = base.to_vec();
            values.push(("YOINK_RUST_COOKIE_IMPORT_PRIVATE_ORIGINS", value));
            let error = config(&values).unwrap_err();
            assert!(!format!("{error:?} {error}").contains("credential-marker"));
        }
        let mut values = base.to_vec();
        values.push((
            "YOINK_RUST_COOKIE_IMPORT_PRIVATE_ORIGINS",
            r#"["http://127.0.0.1:8899","https://[::1]:9443/"]"#,
        ));
        let loaded = config(&values).unwrap();
        assert!(!format!("{loaded:?}").contains("8899"));
        let excess = serde_json::to_string(&vec!["http://127.0.0.1:8899"; 17]).unwrap();
        let mut values = base.to_vec();
        values.push(("YOINK_RUST_COOKIE_IMPORT_PRIVATE_ORIGINS", &excess));
        assert!(config(&values).is_err());
    }

    #[test]
    fn cookie_upload_timeout_accepts_bounds_and_rejects_invalid_values() {
        let base = [
            ("YOINK_RUST_TELEGRAM_ENABLED", "true"),
            ("YOINK_RUST_TELEGRAM_TOKEN", "fixture-token"),
            ("YOINK_RUST_OWNER_ID", "42"),
        ];
        assert!(config(&base).is_ok());
        for value in ["0", "61", "1.5", "invalid", "", "-1"] {
            let mut values = base.to_vec();
            values.push(("YOINK_RUST_TELEGRAM_COOKIE_UPLOAD_TIMEOUT_SECS", value));
            assert!(config(&values).is_err());
        }
        for value in ["1", "30", "60"] {
            let mut values = base.to_vec();
            values.push(("YOINK_RUST_TELEGRAM_COOKIE_UPLOAD_TIMEOUT_SECS", value));
            assert!(config(&values).is_ok());
        }
    }

    #[test]
    fn progress_settings_accept_bounds_and_reject_invalid_enabled_values() {
        let base = [
            ("YOINK_RUST_TELEGRAM_ENABLED", "true"),
            ("YOINK_RUST_TELEGRAM_TOKEN", "fixture-token"),
            ("YOINK_RUST_OWNER_ID", "42"),
        ];
        for key in [
            "YOINK_RUST_TELEGRAM_PROGRESS_INTERVAL_SECS",
            "YOINK_RUST_TELEGRAM_PROGRESS_TIMEOUT_SECS",
        ] {
            for value in ["0", "61", "1.5", "invalid"] {
                let mut values = base.to_vec();
                values.push((key, value));
                assert!(config(&values).is_err(), "{key}={value}");
            }
            for value in ["1", "60"] {
                let mut values = base.to_vec();
                values.push((key, value));
                assert!(config(&values).is_ok(), "{key}={value}");
            }
        }
    }

    #[test]
    fn errors_and_debug_redact_secrets() {
        let values = [
            ("YOINK_RUST_TELEGRAM_ENABLED", "true"),
            ("YOINK_RUST_TELEGRAM_TOKEN", "sensitive-token"),
            ("YOINK_RUST_OWNER_ID", "42"),
        ];
        assert!(!format!("{:?}", config(&values).unwrap()).contains("sensitive-token"));
        let error = validate_api_url("https://secret:password@localhost/?token=sensitive-token")
            .unwrap_err();
        let printed = format!("{error:?} {error}");
        assert!(!printed.contains("password"));
        assert!(!printed.contains("sensitive-token"));
    }

    #[test]
    fn inline_example_uses_real_consumer_and_invalid_values_fail_closed() {
        let values: Vec<_> = include_str!("../../../docs/inline-search.env.example")
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| line.split_once('=').unwrap())
            .collect();
        let parsed = config(&values).unwrap();
        let search = parsed.telegram.unwrap().inline_search;
        assert_eq!(search.concurrency, 3);
        assert_eq!(search.timeout, Duration::from_secs(20));
        assert_eq!(search.requests_per_second, 10);
        assert_eq!(search.actor_interval, Duration::from_millis(300));
        for (key, bad) in [
            ("YOINK_RUST_TELEGRAM_INLINE_CONCURRENCY", "0"),
            ("YOINK_RUST_TELEGRAM_INLINE_TIMEOUT_SECS", "121"),
            ("YOINK_RUST_TELEGRAM_INLINE_REQUESTS_PER_SECOND", "101"),
            (
                "YOINK_RUST_TELEGRAM_INLINE_ACTOR_INTERVAL_MS",
                "secret-value",
            ),
        ] {
            let mut invalid = values.clone();
            invalid.retain(|(old, _)| *old != key);
            invalid.push((key, bad));
            let error = config(&invalid).unwrap_err();
            assert!(!format!("{error:?} {error}").contains("secret-value"));
        }
        let mut proxy = values.clone();
        proxy.push((
            "YOINK_RUST_DOWNLOAD_PROXY_URL",
            "http://user:secret@127.0.0.1:8080",
        ));
        let parsed = config(&proxy).unwrap();
        assert!(!format!("{parsed:?}").contains("secret@"));
        proxy.push((
            "YOINK_RUST_YOINK_RUST_DOWNLOAD_PROXY_URL",
            "http://different.example:8080",
        ));
        assert!(
            config(&proxy).is_err(),
            "conflicting operator network authorities must fail closed"
        );
    }

    #[test]
    fn invalid_configuration_fails_closed() {
        for (key, value) in [
            ("HTTP_BIND", "nonsense"),
            ("TELEGRAM_ENABLED", "yes"),
            ("STARTUP_TIMEOUT_SECS", "0"),
            ("SHUTDOWN_TIMEOUT_SECS", "301"),
        ] {
            assert!(config(&[(&format!("{PREFIX}{key}"), value)]).is_err());
        }
        for value in [
            "file:///tmp",
            "https://localhost/api",
            "https://localhost/#secret",
            "https://localhost/?secret=x",
        ] {
            assert!(validate_api_url(value).is_err());
        }
        assert!(validate_api_url("http://127.0.0.1:8082").is_ok());
    }

    #[test]
    fn database_path_is_opt_in() {
        assert!(config(&[]).unwrap().database.is_none());
        let parsed = config(&[("YOINK_RUST_DATABASE_PATH", "/tmp/yoink-test.db")]).unwrap();
        let db = parsed.database.unwrap();
        assert_eq!(db.path, PathBuf::from("/tmp/yoink-test.db"));
        assert_eq!(db.acquire_timeout, Duration::from_secs(5));
        assert_eq!(db.health_interval, Duration::from_secs(5));
    }

    #[test]
    fn legacy_database_configuration_is_rejected_without_echoing_values() {
        for (key, value) in [
            ("DATABASE_URL", "postgresql://user:secret@host/db"),
            ("MAX_CONNECTIONS", "secret-value"),
            (
                "YOINK_RUST_DATABASE_URL",
                "postgresql://user:secret@host/db",
            ),
            ("YOINK_RUST_DATABASE_MAX_CONNECTIONS", "secret-value"),
        ] {
            let error = config(&[(key, value)]).unwrap_err();
            let printed = format!("{error:?} {error}");
            assert!(printed.contains("legacy DATABASE_URL and MAX_CONNECTIONS are unsupported"));
            assert!(!printed.contains("secret"));
        }
    }

    #[test]
    fn invalid_database_configuration_is_rejected() {
        for value in ["", "relative.db", "   "] {
            assert!(config(&[("YOINK_RUST_DATABASE_PATH", value)]).is_err());
        }
        assert!(config(&[("YOINK_RUST_DATABASE_PATH", "/tmp/yoink.db")]).is_ok());
    }
}
