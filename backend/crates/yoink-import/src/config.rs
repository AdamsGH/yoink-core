use std::{env, path::PathBuf, time::Duration};

use crate::{ImportError, Result};

#[derive(Clone)]
pub struct ImportConfig {
    pub destination: PathBuf,
    pub source_url: String,
    pub ca_cert: Option<PathBuf>,
    pub allow_loopback_plaintext: bool,
    pub connect_timeout: Duration,
    pub lock_timeout: Duration,
    pub statement_timeout: Duration,
    pub total_timeout: Duration,
    pub max_row_bytes: usize,
}

impl std::fmt::Debug for ImportConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportConfig")
            .field("destination", &self.destination)
            .field("source_url", &"[REDACTED]")
            .field("ca_cert", &self.ca_cert)
            .field("allow_loopback_plaintext", &self.allow_loopback_plaintext)
            .field("connect_timeout", &self.connect_timeout)
            .field("lock_timeout", &self.lock_timeout)
            .field("statement_timeout", &self.statement_timeout)
            .field("total_timeout", &self.total_timeout)
            .field("max_row_bytes", &self.max_row_bytes)
            .finish()
    }
}

impl ImportConfig {
    pub fn new(destination: PathBuf, source_url: String) -> Self {
        Self {
            destination,
            source_url,
            ca_cert: None,
            allow_loopback_plaintext: false,
            connect_timeout: Duration::from_secs(10),
            lock_timeout: Duration::from_secs(15),
            statement_timeout: Duration::from_secs(60),
            total_timeout: Duration::from_secs(3600),
            max_row_bytes: 16 * 1024 * 1024,
        }
    }

    pub fn validate(&self) -> Result<()> {
        let bounded =
            |d: Duration, max| d >= Duration::from_millis(1) && d <= Duration::from_secs(max);
        if self.destination.as_os_str().is_empty()
            || self.source_url.trim().is_empty()
            || (self.ca_cert.is_some() && self.allow_loopback_plaintext)
            || !bounded(self.connect_timeout, 300)
            || !bounded(self.lock_timeout, 300)
            || !bounded(self.statement_timeout, 3600)
            || !bounded(self.total_timeout, 86_400)
            || self.max_row_bytes == 0
            || self.max_row_bytes > 256 * 1024 * 1024
        {
            return Err(ImportError::Configuration);
        }
        Ok(())
    }

    /// Read a DSN only from the explicitly named environment variable.
    pub fn from_cli_args(args: impl IntoIterator<Item = String>) -> Result<Self> {
        Self::from_args_and_source(args, env::var("YOINK_IMPORT_SOURCE_URL").ok())
    }

    pub fn from_args_and_source(
        args: impl IntoIterator<Item = String>,
        source: Option<String>,
    ) -> Result<Self> {
        let mut args = args.into_iter();
        let mut seen = std::collections::BTreeSet::new();
        let mut config = Self::new(PathBuf::new(), source.unwrap_or_default());
        while let Some(arg) = args.next() {
            if !seen.insert(arg.clone()) {
                return Err(ImportError::Configuration);
            }
            if arg == "--allow-loopback-plaintext" {
                config.allow_loopback_plaintext = true;
                continue;
            }
            let value = args
                .next()
                .filter(|value| !value.is_empty() && !value.starts_with("--"))
                .ok_or(ImportError::Configuration)?;
            let seconds = || {
                value
                    .parse::<u64>()
                    .map(Duration::from_secs)
                    .map_err(|_| ImportError::Configuration)
            };
            match arg.as_str() {
                "--destination" => config.destination = value.into(),
                "--ca-cert" => config.ca_cert = Some(value.into()),
                "--connect-timeout-seconds" => config.connect_timeout = seconds()?,
                "--lock-timeout-seconds" => config.lock_timeout = seconds()?,
                "--statement-timeout-seconds" => config.statement_timeout = seconds()?,
                "--total-timeout-seconds" => config.total_timeout = seconds()?,
                "--max-row-bytes" => {
                    config.max_row_bytes = value.parse().map_err(|_| ImportError::Configuration)?
                }
                _ => return Err(ImportError::Configuration),
            }
        }
        if config.source_url.trim().is_empty() {
            return Err(ImportError::MissingSource);
        }
        if config.ca_cert.is_some() && config.allow_loopback_plaintext {
            return Err(ImportError::Configuration);
        }
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_contains_source_credentials() {
        let c = ImportConfig::new(
            "target.db".into(),
            "postgres://alice:secret@example.test/db".into(),
        );
        let rendered = format!("{c:?}");
        assert!(!rendered.contains("alice"));
        assert!(!rendered.contains("secret"));
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn rejects_invalid_bounds() {
        let mut c = ImportConfig::new("fresh.db".into(), "postgres://secret".into());
        c.max_row_bytes = 0;
        assert!(c.validate().is_err());
        c.max_row_bytes = 1;
        c.total_timeout = Duration::ZERO;
        assert!(c.validate().is_err());
        c.total_timeout = Duration::from_secs(10);
        c.lock_timeout = Duration::from_nanos(1);
        assert!(
            c.validate().is_err(),
            "sub-millisecond timeout must not become PostgreSQL's unlimited 0ms"
        );
        c.lock_timeout = Duration::from_millis(1);
        c.ca_cert = Some("ca.pem".into());
        c.allow_loopback_plaintext = true;
        assert!(
            c.validate().is_err(),
            "library configuration must enforce the same TLS policy as CLI"
        );
    }

    #[test]
    fn cli_rejects_missing_values_duplicates_and_unknown_flags() {
        for args in [
            vec!["--destination"],
            vec!["--destination", "fresh.db", "--ca-cert"],
            vec!["--destination", "fresh.db", "--destination", "second.db"],
            vec!["--destination", "fresh.db", "--unknown", "value"],
            vec!["--destination", "fresh.db", "--total-timeout-seconds", "0"],
        ] {
            assert!(
                ImportConfig::from_args_and_source(
                    args.into_iter().map(str::to_owned),
                    Some("postgres://explicit".into())
                )
                .is_err()
            );
        }
        let config = ImportConfig::from_args_and_source(
            [
                "--destination",
                "fresh.db",
                "--total-timeout-seconds",
                "120",
            ]
            .map(str::to_owned),
            Some("postgres://explicit".into()),
        )
        .unwrap();
        assert_eq!(config.total_timeout, Duration::from_secs(120));
        assert!(
            ImportConfig::from_args_and_source(
                ["--destination", "fresh.db"].map(str::to_owned),
                None
            )
            .is_err()
        );
    }
}
