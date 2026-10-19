//! Legacy preferences, versioned credentials and pre-admission OAuth refresh.
pub mod oauth;
pub mod oauth_refresh;
use std::fmt;
use yoink_db::Transaction;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use turso::transaction::TransactionBehavior;
use url::Url;
use yoink_access::{FeatureSpec, Role, effective_grant};
use yoink_db::{Database, query};
use yoink_domain::{
    AudioCodec, Container, CookieReference, CredentialVersion, DownloadParams,
    DownloadPreferenceOverrides, MediaMode, NetworkSelection, ProxyReference, ProxySource,
    SubtitleOptions, VideoCodec,
};
use yoink_identity::IdentitySnapshot;
use yoink_worker_protocol::{MAX_COOKIE_JAR_BYTES, NetworkCredentials};

/// Settings bound shared by validation and persisted preference reads.
/// The existing application schema caps proxy_url at 512 characters; a byte
/// bound also keeps non-ASCII input within that cap without a schema migration.
pub const MAX_PROXY_URL_BYTES: usize = 512;

const MAX_CANDIDATES: i64 = 128;
const MAX_ARGS_JSON_BYTES: usize = 64 * 1024;

pub struct NetworkContext<'a> {
    pub actor_id: u64,
    pub domain: &'a str,
    pub no_cookie_domains: &'a [String],
    pub identity: &'a IdentitySnapshot,
    pub system_proxy_domains: &'a [String],
    pub system_proxy: Option<&'a str>,
}

#[derive(Clone)]
pub struct ProfileStore {
    database: Database,
    oauth_refresh: Option<std::sync::Arc<dyn oauth_refresh::OAuthRefreshProvider>>,
    oauth_refresh_timeout: std::time::Duration,
}

#[derive(Clone, Debug)]
pub(crate) struct FormatPreferences {
    pub(crate) quality: Option<String>,
    pub(crate) codec: Option<String>,
    pub(crate) container: Option<String>,
    pub(crate) send_as_file: bool,
    pub(crate) audio_codec: Option<String>,
    pub(crate) gallery_zip: bool,
    pub(crate) subtitle_language: Option<String>,
    pub(crate) subtitle_automatic: bool,
}

