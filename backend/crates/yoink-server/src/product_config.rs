use crate::config::{ConfigError, invalid};
use std::{collections::BTreeMap, fmt, path::PathBuf, time::Duration};
use yoink_music::{MusicCredentials, MusicProvider, MusicSettings};

pub struct MusicConfig {
    pub settings: MusicSettings,
    pub executor: yoink_executor::ExecutorConfig,
    pub observation_max_age: Duration,
    pub response_bytes: usize,
}

impl fmt::Debug for MusicConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MusicConfig([redacted])")
    }
}

#[derive(Debug)]
pub struct StatsConfig {
    pub index_dir: PathBuf,
    pub names: yoink_stats::UsernameRefreshSchedule,
    pub search_interval: Duration,
    pub text_timeout: Duration,
    pub response_bytes: usize,
    pub import_root: PathBuf,
    pub import_max_bytes: usize,
    pub import_max_messages: usize,
    pub import_max_jobs: usize,
    pub import_timeout: Duration,
}

type Read<'a> = dyn FnMut(&str) -> Result<Option<String>, ConfigError> + 'a;

fn enabled(
    read: &mut Read<'_>,
    key: &str,
    database: bool,
    telegram: bool,
) -> Result<bool, ConfigError> {
    let enabled = boolean(read, key, false)?;
    if enabled && (!database || !telegram) {
        return Err(invalid(
            key,
            "requires explicit database and Telegram configuration",
        ));
    }
    Ok(enabled)
}

fn boolean(read: &mut Read<'_>, key: &str, default: bool) -> Result<bool, ConfigError> {
    match read(key)?.as_deref() {
        None => Ok(default),
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        _ => Err(invalid(key, "expected true or false")),
    }
}

fn number(
    read: &mut Read<'_>,
    key: &str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, ConfigError> {
    let value = read(key)?
        .map(|v| v.parse::<u64>())
        .transpose()
        .map_err(|_| invalid(key, "expected a bounded integer"))?
        .unwrap_or(default);
    if !(min..=max).contains(&value) {
        return Err(invalid(key, "integer is outside the supported range"));
    }
    Ok(value)
}

fn seconds(
    read: &mut Read<'_>,
    key: &str,
    default: u64,
    max: u64,
) -> Result<Duration, ConfigError> {
    Ok(Duration::from_secs(number(read, key, default, 1, max)?))
}

fn directory(read: &mut Read<'_>, key: &str, default: &str) -> Result<PathBuf, ConfigError> {
    let path = PathBuf::from(read(key)?.unwrap_or_else(|| default.into()));
    if !path.is_absolute()
        || path.parent().is_none()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(invalid(
            key,
            "expected an absolute dedicated directory without parent traversal",
        ));
    }
    Ok(path)
}

fn secret(read: &mut Read<'_>, key: &str) -> Result<Option<String>, ConfigError> {
    let value = read(key)?;
    if value
        .as_ref()
        .is_some_and(|s| s.is_empty() || s.len() > 4096 || s.chars().any(char::is_control))
    {
        return Err(invalid(key, "expected nonempty bounded credential text"));
    }
    Ok(value)
}

