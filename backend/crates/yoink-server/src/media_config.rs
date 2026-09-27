use std::{fmt, path::PathBuf, time::Duration};

use yoink_downloads::{DownloadConfig, DownloadNetworkConfig};
use yoink_executor::ExecutorConfig;
use yoink_storage::AdmissionPolicy;

use crate::config::{ConfigError, invalid};

pub struct MediaConfig {
    pub executor: ExecutorConfig,
    pub downloads: DownloadConfig,
    pub network: DownloadNetworkConfig,
}

impl fmt::Debug for MediaConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaConfig")
            .field("downloads", &self.downloads)
            .field("executor", &"confined worker launcher")
            .field("network", &self.network)
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
            50 * 1024 * 1024,
        )?;
        let max_workspace_bytes = number(
            read,
            "DOWNLOAD_WORKSPACE_MAX_BYTES",
            100 * 1024 * 1024,
            max_artifact_bytes,
            200 * 1024 * 1024,
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
        let system_proxy = read("YOINK_RUST_DOWNLOAD_PROXY_URL")?;
        let no_cookie_domains: Vec<String> = serde_json::from_str(
            &read("YOINK_RUST_DOWNLOAD_NO_COOKIE_DOMAINS_JSON")?.unwrap_or_else(|| "[]".into()),
        )
        .map_err(|_| {
            invalid(
                "YOINK_RUST_DOWNLOAD_NO_COOKIE_DOMAINS_JSON",
                "expected a bounded JSON array of hostnames",
            )
        })?;
        if no_cookie_domains.len() > 64 {
            return Err(invalid(
                "YOINK_RUST_DOWNLOAD_NO_COOKIE_DOMAINS_JSON",
                "at most sixty-four domains are allowed",
            ));
        }
        let network =
            DownloadNetworkConfig::new(system_proxy, no_cookie_domains).map_err(|_| {
                invalid(
                    "YOINK_RUST_DOWNLOAD_PROXY_URL",
                    "proxy or no-cookie domain configuration is invalid",
                )
            })?;
        Ok(Self {
            network,
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
            },
        })
    }
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
            Duration::from_secs(60),
        )
    }

    #[test]
    fn bounded_defaults_and_fixed_worker_program() {
        let config = parse(&[]).unwrap();
        assert_eq!(config.downloads.concurrency, 2);
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
            vec![("DOWNLOAD_MAX_BYTES", "52428801")],
            vec![("DOWNLOAD_WORKSPACE", "/")],
            vec![("DOWNLOAD_WORKSPACE", "relative")],
            vec![("DOWNLOAD_LEASE_SECS", "10")],
            vec![("DOWNLOAD_HEARTBEAT_SECS", "21")],
            vec![("WORKER_PROCESSING_TIMEOUT_SECS", "121")],
            vec![("WORKER_PROCESSING_TIMEOUT_SECS", "0")],
            vec![("DOWNLOAD_MAX_ITEMS", "101")],
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