#[derive(Clone, PartialEq)]
pub struct DownloadPreferences {
    pub quality: String,
    pub codec: String,
    pub container: String,
    pub proxy_enabled: bool,
    pub proxy_url: Option<String>,
    pub keyboard: String,
    pub subs_enabled: bool,
    pub subs_auto: bool,
    pub subs_always_ask: bool,
    pub subs_lang: String,
    pub split_size: i64,
    pub nsfw_blur: bool,
    pub mediainfo: bool,
    pub send_as_file: bool,
    pub audio_codec: String,
    pub gallery_zip: bool,
    pub use_pool_cookies: bool,
    pub youtube_auth_mode: String,
    pub dm_topic_thread_id: Option<i64>,
    pub args_json: Value,
    pub updated_at: Option<DateTime<Utc>>,
}
impl fmt::Debug for DownloadPreferences {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadPreferences")
            .field("quality", &self.quality)
            .field("codec", &self.codec)
            .field("container", &self.container)
            .field("proxy_enabled", &self.proxy_enabled)
            .field("proxy_url", &self.proxy_url.as_ref().map(|_| "[REDACTED]"))
            .field("keyboard", &self.keyboard)
            .field("subs_enabled", &self.subs_enabled)
            .field("subs_auto", &self.subs_auto)
            .field("subs_always_ask", &self.subs_always_ask)
            .field("subs_lang", &self.subs_lang)
            .field("split_size", &self.split_size)
            .field("nsfw_blur", &self.nsfw_blur)
            .field("mediainfo", &self.mediainfo)
            .field("send_as_file", &self.send_as_file)
            .field("audio_codec", &self.audio_codec)
            .field("gallery_zip", &self.gallery_zip)
            .field("use_pool_cookies", &self.use_pool_cookies)
            .field("youtube_auth_mode", &self.youtube_auth_mode)
            .field("dm_topic_thread_id", &self.dm_topic_thread_id)
            .field("args_json", &"[REDACTED]")
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ProfileError {
    #[error("profile or identity is unavailable")]
    Unavailable,
    #[error("network credential reference is no longer valid")]
    StaleReference,
    #[error("shared cookie access is not authorized")]
    SharedAccessDenied,
    #[error("proxy is enabled but no proxy is configured")]
    ProxyUnavailable,
    #[error("proxy URL is invalid")]
    InvalidProxy,
    #[error("cookie credential is malformed or exceeds limits")]
    InvalidCookie,
    #[error("cookie credential uses unsupported OAuth authentication")]
    UnsupportedAuthentication,
    #[error("database profile read failed")]
    Database,
    #[error("download preferences contain invalid values")]
    InvalidPreferences,
    #[error("cookie candidate set exceeds the supported capacity")]
    CandidateCapacityExceeded,
}
impl From<yoink_db::Error> for ProfileError {
    fn from(_: yoink_db::Error) -> Self {
        Self::Database
    }
}
impl From<turso::Error> for ProfileError {
    fn from(_: turso::Error) -> Self {
        Self::Database
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Candidate {
    id: i32,
    owner_id: i64,
    domain: String,
    is_pool: bool,
}

impl ProfileStore {
    pub fn new(database: Database) -> Self {
        Self {
            database,
            oauth_refresh: None,
            oauth_refresh_timeout: std::time::Duration::from_secs(15),
        }
    }

    /// Checks required native tables and columns with zero-row reads.
    pub async fn check_schema(&self) -> Result<(), ProfileError> {
        for sql in [
            include_str!("schema_users.sql"),
            include_str!("schema_preferences.sql"),
            include_str!("schema_cookies.sql"),
        ] {
            query(sql).execute(&self.database).await?;
        }
        Ok(())
    }

    pub(crate) async fn format_preferences(
        &self,
        actor_id: u64,
    ) -> Result<Option<FormatPreferences>, ProfileError> {
        let id = valid_actor(actor_id)?;
        let row = query(include_str!("format_preferences.sql"))
            .bind(id)
            .fetch_optional(&self.database)
            .await?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(FormatPreferences {
            quality: row.try_get("quality").map_err(|_| ProfileError::Database)?,
            codec: row.try_get("codec").map_err(|_| ProfileError::Database)?,
            container: row
                .try_get("container")
                .map_err(|_| ProfileError::Database)?,
            send_as_file: row
                .try_get("send_as_file")
                .map_err(|_| ProfileError::Database)?,
            audio_codec: row
                .try_get("audio_codec")
                .map_err(|_| ProfileError::Database)?,
            gallery_zip: row
                .try_get("gallery_zip")
                .map_err(|_| ProfileError::Database)?,
            subtitle_language: row
                .try_get("subtitle_language")
                .map_err(|_| ProfileError::Database)?,
            subtitle_automatic: row
                .try_get("subtitle_automatic")
                .map_err(|_| ProfileError::Database)?,
        }))
    }

    pub async fn preferences(
        &self,
        actor_id: u64,
    ) -> Result<Option<DownloadPreferences>, ProfileError> {
        let id = valid_actor(actor_id)?;
        let row = query(include_str!("preferences.sql"))
            .bind(id)
            .fetch_optional(&self.database)
            .await?;
        let Some(row) = row else { return Ok(None) };
        if row
            .try_get::<bool, _>("args_json_too_large")
            .map_err(|_| ProfileError::Database)?
        {
            return Err(ProfileError::InvalidPreferences);
        }
        let preferences = DownloadPreferences {
            quality: row.try_get("quality").map_err(|_| ProfileError::Database)?,
            codec: row.try_get("codec").map_err(|_| ProfileError::Database)?,
            container: row
                .try_get("container")
                .map_err(|_| ProfileError::Database)?,
            proxy_enabled: row
                .try_get("proxy_enabled")
                .map_err(|_| ProfileError::Database)?,
            proxy_url: row
                .try_get("proxy_url")
                .map_err(|_| ProfileError::Database)?,
            keyboard: row
                .try_get("keyboard")
                .map_err(|_| ProfileError::Database)?,
            subs_enabled: row
                .try_get("subs_enabled")
                .map_err(|_| ProfileError::Database)?,
            subs_auto: row
                .try_get("subs_auto")
                .map_err(|_| ProfileError::Database)?,
            subs_always_ask: row
                .try_get("subs_always_ask")
                .map_err(|_| ProfileError::Database)?,
            subs_lang: row
                .try_get("subs_lang")
                .map_err(|_| ProfileError::Database)?,
            split_size: row
                .try_get("split_size")
                .map_err(|_| ProfileError::Database)?,
            nsfw_blur: row
                .try_get("nsfw_blur")
                .map_err(|_| ProfileError::Database)?,
            mediainfo: row
                .try_get("mediainfo")
                .map_err(|_| ProfileError::Database)?,
            send_as_file: row
                .try_get("send_as_file")
                .map_err(|_| ProfileError::Database)?,
            audio_codec: row
                .try_get("audio_codec")
                .map_err(|_| ProfileError::Database)?,
            gallery_zip: row
                .try_get("gallery_zip")
                .map_err(|_| ProfileError::Database)?,
            use_pool_cookies: row
                .try_get("use_pool_cookies")
                .map_err(|_| ProfileError::Database)?,
            youtube_auth_mode: row
                .try_get("youtube_auth_mode")
                .map_err(|_| ProfileError::Database)?,
            dm_topic_thread_id: row
                .try_get("dm_topic_thread_id")
                .map_err(|_| ProfileError::Database)?,
            args_json: row
                .try_get("args_json")
                .map_err(|_| ProfileError::Database)?,
            updated_at: row
                .try_get("updated_at")
                .map_err(|_| ProfileError::Database)?,
        };
        validate_preferences(&preferences)?;
        Ok(Some(preferences))
    }

    /// Freeze current credential versions. No secret content is returned from this method.
    pub async fn prepare_network(
        &self,
        context: NetworkContext<'_>,
        selection_seed: u64,
    ) -> Result<NetworkSelection, ProfileError> {
        self.refresh_before_admission(&context).await?;
        let NetworkContext {
            actor_id,
            domain: requested_domain,
            no_cookie_domains,
            identity,
            system_proxy,
            system_proxy_domains,
        } = context;
        let actor = valid_actor(actor_id)?;
        validate_identity(identity, actor)?;
        let target = normalize_domain(requested_domain)?;
        let mut connection = self.database.connection().await?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Deferred).await?;
        let settings = query(include_str!("network_settings.sql"))
            .bind(actor)
            .fetch_optional(&*tx)
            .await?;
        let (proxy_enabled, personal_proxy, use_pool, youtube_auth_mode) = match settings {
            Some(r) => (
                r.try_get::<bool, _>("proxy_enabled")
                    .map_err(|_| ProfileError::Database)?,
                r.try_get::<Option<String>, _>("proxy_url")
                    .map_err(|_| ProfileError::Database)?,
                r.try_get::<bool, _>("use_pool_cookies")
                    .map_err(|_| ProfileError::Database)?,
                r.try_get::<String, _>("youtube_auth_mode")
                    .map_err(|_| ProfileError::Database)?,
            ),
            None => (false, None, true, "cookies".to_owned()),
        };
        validate_auth_mode(&youtube_auth_mode)?;
        let automatic_proxy =
            system_proxy.is_some() && matches_no_cookie_domain(&target, system_proxy_domains)?;
        let proxy_ref = if proxy_enabled || automatic_proxy {
            let personal = if proxy_enabled {
                personal_proxy.as_deref()
            } else {
                None
            };
            let (source, value) = choose_proxy(actor_id, personal, system_proxy)?;
            validate_proxy(&value)?;
            Some(
                ProxyReference::new(source, version(value.as_bytes()))
                    .map_err(|_| ProfileError::InvalidProxy)?,
            )
        } else {
            None
        };

        let bypass = matches_no_cookie_domain(&target, no_cookie_domains)?;
        let want_oauth = target == "youtube.com" && is_oauth_mode(&youtube_auth_mode);
        let cookie_ref = if bypass {
            None
        } else {
            let shared_allowed = pool_allowed(identity, use_pool);
            let mut candidates = self
                .cookie_candidates(&tx, actor, &target, shared_allowed)
                .await?;
            // OAuth is personal-only. Missing OAuth preserves real cookie fallback.
            if want_oauth {
                let mut oauth_candidates = Vec::new();
                for candidate in candidates.iter().filter(|c| !c.is_pool) {
                    let row = query(include_str!("cookie_by_id.sql"))
                        .bind(candidate.id)
                        .bind(MAX_COOKIE_JAR_BYTES as i64)
                        .fetch_optional(&*tx)
                        .await?;
                    if let Some(row) = row {
                        let content: Option<String> =
                            row.try_get("content").map_err(|_| ProfileError::Database)?;
                        if content.as_deref().is_some_and(is_oauth) {
                            oauth_candidates.push(candidate.clone());
                        }
                    }
                }
                if !oauth_candidates.is_empty() {
                    candidates = oauth_candidates;
                }
            }
            let selected = select_candidate(&candidates, selection_seed);
            if let Some(c) = selected {
                let row = query(include_str!("cookie_by_id.sql"))
                    .bind(c.id)
                    .bind(MAX_COOKIE_JAR_BYTES as i64)
                    .fetch_optional(&*tx)
                    .await?;
                match row {
                    Some(row) => {
                        let content: Option<String> =
                            row.try_get("content").map_err(|_| ProfileError::Database)?;
                        let content = content.ok_or(ProfileError::InvalidCookie)?;
                        let actual_id: i32 =
                            row.try_get("id").map_err(|_| ProfileError::Database)?;
                        let actual_owner: i64 =
                            row.try_get("user_id").map_err(|_| ProfileError::Database)?;
                        let raw_domain: String =
                            row.try_get("domain").map_err(|_| ProfileError::Database)?;
                        let valid: bool = row
                            .try_get("is_valid")
                            .map_err(|_| ProfileError::Database)?;
                        let actual_pool: bool =
                            row.try_get("is_pool").map_err(|_| ProfileError::Database)?;
                        if !valid
                            || actual_id != c.id
                            || actual_owner != c.owner_id
                            || actual_pool != c.is_pool
                            || normalize_domain(&raw_domain)? != c.domain
                            || !domain_match(&target, &c.domain)
                        {
                            return Err(ProfileError::StaleReference);
                        }
                        if is_oauth(&content) {
                            if !want_oauth || actual_pool {
                                return Err(ProfileError::UnsupportedAuthentication);
                            }
                            oauth::require_fresh(&oauth::decode(&content)?, identity.now)?;
                        } else if !valid_cookie(&content) {
                            return Err(ProfileError::InvalidCookie);
                        }
                        let ver = cookie_version(
                            &content,
                            actual_id,
                            actual_owner,
                            &raw_domain,
                            actual_pool,
                            row.try_get("updated_at")
                                .map_err(|_| ProfileError::Database)?,
                        );
                        Some(
                            CookieReference::new(
                                actual_id,
                                actual_owner as u64,
                                c.domain,
                                actual_pool,
                                ver,
                            )
                            .map_err(|_| ProfileError::InvalidCookie)?,
                        )
                    }
                    None => return Err(ProfileError::StaleReference),
                }
            } else {
                None
            }
        };
        tx.commit().await?;
        Ok(NetworkSelection::new(cookie_ref, proxy_ref))
    }