impl MusicConfig {
    pub fn from_lookup(
        read: &mut Read<'_>,
        database: bool,
        telegram: bool,
    ) -> Result<Option<Self>, ConfigError> {
        if !enabled(read, "MUSIC_ENABLED", database, telegram)? {
            return Ok(None);
        }
        let proxy = secret(read, "MUSIC_PROXY_URL")?;
        let mut proxies = BTreeMap::new();
        let platforms = read("MUSIC_PROXY_PLATFORMS")?.unwrap_or_default();
        if platforms.len() > 256 {
            return Err(invalid("MUSIC_PROXY_PLATFORMS", "too many platform keys"));
        }
        for key in platforms
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let provider = MusicProvider::ALL
                .into_iter()
                .find(|p| p.key() == key)
                .ok_or_else(|| invalid("MUSIC_PROXY_PLATFORMS", "unknown platform key"))?;
            proxies.insert(
                provider,
                proxy
                    .clone()
                    .ok_or_else(|| invalid("MUSIC_PROXY_URL", "required for proxy platforms"))?,
            );
        }
        let settings = MusicSettings {
            request_timeout: seconds(read, "MUSIC_REQUEST_TIMEOUT_SECS", 10, 120)?,
            lookup_timeout: seconds(read, "MUSIC_LOOKUP_TIMEOUT_SECS", 30, 120)?,
            resolve_timeout: seconds(read, "MUSIC_RESOLVE_TIMEOUT_SECS", 120, 3600)?,
            audio_timeout: seconds(read, "MUSIC_AUDIO_TIMEOUT_SECS", 120, 3600)?,
            delivery_timeout: seconds(read, "MUSIC_DELIVERY_TIMEOUT_SECS", 30, 60)?,
            persistence_timeout: seconds(read, "MUSIC_PERSISTENCE_TIMEOUT_SECS", 10, 120)?,
            cache_ttl: seconds(read, "MUSIC_CACHE_TTL_SECS", 3600, 86400)?,
            cache_entries: number(read, "MUSIC_CACHE_ENTRIES", 1000, 1, 10000)? as usize,
            worker_concurrency: number(read, "MUSIC_WORKER_CONCURRENCY", 2, 1, 32)? as usize,
            response_max_bytes: number(
                read,
                "MUSIC_WORKER_RESPONSE_BYTES",
                1024 * 1024,
                1024,
                8388608,
            )? as u32,
            max_requests: number(read, "MUSIC_MAX_REQUESTS", 30, 1, 100)? as u16,
            audio_max_bytes: number(
                read,
                "MUSIC_AUDIO_MAX_BYTES",
                49 * 1024 * 1024,
                1,
                49 * 1024 * 1024,
            )?,
            max_links_per_command: number(read, "MUSIC_MAX_LINKS", 10, 1, 10)? as usize,
            download_enabled: boolean(read, "MUSIC_DOWNLOAD_ENABLED", false)?,
            credentials: MusicCredentials {
                spotify_client_id: secret(read, "MUSIC_SPOTIFY_CLIENT_ID")?,
                spotify_client_secret: secret(read, "MUSIC_SPOTIFY_CLIENT_SECRET")?,
                tidal_client_id: secret(read, "MUSIC_TIDAL_CLIENT_ID")?,
                tidal_client_secret: secret(read, "MUSIC_TIDAL_CLIENT_SECRET")?,
            },
            proxies,
        };
        settings
            .validate()
            .map_err(|_| invalid("MUSIC_ENABLED", "invalid bounded Music worker settings"))?;
        let workspace_root = directory(read, "MUSIC_WORKSPACE", "/tmp/yoink-music")?;
        let timeout = settings.lookup_timeout.max(settings.audio_timeout);
        let executor = yoink_executor::ExecutorConfig {
            program: "/usr/local/bin/yoink-worker-launcher".into(),
            args: vec![],
            workspace_root,
            timeout,
            processing_timeout: timeout,
            socket_timeout: seconds(read, "MUSIC_WORKER_SOCKET_TIMEOUT_SECS", 15, 120)?,
            max_artifact_bytes: settings.audio_max_bytes,
            max_workspace_bytes: number(
                read,
                "MUSIC_WORKSPACE_MAX_BYTES",
                200 * 1024 * 1024,
                settings.audio_max_bytes,
                1024 * 1024 * 1024,
            )?,
            max_output_bytes: number(
                read,
                "MUSIC_WORKER_OUTPUT_BYTES",
                1024 * 1024,
                65536,
                4 * 1024 * 1024,
            )? as usize,
            max_artifacts: 1,
            allowed_proxy_origins: origins(read, "MUSIC_WORKER_ALLOWED_PROXY_ORIGINS")?,
            allowed_private_origins: origins(read, "MUSIC_WORKER_ALLOWED_PRIVATE_ORIGINS")?,
        };
        Ok(Some(Self {
            settings,
            executor,
            observation_max_age: seconds(read, "MUSIC_OBSERVATION_MAX_AGE_SECS", 300, 86400)?,
            response_bytes: number(
                read,
                "MUSIC_DELIVERY_RESPONSE_BYTES",
                65536,
                1024,
                64 * 1024,
            )? as usize,
        }))
    }
}

