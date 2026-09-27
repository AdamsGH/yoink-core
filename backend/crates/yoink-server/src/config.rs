use std::{fmt, net::SocketAddr, time::Duration};

use thiserror::Error;
use url::Url;

const PREFIX: &str = "YOINK_RUST_";

/// Never prints its contents, including through the parent configuration's Debug.
pub struct SecretToken(String);

impl SecretToken {
    pub fn into_inner(self) -> String {
        self.0
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

#[derive(Debug)]
pub struct DatabaseConfig {
    pub url: SecretToken,
    pub max_connections: u32,
    pub acquire_timeout: Duration,
    pub health_interval: Duration,
}

impl DatabaseConfig {
    pub async fn connect(&self) -> Result<yoink_storage::Store, yoink_storage::StoreError> {
        yoink_storage::Store::connect(
            self.url.as_str(),
            self.max_connections,
            self.acquire_timeout,
        )
        .await
    }
}

#[derive(Debug)]
pub struct TelegramConfig {
    pub token: SecretToken,
    pub owner_id: u64,
    pub api_url: Option<Url>,
    pub poll_timeout: Duration,
}

#[derive(Debug)]
pub struct Config {
    pub http_bind: SocketAddr,
    pub telegram: Option<TelegramConfig>,
    pub startup_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub database: Option<DatabaseConfig>,
}

#[derive(Debug, Error)]
#[error("invalid configuration {key}: {reason}")]
pub struct ConfigError {
    key: String,
    reason: &'static str,
}

fn invalid(key: &str, reason: &'static str) -> ConfigError {
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
        let mut read = |key: &str| lookup(&format!("{PREFIX}{key}"));
        let http_bind = read("HTTP_BIND")?
            .unwrap_or_else(|| "127.0.0.1:8088".into())
            .parse()
            .map_err(|_| invalid("HTTP_BIND", "expected an IP address and port"))?;
        let startup_timeout = timeout(read("STARTUP_TIMEOUT_SECS")?, "STARTUP_TIMEOUT_SECS", 15)?;
        let shutdown_timeout =
            timeout(read("SHUTDOWN_TIMEOUT_SECS")?, "SHUTDOWN_TIMEOUT_SECS", 10)?;
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
            Some(TelegramConfig {
                token: SecretToken(token),
                owner_id,
                api_url,
                poll_timeout,
            })
        } else {
            None
        };
        let database = if let Some(value) = read("DATABASE_URL")? {
            let url = Url::parse(&value).map_err(|_| {
                invalid(
                    "DATABASE_URL",
                    "expected a PostgreSQL URL with host and database",
                )
            })?;
            if !matches!(url.scheme(), "postgres" | "postgresql")
                || url.host_str().is_none()
                || url.path().trim_matches('/').is_empty()
                || url.fragment().is_some()
                || value.chars().any(char::is_whitespace)
            {
                return Err(invalid(
                    "DATABASE_URL",
                    "expected a PostgreSQL URL with host and database",
                ));
            }
            let max_connections = read("DATABASE_MAX_CONNECTIONS")?
                .unwrap_or_else(|| "5".into())
                .parse::<u32>()
                .map_err(|_| {
                    invalid(
                        "DATABASE_MAX_CONNECTIONS",
                        "expected an integer in range 1..=32",
                    )
                })?;
            if !(1..=32).contains(&max_connections) {
                return Err(invalid(
                    "DATABASE_MAX_CONNECTIONS",
                    "expected an integer in range 1..=32",
                ));
            }
            Some(DatabaseConfig {
                url: SecretToken(value),
                max_connections,
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
        Ok(Self {
            http_bind,
            telegram,
            startup_timeout,
            shutdown_timeout,
            database,
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
    fn database_is_opt_in_and_redacted() {
        assert!(
            config(&[("database_url", "postgresql://legacy:secret@localhost/live")])
                .unwrap()
                .database
                .is_none()
        );
        let parsed = config(&[(
            "YOINK_RUST_DATABASE_URL",
            "postgresql://user:db-sensitive@localhost/isolated",
        )])
        .unwrap();
        assert!(!format!("{parsed:?}").contains("db-sensitive"));
        let db = parsed.database.unwrap();
        assert_eq!(db.max_connections, 5);
        assert_eq!(db.acquire_timeout, Duration::from_secs(5));
    }

    #[test]
    fn invalid_database_configuration_is_rejected() {
        for value in [
            "",
            "sqlite://localhost/test",
            "postgresql://localhost/",
            "postgresql://localhost/db#secret",
            "postgresql://localhost/db name",
        ] {
            assert!(config(&[("YOINK_RUST_DATABASE_URL", value)]).is_err());
        }
        for value in ["0", "33", "invalid"] {
            assert!(
                config(&[
                    ("YOINK_RUST_DATABASE_URL", "postgresql://localhost/test"),
                    ("YOINK_RUST_DATABASE_MAX_CONNECTIONS", value)
                ])
                .is_err()
            );
        }
    }
}