    /// Re-read and validate precisely the frozen references; never chooses replacements.
    pub async fn resolve(
        &self,
        context: NetworkContext<'_>,
        selection: &NetworkSelection,
    ) -> Result<NetworkCredentials, ProfileError> {
        let NetworkContext {
            actor_id,
            domain: requested_domain,
            no_cookie_domains,
            identity,
            system_proxy,
            system_proxy_domains,
        } = context;
        let actor = valid_actor(actor_id)?;
        validate_identity(identity, actor)?;
        let target = normalize_domain(requested_domain)?;
        let mut connection = self.database.connection().await?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Deferred).await?;
        let bypass = matches_no_cookie_domain(&target, no_cookie_domains)?;
        selection
            .validate_actor(actor_id)
            .map_err(|_| ProfileError::StaleReference)?;
        if identity.principal.is_blocked(identity.now) {
            return Err(ProfileError::SharedAccessDenied);
        }
        let settings = query(include_str!("network_settings.sql"))
            .bind(actor)
            .fetch_optional(&*tx)
            .await?;
        let proxy_enabled = settings
            .as_ref()
            .map(|row| row.try_get::<bool, _>("proxy_enabled"))
            .transpose()
            .map_err(|_| ProfileError::Database)?
            .unwrap_or(false);
        let automatic_proxy =
            system_proxy.is_some() && matches_no_cookie_domain(&target, system_proxy_domains)?;
        if (proxy_enabled || automatic_proxy) != selection.proxy().is_some() {
            // A frozen direct route cannot bypass a newly enabled proxy policy.
            return Err(ProfileError::StaleReference);
        }
        let auth_mode = settings
            .as_ref()
            .map(|r| r.try_get::<String, _>("youtube_auth_mode"))
            .transpose()
            .map_err(|_| ProfileError::Database)?
            .unwrap_or_else(|| "cookies".to_owned());
        validate_auth_mode(&auth_mode)?;
        let want_oauth = target == "youtube.com" && is_oauth_mode(&auth_mode);
        if bypass && selection.cookie().is_some() {
            return Err(ProfileError::StaleReference);
        }
        let mut cookie_jar = None;
        let mut youtube_oauth = None;
        if let Some(reference) = selection.cookie() {
            let use_pool = settings
                .as_ref()
                .map(|r| r.try_get::<bool, _>("use_pool_cookies"))
                .transpose()
                .map_err(|_| ProfileError::Database)?
                .unwrap_or(true);
            if reference.is_pool() && !pool_allowed(identity, use_pool) {
                return Err(ProfileError::SharedAccessDenied);
            }
            let row = query(include_str!("cookie_by_id.sql"))
                .bind(reference.id())
                .bind(MAX_COOKIE_JAR_BYTES as i64)
                .fetch_optional(&*tx)
                .await?
                .ok_or(ProfileError::StaleReference)?;
            let id: i32 = row.try_get("id").map_err(|_| ProfileError::Database)?;
            let owner: i64 = row.try_get("user_id").map_err(|_| ProfileError::Database)?;
            let domain: String = row.try_get("domain").map_err(|_| ProfileError::Database)?;
            let valid: bool = row
                .try_get("is_valid")
                .map_err(|_| ProfileError::Database)?;
            let pool: bool = row.try_get("is_pool").map_err(|_| ProfileError::Database)?;
            let updated: DateTime<Utc> = row
                .try_get("updated_at")
                .map_err(|_| ProfileError::Database)?;
            let content: Option<String> =
                row.try_get("content").map_err(|_| ProfileError::Database)?;
            let content = content.ok_or(ProfileError::InvalidCookie)?;
            if !valid
                || id != reference.id()
                || owner as u64 != reference.owner_id()
                || pool != reference.is_pool()
                || !domain_match(&target, &normalize_domain(&domain)?)
                || normalize_domain(&domain)? != reference.domain()
                || cookie_version(&content, id, owner, &domain, pool, updated)
                    != *reference.version()
            {
                return Err(ProfileError::StaleReference);
            }
            if is_oauth(&content) {
                if !want_oauth || pool {
                    return Err(ProfileError::StaleReference);
                }
                let tokens = oauth::decode(&content)?;
                oauth::require_fresh(&tokens, identity.now)?;
                youtube_oauth = Some(tokens);
            } else {
                if !valid_cookie(&content) {
                    return Err(ProfileError::InvalidCookie);
                }
                cookie_jar = Some(content);
            }
        }
        let proxy_url = if let Some(reference) = selection.proxy() {
            let (candidate, expected_source) = match reference.source() {
                ProxySource::Personal { actor_id: owner } if owner == actor_id => {
                    let r = query(include_str!("network_settings.sql"))
                        .bind(actor)
                        .fetch_optional(&*tx)
                        .await?
                        .ok_or(ProfileError::StaleReference)?;
                    if !r
                        .try_get::<bool, _>("proxy_enabled")
                        .map_err(|_| ProfileError::Database)?
                    {
                        return Err(ProfileError::StaleReference);
                    }
                    (
                        r.try_get::<Option<String>, _>("proxy_url")
                            .map_err(|_| ProfileError::Database)?
                            .filter(|x| !x.is_empty())
                            .ok_or(ProfileError::StaleReference)?,
                        ProxySource::Personal { actor_id },
                    )
                }
                ProxySource::System => {
                    if !proxy_enabled && !automatic_proxy {
                        return Err(ProfileError::StaleReference);
                    }
                    let personal = settings
                        .as_ref()
                        .map(|r| r.try_get::<Option<String>, _>("proxy_url"))
                        .transpose()
                        .map_err(|_| ProfileError::Database)?
                        .flatten();
                    if proxy_enabled && personal.filter(|x| !x.is_empty()).is_some() {
                        return Err(ProfileError::StaleReference);
                    }
                    (
                        system_proxy.ok_or(ProfileError::StaleReference)?.to_owned(),
                        ProxySource::System,
                    )
                }
                _ => return Err(ProfileError::StaleReference),
            };
            let _ = expected_source;
            validate_proxy(&candidate)?;
            if version(candidate.as_bytes()) != *reference.version() {
                return Err(ProfileError::StaleReference);
            }
            Some(candidate)
        } else {
            None
        };
        tx.commit().await?;
        Ok(NetworkCredentials {
            youtube_oauth,
            proxy_url,
            cookie_jar,
        })
    }