fn origins(read: &mut Read<'_>, key: &str) -> Result<Vec<String>, ConfigError> {
    let values: Vec<String> = serde_json::from_str(&read(key)?.unwrap_or_else(|| "[]".into()))
        .map_err(|_| invalid(key, "expected a bounded JSON array of origins"))?;
    if values.len() > 16 {
        return Err(invalid(key, "at most sixteen origins are allowed"));
    }
    for value in &values {
        let url = url::Url::parse(value).map_err(|_| invalid(key, "invalid origin"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || value.len() > 2048
            || value.contains('@')
            || url.port() == Some(0)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || value.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(invalid(
                key,
                "expected an HTTP(S) origin without credentials, path, query or fragment",
            ));
        }
    }
    Ok(values)
}

impl StatsConfig {
    pub fn from_lookup(
        read: &mut Read<'_>,
        database: bool,
        telegram: bool,
    ) -> Result<Option<Self>, ConfigError> {
        if !enabled(read, "STATS_ENABLED", database, telegram)? {
            return Ok(None);
        }
        Ok(Some(Self {
            index_dir: directory(read, "STATS_INDEX_DIR", "/var/lib/yoink/stats-search")?,
            names: yoink_stats::UsernameRefreshSchedule {
                first_delay: seconds(read, "STATS_USERNAME_FIRST_DELAY_SECS", 60, 86400)?,
                interval: seconds(read, "STATS_REFRESH_INTERVAL_SECS", 3600, 86400)?,
                lookup_timeout: seconds(read, "STATS_LOOKUP_TIMEOUT_SECS", 10, 120)?,
            },
            search_interval: seconds(read, "STATS_SEARCH_REFRESH_INTERVAL_SECS", 3600, 86400)?,
            text_timeout: seconds(read, "STATS_TEXT_TIMEOUT_SECS", 30, 60)?,
            response_bytes: number(
                read,
                "STATS_LOOKUP_RESPONSE_BYTES",
                65536,
                1024,
                1024 * 1024,
            )? as usize,
            import_root: directory(read, "STATS_IMPORT_ROOT", "/var/lib/yoink/stats-import")?,
            import_max_bytes: number(
                read,
                "STATS_IMPORT_MAX_BYTES",
                16 * 1024 * 1024,
                1024,
                16 * 1024 * 1024,
            )? as usize,
            import_max_messages: number(read, "STATS_IMPORT_MAX_MESSAGES", 100000, 1, 100000)?
                as usize,
            import_max_jobs: number(read, "STATS_IMPORT_MAX_JOBS", 16, 1, 128)? as usize,
            import_timeout: seconds(read, "STATS_IMPORT_TIMEOUT_SECS", 120, 3600)?,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn read<'a>(
        entries: &'a [(&str, &str)],
    ) -> impl FnMut(&str) -> Result<Option<String>, ConfigError> + 'a {
        move |key| {
            Ok(entries
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string()))
        }
    }
    #[test]
    fn products_are_opt_in_and_require_native_dependencies() {
        assert!(
            MusicConfig::from_lookup(&mut read(&[]), false, false)
                .unwrap()
                .is_none()
        );
        assert!(
            StatsConfig::from_lookup(&mut read(&[]), false, false)
                .unwrap()
                .is_none()
        );
        for (db, tg) in [(false, false), (true, false), (false, true)] {
            assert!(
                MusicConfig::from_lookup(&mut read(&[("MUSIC_ENABLED", "true")]), db, tg).is_err()
            );
            assert!(
                StatsConfig::from_lookup(&mut read(&[("STATS_ENABLED", "true")]), db, tg).is_err()
            );
        }
    }
    #[test]
    fn legacy_active_defaults_and_credentials_are_preserved_and_redacted() {
        let config = MusicConfig::from_lookup(
            &mut read(&[
                ("MUSIC_ENABLED", "true"),
                ("MUSIC_SPOTIFY_CLIENT_SECRET", "fixture-secret"),
            ]),
            true,
            true,
        )
        .unwrap()
        .unwrap();
        assert_eq!(config.settings.request_timeout.as_secs(), 10);
        assert_eq!(config.settings.cache_ttl.as_secs(), 3600);
        assert!(!config.settings.download_enabled);
        assert!(!format!("{config:?}").contains("fixture-secret"));
        let stats = StatsConfig::from_lookup(&mut read(&[("STATS_ENABLED", "true")]), true, true)
            .unwrap()
            .unwrap();
        assert_eq!(stats.names.interval.as_secs(), 3600);
        assert_eq!(stats.names.first_delay.as_secs(), 60);
        yoink_http::stats_import::ImportConfig {
            root: stats.import_root,
            max_bytes: stats.import_max_bytes,
            max_messages: stats.import_max_messages,
            max_jobs: stats.import_max_jobs,
            timeout: stats.import_timeout,
        }
        .validate()
        .unwrap();
    }
    #[test]
    fn unsafe_paths_unbounded_limits_and_proxy_errors_never_echo_input() {
        for (key, value) in [
            ("MUSIC_WORKSPACE", "/"),
            ("MUSIC_WORKSPACE", "/tmp/../secret"),
            ("MUSIC_MAX_LINKS", "11"),
            ("MUSIC_AUDIO_TIMEOUT_SECS", "0"),
            ("MUSIC_PROXY_URL", "https://secret/path"),
        ] {
            let entries = [
                ("MUSIC_ENABLED", "true"),
                (key, value),
                ("MUSIC_PROXY_PLATFORMS", "spotify"),
            ];
            let error = MusicConfig::from_lookup(&mut read(&entries), true, true).unwrap_err();
            assert!(!error.to_string().contains(value));
        }
        for (key, value) in [
            ("STATS_INDEX_DIR", "relative"),
            ("STATS_IMPORT_ROOT", "/"),
            ("STATS_IMPORT_MAX_JOBS", "0"),
            ("STATS_IMPORT_MAX_BYTES", "16777217"),
            ("STATS_IMPORT_MAX_MESSAGES", "100001"),
            ("STATS_LOOKUP_TIMEOUT_SECS", "121"),
        ] {
            assert!(
                StatsConfig::from_lookup(
                    &mut read(&[("STATS_ENABLED", "true"), (key, value)]),
                    true,
                    true
                )
                .is_err()
            );
        }
    }
}