    async fn cookie_candidates(
        &self,
        connection: &turso::Connection,
        actor: i64,
        domain: &str,
        shared_allowed: bool,
    ) -> Result<Vec<Candidate>, ProfileError> {
        let rows = query(include_str!("cookie_candidates.sql"))
            .bind(actor)
            .bind(shared_allowed)
            .fetch_all(connection)
            .await?;
        let mut out = Vec::new();
        for row in rows {
            let raw: String = row.try_get("domain").map_err(|_| ProfileError::Database)?;
            let normalized = normalize_domain(&raw)?;
            if !candidate_domain_matches(domain, &raw)? {
                continue;
            }
            out.push(Candidate {
                id: row.try_get("id").map_err(|_| ProfileError::Database)?,
                owner_id: row.try_get("user_id").map_err(|_| ProfileError::Database)?,
                domain: normalized,
                is_pool: row.try_get("is_pool").map_err(|_| ProfileError::Database)?,
            });
            if out.len() > MAX_CANDIDATES as usize {
                return Err(ProfileError::CandidateCapacityExceeded);
            }
        }
        Ok(out)
    }
}

pub(crate) fn pool_allowed(identity: &IdentitySnapshot, use_pool: bool) -> bool {
    let feature = FeatureSpec {
        plugin: "dl".into(),
        feature: "shared_cookies".into(),
        default_min_role: Some(Role::Admin),
    };
    use_pool
        && effective_grant(
            &identity.principal,
            &feature,
            &identity.grants,
            &identity.providers,
            identity.now,
        )
        .is_some()
}

pub(super) fn choose_proxy(
    actor_id: u64,
    personal: Option<&str>,
    system: Option<&str>,
) -> Result<(ProxySource, String), ProfileError> {
    if let Some(value) = personal.filter(|value| !value.is_empty()) {
        Ok((ProxySource::Personal { actor_id }, value.to_owned()))
    } else {
        Ok((
            ProxySource::System,
            system.ok_or(ProfileError::ProxyUnavailable)?.to_owned(),
        ))
    }
}

fn select_candidate(candidates: &[Candidate], seed: u64) -> Option<Candidate> {
    let personal = candidates.iter().find(|c| !c.is_pool);
    let pool: Vec<_> = candidates.iter().filter(|c| c.is_pool).collect();
    match (personal, pool.is_empty(), seed.is_multiple_of(2)) {
        (Some(p), _, true) => Some(p.clone()),
        (Some(_), false, false) => Some(pool[((seed / 2) % pool.len() as u64) as usize].clone()),
        (Some(p), true, _) => Some(p.clone()),
        (None, false, _) => Some(pool[(seed % pool.len() as u64) as usize].clone()),
        (None, true, _) => None,
    }
}

pub fn resolve_format_preferences(
    mode: MediaMode,
    preferences: Option<&DownloadPreferences>,
    overrides: &DownloadPreferenceOverrides,
) -> Result<DownloadParams, ProfileError> {
    let compact = preferences.map(|p| FormatPreferences {
        quality: Some(p.quality.clone()),
        codec: Some(p.codec.clone()),
        container: Some(p.container.clone()),
        send_as_file: p.send_as_file,
        audio_codec: Some(p.audio_codec.clone()),
        gallery_zip: p.gallery_zip,
        subtitle_language: Some(p.subs_lang.clone()),
        subtitle_automatic: p.subs_auto,
    });
    resolve_format_values(mode, compact.as_ref(), overrides)
}

pub(crate) fn resolve_format_values(
    mode: MediaMode,
    preferences: Option<&FormatPreferences>,
    o: &DownloadPreferenceOverrides,
) -> Result<DownloadParams, ProfileError> {
    if mode != MediaMode::Subtitles
        && (o.subtitle_language.is_some() || o.subtitle_automatic.is_some())
    {
        return Err(ProfileError::InvalidPreferences);
    }
    let irrelevant = match mode {
        MediaMode::Subtitles => {
            o.max_height.is_some()
                || o.video_codec.is_some()
                || o.audio_codec.is_some()
                || o.container.is_some()
                || o.gallery_zip.is_some()
                || o.send_as_file.is_some()
        }
        MediaMode::Video => o.gallery_zip.is_some(),
        MediaMode::Audio => {
            o.max_height.is_some()
                || o.video_codec.is_some()
                || o.gallery_zip.is_some()
                || o.container.is_some()
        }
        MediaMode::Image => {
            o.max_height.is_some()
                || o.video_codec.is_some()
                || o.audio_codec.is_some()
                || o.container.is_some()
        }
        MediaMode::Formats | MediaMode::Links | MediaMode::AudioLinks => true,
    };
    if irrelevant
        || o.max_height
            .flatten()
            .is_some_and(|h| h == 0 || h > 100_000)
    {
        return Err(ProfileError::InvalidPreferences);
    }
    let defaults = FormatPreferences {
        quality: Some("best".to_owned()),
        codec: Some("avc1".to_owned()),
        container: Some("mp4".to_owned()),
        send_as_file: false,
        audio_codec: Some("best".to_owned()),
        gallery_zip: false,
        subtitle_language: Some("en".into()),
        subtitle_automatic: false,
    };
    let p = Some(preferences.unwrap_or(&defaults));
    let mut params = DownloadParams::default();
    match mode {
        MediaMode::Subtitles => {
            let language = o
                .subtitle_language
                .as_deref()
                .or_else(|| p.and_then(|p| p.subtitle_language.as_deref()))
                .ok_or(ProfileError::InvalidPreferences)?;
            let automatic = o
                .subtitle_automatic
                .unwrap_or(p.is_some_and(|p| p.subtitle_automatic));
            params = params.with_subtitles(Some(
                SubtitleOptions::new(language, automatic)
                    .map_err(|_| ProfileError::InvalidPreferences)?,
            ));
        }
        MediaMode::Video => {
            let height = match o.max_height {
                Some(height) => height,
                None => {
                    let quality = p
                        .and_then(|p| p.quality.as_deref())
                        .ok_or(ProfileError::InvalidPreferences)?
                        .to_ascii_lowercase();
                    match quality.as_str() {
                        "best" => None,
                        "4k" => Some(2160),
                        "8k" => Some(4320),
                        value => {
                            let numeric = value.strip_suffix('p').unwrap_or(value);
                            Some(
                                numeric
                                    .parse::<u32>()
                                    .ok()
                                    .filter(|v| *v > 0 && *v <= 100_000)
                                    .ok_or(ProfileError::InvalidPreferences)?,
                            )
                        }
                    }
                }
            };
            params = params
                .with_max_height(height)
                .map_err(|_| ProfileError::InvalidPreferences)?;
            let codec = match o.video_codec {
                Some(v) => v,
                None => match p
                    .and_then(|p| p.codec.as_deref())
                    .ok_or(ProfileError::InvalidPreferences)?
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "avc1" | "h264" => VideoCodec::H264,
                    "h265" | "hevc" => VideoCodec::H265,
                    "av01" | "av1" => VideoCodec::Av1,
                    "vp09" | "vp9" => VideoCodec::Vp9,
                    "any" => VideoCodec::Any,
                    _ => return Err(ProfileError::InvalidPreferences),
                },
            };
            let container = match o.container {
                Some(v) => v,
                None => parse_container(
                    p.and_then(|p| p.container.as_deref())
                        .ok_or(ProfileError::InvalidPreferences)?,
                )?,
            };
            if container == Container::Webm {
                if !matches!(codec, VideoCodec::Vp9 | VideoCodec::Av1)
                    || o.audio_codec != Some(AudioCodec::Opus)
                {
                    return Err(ProfileError::InvalidPreferences);
                }
                params = params.with_audio_codec(AudioCodec::Opus);
            } else {
                params = params.with_audio_codec(o.audio_codec.unwrap_or(AudioCodec::Best));
            }
            params = params
                .with_video_codec(codec)
                .with_container(container)
                .with_send_as_file(
                    o.send_as_file
                        .unwrap_or(p.map(|p| p.send_as_file).unwrap_or(false)),
                );
        }
        MediaMode::Audio => {
            let audio = match o.audio_codec {
                Some(v) => v,
                None => match p
                    .and_then(|p| p.audio_codec.as_deref())
                    .ok_or(ProfileError::InvalidPreferences)?
                {
                    "best" => AudioCodec::Best,
                    "aac" | "mp4a" => AudioCodec::Aac,
                    "mp3" => AudioCodec::Mp3,
                    "opus" => AudioCodec::Opus,
                    _ => return Err(ProfileError::InvalidPreferences),
                },
            };
            params = params.with_audio_codec(audio).with_send_as_file(
                o.send_as_file
                    .unwrap_or(p.map(|p| p.send_as_file).unwrap_or(false)),
            );
        }
        MediaMode::Image => {
            params = params
                .with_gallery_zip(
                    o.gallery_zip
                        .unwrap_or(p.map(|p| p.gallery_zip).unwrap_or(false)),
                )
                .with_send_as_file(
                    o.send_as_file
                        .unwrap_or(p.map(|p| p.send_as_file).unwrap_or(false)),
                );
        }
        MediaMode::Formats | MediaMode::Links | MediaMode::AudioLinks => {
            return Err(ProfileError::InvalidPreferences);
        }
    }
    Ok(params)
}

fn parse_container(value: &str) -> Result<Container, ProfileError> {
    match value {
        "mp4" => Ok(Container::Mp4),
        "mkv" => Ok(Container::Mkv),
        "webm" => Ok(Container::Webm),
        _ => Err(ProfileError::InvalidPreferences),
    }
}

fn validate_preferences(p: &DownloadPreferences) -> Result<(), ProfileError> {
    let bounded = |value: &str, max: usize| {
        !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
    };
    if !bounded(&p.quality, 32)
        || !bounded(&p.codec, 16)
        || !bounded(&p.container, 8)
        || !bounded(&p.keyboard, 8)
        || !bounded(&p.subs_lang, 8)
        || !bounded(&p.audio_codec, 16)
        || !bounded(&p.youtube_auth_mode, 16)
        || validate_auth_mode(&p.youtube_auth_mode).is_err()
        || p.split_size <= 0
        || p.dm_topic_thread_id.is_some_and(|id| id <= 0)
        || !p.args_json.is_object()
        || serde_json::to_vec(&p.args_json).map_or(true, |v| v.len() > MAX_ARGS_JSON_BYTES)
        || p.proxy_url
            .as_ref()
            .is_some_and(|url| url.len() > MAX_PROXY_URL_BYTES || url.chars().any(char::is_control))
    {
        return Err(ProfileError::InvalidPreferences);
    }
    Ok(())
}

fn valid_actor(actor: u64) -> Result<i64, ProfileError> {
    i64::try_from(actor)
        .ok()
        .filter(|v| *v > 0)
        .ok_or(ProfileError::Unavailable)
}
fn version(bytes: &[u8]) -> CredentialVersion {
    CredentialVersion::new(format!("{:x}", Sha256::digest(bytes))).expect("SHA-256 length")
}
pub(crate) fn cookie_version(
    content: &str,
    id: i32,
    owner: i64,
    domain: &str,
    pool: bool,
    updated: DateTime<Utc>,
) -> CredentialVersion {
    let mut h = Sha256::new();
    let owner_bytes = owner.to_be_bytes();
    let id_bytes = id.to_be_bytes();
    let time_bytes = updated.timestamp_micros().to_be_bytes();
    let pool_bytes = [pool as u8];
    for part in [
        content.as_bytes(),
        &id_bytes[..],
        &owner_bytes[..],
        domain.as_bytes(),
        &pool_bytes[..],
        &time_bytes[..],
    ] {
        h.update((part.len() as u64).to_be_bytes());
        h.update(part);
    }
    CredentialVersion::new(format!("{:x}", h.finalize())).expect("SHA-256 length")
}
fn is_oauth(value: &str) -> bool {
    value.trim_start().starts_with("__oauth2__")
        || value.trim_start().starts_with("{\"__oauth2__\"")
}
pub(crate) fn valid_cookie(value: &str) -> bool {
    if value.is_empty()
        || value.len() > MAX_COOKIE_JAR_BYTES
        || value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\t' | '\r' | '\n'))
    {
        return false;
    }
    let mut lines = value.lines();
    if !matches!(
        lines.next(),
        Some("# Netscape HTTP Cookie File" | "# HTTP Cookie File")
    ) {
        return false;
    }
    let mut records = 0usize;
    for raw in lines {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#HttpOnly_") {
            if !valid_cookie_record(rest) {
                return false;
            }
            records += 1;
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if !valid_cookie_record(line) {
            return false;
        }
        records += 1;
    }
    records > 0
}
fn valid_cookie_record(line: &str) -> bool {
    let fields: Vec<_> = line.split('\t').collect();
    fields.len() == 7
        && !fields[0].is_empty()
        && matches!(fields[1], "TRUE" | "FALSE")
        && fields[0].starts_with('.') == (fields[1] == "TRUE")
        && fields[2].starts_with('/')
        && matches!(fields[3], "TRUE" | "FALSE")
        && (fields[4].is_empty() || fields[4].parse::<i64>().is_ok())
        && !fields[5].is_empty()
}
pub(super) fn validate_proxy(value: &str) -> Result<(), ProfileError> {
    if value.is_empty() || value.len() > MAX_PROXY_URL_BYTES || value.chars().any(char::is_control)
    {
        return Err(ProfileError::InvalidProxy);
    }
    let url = Url::parse(value).map_err(|_| ProfileError::InvalidProxy)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none_or(str::is_empty)
        || value
            .split_once("://")
            .is_none_or(|(_, a)| a.starts_with('/'))
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ProfileError::InvalidProxy);
    }
    Ok(())
}
pub fn normalize_domain(value: &str) -> Result<String, ProfileError> {
    let input = value.trim();
    let host = if let Ok(ip) = input.parse::<std::net::IpAddr>() {
        return Ok(ip.to_string());
    } else {
        let url_text = if input.contains("://") {
            input.to_owned()
        } else {
            format!("https://{input}")
        };
        let url = Url::parse(&url_text).map_err(|_| ProfileError::Unavailable)?;
        match url.host().ok_or(ProfileError::Unavailable)? {
            url::Host::Domain(host) => host.trim_end_matches('.').to_ascii_lowercase(),
            url::Host::Ipv4(ip) => return Ok(ip.to_string()),
            url::Host::Ipv6(ip) => return Ok(ip.to_string()),
        }
    };
    let mut host = host;
    if let Some(h) = host.strip_prefix("www.") {
        host = h.to_owned();
    }
    host = match host.as_str() {
        "youtu.be" | "m.youtube.com" | "music.youtube.com" => "youtube.com",
        "x.com" | "m.twitter.com" => "twitter.com",
        "m.instagram.com" => "instagram.com",
        "m.tiktok.com" => "tiktok.com",
        "m.facebook.com" => "facebook.com",
        "m.reddit.com" | "old.reddit.com" => "reddit.com",
        _ => host.as_str(),
    }
    .to_owned();
    if host.is_empty()
        || host.len() > 253
        || host.split('.').any(|l| {
            l.is_empty()
                || l.len() > 63
                || l.starts_with('-')
                || l.ends_with('-')
                || !l
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
    {
        return Err(ProfileError::Unavailable);
    }
    Ok(host)
}
fn domain_match(target: &str, cookie_domain: &str) -> bool {
    target == cookie_domain
        || target
            .strip_suffix(cookie_domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}
fn validate_identity(identity: &IdentitySnapshot, actor: i64) -> Result<(), ProfileError> {
    if identity.principal.user_id != actor {
        return Err(ProfileError::Unavailable);
    }
    if identity.principal.is_blocked(identity.now) {
        return Err(ProfileError::SharedAccessDenied);
    }
    Ok(())
}
fn validate_auth_mode(mode: &str) -> Result<(), ProfileError> {
    if matches!(mode, "cookies" | "oauth" | "oauth2") {
        Ok(())
    } else {
        Err(ProfileError::InvalidPreferences)
    }
}
fn is_oauth_mode(mode: &str) -> bool {
    matches!(mode, "oauth" | "oauth2")
}
fn candidate_domain_matches(target: &str, stored: &str) -> Result<bool, ProfileError> {
    Ok(domain_match(target, &normalize_domain(stored)?))
}
fn matches_no_cookie_domain(target: &str, patterns: &[String]) -> Result<bool, ProfileError> {
    for pattern in patterns {
        if domain_match(target, &normalize_domain(pattern)?) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn format_prefs(quality: &str, codec: &str, container: &str, audio: &str) -> FormatPreferences {
        FormatPreferences {
            quality: Some(quality.to_owned()),
            codec: Some(codec.to_owned()),
            container: Some(container.to_owned()),
            send_as_file: false,
            audio_codec: Some(audio.to_owned()),
            gallery_zip: false,
            subtitle_language: Some("en".into()),
            subtitle_automatic: false,
        }
    }

    #[test]
    fn format_preference_aliases_and_modes_are_resolved_safely() {
        let prefs = format_prefs("4K", "AV01", "mp4", "not-a-video-setting");
        let params = resolve_format_values(
            MediaMode::Video,
            Some(&prefs),
            &DownloadPreferenceOverrides::default(),
        )
        .unwrap();
        assert_eq!(params.max_height(), Some(2160));
        assert_eq!(params.video_codec(), VideoCodec::Av1);
        assert_eq!(params.audio_codec(), AudioCodec::Best);

        for (quality, height) in [("720", 720), ("720P", 720), ("8k", 4320), ("best", 0)] {
            let prefs = format_prefs(quality, "vp09", "mkv", "malformed legacy audio codec");
            let params = resolve_format_values(
                MediaMode::Video,
                Some(&prefs),
                &DownloadPreferenceOverrides::default(),
            )
            .unwrap();
            assert_eq!(params.max_height(), (height != 0).then_some(height));
            assert_eq!(params.video_codec(), VideoCodec::Vp9);
            assert_eq!(params.audio_codec(), AudioCodec::Best);
        }

        assert!(
            resolve_format_values(
                MediaMode::Video,
                None,
                &DownloadPreferenceOverrides::default()
            )
            .is_ok()
        );
        let audio = format_prefs("ignored", "ignored", "ignored", "mp4a");
        assert_eq!(
            resolve_format_values(
                MediaMode::Audio,
                Some(&audio),
                &DownloadPreferenceOverrides::default()
            )
            .unwrap()
            .audio_codec(),
            AudioCodec::Aac
        );
        let image = FormatPreferences {
            gallery_zip: true,
            ..format_prefs("ignored", "ignored", "ignored", "ignored")
        };
        assert!(
            resolve_format_values(
                MediaMode::Image,
                Some(&image),
                &DownloadPreferenceOverrides::default()
            )
            .unwrap()
            .gallery_zip()
        );
        assert!(
            resolve_format_values(
                MediaMode::Image,
                None,
                &DownloadPreferenceOverrides::default()
            )
            .is_ok()
        );
    }

    #[test]
    fn overrides_validate_bounds_relevance_webm_and_rescue_bad_stored_values() {
        for height in [0, 100_001] {
            let overrides = DownloadPreferenceOverrides {
                max_height: Some(Some(height)),
                ..Default::default()
            };
            assert_eq!(
                resolve_format_values(MediaMode::Video, None, &overrides),
                Err(ProfileError::InvalidPreferences)
            );
        }
        let min = DownloadPreferenceOverrides {
            max_height: Some(Some(1)),
            ..Default::default()
        };
        let max = DownloadPreferenceOverrides {
            max_height: Some(Some(100_000)),
            ..Default::default()
        };
        assert_eq!(
            resolve_format_values(MediaMode::Video, None, &min)
                .unwrap()
                .max_height(),
            Some(1)
        );
        assert_eq!(
            resolve_format_values(MediaMode::Video, None, &max)
                .unwrap()
                .max_height(),
            Some(100_000)
        );

        let invalid = format_prefs(
            "ask",
            "malicious-quality-codec",
            "malicious-container",
            "malicious-audio",
        );
        assert_eq!(
            resolve_format_values(
                MediaMode::Video,
                Some(&format_prefs("ask", "avc1", "mp4", "best")),
                &DownloadPreferenceOverrides::default(),
            ),
            Err(ProfileError::InvalidPreferences)
        );
        let rescue = DownloadPreferenceOverrides {
            max_height: Some(None),
            video_codec: Some(VideoCodec::H264),
            container: Some(Container::Mp4),
            audio_codec: Some(AudioCodec::Opus),
            ..Default::default()
        };
        let resolved = resolve_format_values(MediaMode::Video, Some(&invalid), &rescue).unwrap();
        assert_eq!(resolved.max_height(), None);
        assert_eq!(resolved.video_codec(), VideoCodec::H264);
        assert_eq!(resolved.audio_codec(), AudioCodec::Opus);

        let webm = format_prefs("best", "vp9", "webm", "malformed ignored legacy audio");
        assert_eq!(
            resolve_format_values(
                MediaMode::Video,
                Some(&webm),
                &DownloadPreferenceOverrides::default()
            ),
            Err(ProfileError::InvalidPreferences)
        );
        let webm_override = DownloadPreferenceOverrides {
            audio_codec: Some(AudioCodec::Opus),
            ..Default::default()
        };
        assert_eq!(
            resolve_format_values(MediaMode::Video, Some(&webm), &webm_override)
                .unwrap()
                .audio_codec(),
            AudioCodec::Opus
        );

        for (mode, overrides) in [
            (
                MediaMode::Audio,
                DownloadPreferenceOverrides {
                    container: Some(Container::Mkv),
                    ..Default::default()
                },
            ),
            (
                MediaMode::Video,
                DownloadPreferenceOverrides {
                    gallery_zip: Some(true),
                    ..Default::default()
                },
            ),
            (
                MediaMode::Audio,
                DownloadPreferenceOverrides {
                    video_codec: Some(VideoCodec::Vp9),
                    ..Default::default()
                },
            ),
            (
                MediaMode::Image,
                DownloadPreferenceOverrides {
                    audio_codec: Some(AudioCodec::Opus),
                    ..Default::default()
                },
            ),
        ] {
            assert_eq!(
                resolve_format_values(mode, None, &overrides),
                Err(ProfileError::InvalidPreferences)
            );
        }
        let bad = format_prefs(
            "99999999999999999999999999999999",
            "unsafe",
            "bad",
            "unsafe",
        );
        assert_eq!(
            resolve_format_values(
                MediaMode::Video,
                Some(&bad),
                &DownloadPreferenceOverrides::default()
            ),
            Err(ProfileError::InvalidPreferences)
        );
    }
    use yoink_access::{Grant, Principal};
    use yoink_identity::IdentitySnapshot;

    fn identity(role: Role) -> IdentitySnapshot {
        IdentitySnapshot {
            principal: Principal {
                user_id: 42,
                role,
                ban_until: None,
            },
            language: "en".into(),
            grants: vec![],
            providers: vec![],
            now: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            group: None,
        }
    }

    #[test]
    fn shared_access_honors_roles_expiry_and_bans() {
        assert!(!pool_allowed(&identity(Role::User), true));
        assert!(pool_allowed(&identity(Role::Admin), true));
        assert!(pool_allowed(&identity(Role::Owner), true));
        assert!(!pool_allowed(&identity(Role::Admin), false));
        let mut explicit = identity(Role::Restricted);
        explicit.grants.push(Grant {
            plugin: "dl".into(),
            feature: "shared_cookies".into(),
            expires_at: Some(explicit.now),
        });
        assert!(!pool_allowed(&explicit, true));
        explicit.grants[0].expires_at = None;
        assert!(pool_allowed(&explicit, true));
        explicit.principal.role = Role::Banned;
        assert!(!pool_allowed(&explicit, true));
        explicit.principal.role = Role::Restricted;
        explicit.principal.ban_until = Some(explicit.now + chrono::Duration::seconds(1));
        assert!(!pool_allowed(&explicit, true));
    }

    #[test]
    fn candidate_rotation_is_seeded_sorted_and_personal_pool_alternates() {
        let candidates = vec![
            Candidate {
                id: 5,
                owner_id: 9,
                domain: "example.com".into(),
                is_pool: true,
            },
            Candidate {
                id: 2,
                owner_id: 42,
                domain: "example.com".into(),
                is_pool: false,
            },
            Candidate {
                id: 8,
                owner_id: 9,
                domain: "example.com".into(),
                is_pool: true,
            },
        ];
        assert_eq!(select_candidate(&candidates, 0).unwrap().id, 2);
        assert_eq!(select_candidate(&candidates, 1).unwrap().id, 5);
        assert_eq!(select_candidate(&candidates, 3).unwrap().id, 8);
    }

    #[test]
    fn proxy_precedence_and_no_direct_fallback_are_explicit() {
        assert_eq!(
            choose_proxy(42, Some("http://personal.test"), Some("http://system.test"))
                .unwrap()
                .0,
            ProxySource::Personal { actor_id: 42 }
        );
        assert_eq!(
            choose_proxy(42, None, Some("http://system.test"))
                .unwrap()
                .0,
            ProxySource::System
        );
        assert_eq!(
            choose_proxy(42, None, None),
            Err(ProfileError::ProxyUnavailable)
        );
    }

    #[test]
    fn cookie_versions_change_with_content_and_metadata_and_are_redacted() {
        let time = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let a = cookie_version("content-a", 5, 42, "example.com", false, time);
        let b = cookie_version("content-b", 5, 42, "example.com", false, time);
        let c = cookie_version(
            "content-a",
            5,
            42,
            "example.com",
            false,
            time + chrono::Duration::seconds(1),
        );
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert!(!format!("{a:?}").contains(a.as_str()));
    }

    #[test]
    fn aliases_and_suffix_boundaries_are_safe() {
        assert_eq!(
            normalize_domain("https://youtu.be:443/a").unwrap(),
            "youtube.com"
        );
        assert!(domain_match("a.example.com", "example.com"));
        assert!(!domain_match("notexample.com", "example.com"));
    }
    #[test]
    fn debug_never_exposes_proxy_or_arbitrary_args() {
        let p = DownloadPreferences {
            quality: "best".into(),
            codec: "avc1".into(),
            container: "mp4".into(),
            proxy_enabled: true,
            proxy_url: Some("http://secret.test".into()),
            keyboard: "2x3".into(),
            subs_enabled: false,
            subs_auto: false,
            subs_always_ask: false,
            subs_lang: "en".into(),
            split_size: 1,
            nsfw_blur: true,
            mediainfo: false,
            send_as_file: false,
            audio_codec: "best".into(),
            gallery_zip: false,
            use_pool_cookies: true,
            youtube_auth_mode: "cookies".into(),
            dm_topic_thread_id: None,
            args_json: serde_json::json!({"token":"secret"}),
            updated_at: None,
        };
        let debug = format!("{p:?}");
        assert!(!debug.contains("secret"));
    }
    #[test]
    fn rejects_bad_proxy_grammar_and_size() {
        for value in [
            "ftp://proxy.test",
            "https:///missing",
            "https://proxy.test/path",
            "https://proxy.test?x=1",
            "https://proxy.test\n",
        ] {
            assert_eq!(validate_proxy(value), Err(ProfileError::InvalidProxy));
        }
        assert!(validate_proxy("https://proxy.test:8443").is_ok());
        assert_eq!(
            validate_proxy(&format!("http://x/{}", "a".repeat(MAX_PROXY_URL_BYTES))),
            Err(ProfileError::InvalidProxy)
        );
    }

    #[test]
    fn rejects_oauth_as_cookie_and_bounds_content() {
        assert!(is_oauth("__oauth2__{...}"));
        assert!(!valid_cookie("__oauth2__{...}"));
        assert!(!valid_cookie(&"x".repeat(MAX_COOKIE_JAR_BYTES + 1)));
    }
    #[test]
    fn cookie_jar_accepts_httponly_and_rejects_bad_records_or_header() {
        let valid = "# Netscape HTTP Cookie File\n#HttpOnly_.example.com\tTRUE\t/\tTRUE\t1700000000\tsid\tfixture\n";
        assert!(valid_cookie(valid));
        assert!(valid_cookie(
            "# HTTP Cookie File\nexample.com\tFALSE\t/\tFALSE\t\tsession\tfixture\n"
        ));
        assert!(valid_cookie(&valid.replace('\n', "\r\n")));
        assert!(!valid_cookie(&format!("{valid}malformed\n")));
        assert!(!valid_cookie(
            &valid.replace("# Netscape HTTP Cookie File", "# Some other format")
        ));
        assert!(!valid_cookie(
            &valid.replace("1700000000\tsid", "1700000000")
        ));
        assert!(!valid_cookie(
            &valid.replace(".example.com\tTRUE", "example.com\tTRUE")
        ));
        assert!(!valid_cookie(&format!("\n{valid}")));
    }
    #[test]
    fn domain_alias_and_label_validation_are_safe() {
        assert!(candidate_domain_matches("youtube.com", "youtu.be").unwrap());
        assert!(candidate_domain_matches("sub.example.com", "example.com").unwrap());
        assert!(!candidate_domain_matches("notexample.com", "example.com").unwrap());
        assert_eq!(normalize_domain("[::1]").unwrap(), "::1");
        assert_eq!(
            normalize_domain("https://[2001:db8::1]:8443/media").unwrap(),
            "2001:db8::1"
        );
        assert!(normalize_domain("-bad.example").is_err());
        assert!(normalize_domain("bad-.example").is_err());
        assert!(normalize_domain("bad:domain").is_err());
    }
}
