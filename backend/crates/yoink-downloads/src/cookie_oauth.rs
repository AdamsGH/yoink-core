//! Typed imported YouTube TV credentials; pre-admission refresh only.
use crate::{CookieDomain, DownloadError, DownloadService, cookie_web::hash};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use turso::transaction::TransactionBehavior;
use yoink_db::Transaction;
use yoink_db::query;
use yoink_domain::CookieReference;
pub use yoink_worker_protocol::youtube_auth::YoutubeOAuth as OAuthCredential;
pub const CONTENT_PREFIX: &str = yoink_worker_protocol::youtube_auth::OAUTH_CONTENT_PREFIX;
pub const CREDENTIAL_KIND: &str = "youtube_tv_oauth_v1";
const PREFIX: &str = CONTENT_PREFIX;
const MAX_OAUTH_BYTES: usize = yoink_worker_protocol::youtube_auth::MAX_OAUTH_CONTENT_BYTES;
const SESSION_TTL: i64 = 1800;
pub trait OAuthCredentialContent: Sized {
    fn decode(content: &str) -> Result<Self, DownloadError>;
    fn encode(&self) -> Result<String, DownloadError>;
}
impl OAuthCredentialContent for OAuthCredential {
    fn decode(content: &str) -> Result<Self, DownloadError> {
        let value = Self::decode_content(content).map_err(|_| DownloadError::InvalidInput)?;
        DateTime::parse_from_rfc3339(&value.expires_at).map_err(|_| DownloadError::InvalidInput)?;
        Ok(value)
    }
    fn encode(&self) -> Result<String, DownloadError> {
        DateTime::parse_from_rfc3339(&self.expires_at).map_err(|_| DownloadError::InvalidInput)?;
        self.encode_content()
            .map_err(|_| DownloadError::InvalidInput)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthReference {
    pub cookie: CookieReference,
    pub kind: &'static str,
}
fn reference(
    id: i64,
    user: i64,
    domain: &str,
    content: &str,
    updated: DateTime<Utc>,
) -> Result<OAuthReference, DownloadError> {
    let id = i32::try_from(id).map_err(|_| DownloadError::InvalidCredentials)?;
    let version = crate::profile::cookie_version(content, id, user, domain, false, updated);
    let canonical = CookieDomain::parse(domain)?;
    Ok(OAuthReference {
        cookie: CookieReference::new(id, user as u64, canonical.as_str(), false, version)
            .map_err(|_| DownloadError::InvalidCredentials)?,
        kind: CREDENTIAL_KIND,
    })
}
#[derive(Clone, Copy, Debug)]
pub struct OAuthProviderConfig {
    timeout: Duration,
}
impl Default for OAuthProviderConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
        }
    }
}
impl OAuthProviderConfig {
    pub fn new(timeout: Duration) -> Result<Self, DownloadError> {
        if timeout < Duration::from_secs(1) || timeout > Duration::from_secs(60) {
            return Err(DownloadError::InvalidConfig);
        }
        Ok(Self { timeout })
    }
    pub fn timeout(self) -> Duration {
        self.timeout
    }
}
#[derive(Serialize)]
pub struct DeviceStart {
    pub session_id: String,
    pub verification_url: String,
    pub user_code: String,
    pub expires_in: i64,
    pub interval: i64,
}
#[derive(Serialize)]
pub struct DevicePoll {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<&'static str>,
}
pub struct ProviderDevice {
    pub device_code: String,
    pub verification_url: String,
    pub user_code: String,
    pub expires_in: i64,
    pub interval: i64,
}
pub enum ProviderPoll {
    Pending,
    SlowDown,
    Expired,
    Error,
    Ready(OAuthCredential),
}
pub type ProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, DownloadError>> + Send + 'a>>;
pub trait OAuthProvider: Send + Sync {
    fn timeout(&self) -> Duration {
        Duration::from_secs(15)
    }
    fn start(&self) -> ProviderFuture<'_, ProviderDevice>;
    fn poll<'a>(&'a self, device_code: &'a str, now: i64) -> ProviderFuture<'a, ProviderPoll>;
    fn refresh<'a>(
        &'a self,
        credential: &'a OAuthCredential,
        now: i64,
    ) -> ProviderFuture<'a, OAuthCredential>;
}
/// Fixed legacy provider origins, HTTPS only, no redirects/environment proxies,
/// 15s timeout and bounded provider response. Error bodies are never propagated.
pub struct YouTubeOAuthProvider {
    timeout: Duration,
    client: reqwest::Client,
}
impl YouTubeOAuthProvider {
    pub fn new() -> Result<Self, DownloadError> {
        Self::with_config(OAuthProviderConfig::default())
    }
    pub fn with_config(config: OAuthProviderConfig) -> Result<Self, DownloadError> {
        Ok(Self {
            timeout: config.timeout(),
            client: crate::cookie_web_network::client(config.timeout())?,
        })
    }
    async fn post(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, DownloadError> {
        let mut response = self
            .client
            .post(format!("https://www.youtube.com/o/oauth2/{path}"))
            .json(&body)
            .send()
            .await
            .map_err(|_| DownloadError::Executor)?;
        if response.status().is_redirection() {
            return Err(DownloadError::Executor);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| DownloadError::Executor)?
        {
            if bytes.len() + chunk.len() > MAX_OAUTH_BYTES {
                return Err(DownloadError::Executor);
            }
            bytes.extend_from_slice(&chunk)
        }
        serde_json::from_slice(&bytes).map_err(|_| DownloadError::Executor)
    }
}
const CLIENT_ID: &str = "861556708454-d6dlm3lh05idd8npek18k6be8ba3oc68.apps.googleusercontent.com";
// Public legacy TV application credential, never an operator credential.
const CLIENT_SECRET: &str = "SboVhoG9s0rNafixCSGGKXAT";
fn tokens(
    v: &serde_json::Value,
    refresh: Option<&str>,
    now: i64,
) -> Result<OAuthCredential, DownloadError> {
    let value = OAuthCredential {
        access_token: v["access_token"]
            .as_str()
            .ok_or(DownloadError::Executor)?
            .into(),
        refresh_token: v["refresh_token"]
            .as_str()
            .or(refresh)
            .ok_or(DownloadError::Executor)?
            .into(),
        expires_at: DateTime::from_timestamp(
            now.checked_add(v["expires_in"].as_i64().unwrap_or(3600).clamp(1, 86400))
                .ok_or(DownloadError::Executor)?,
            0,
        )
        .ok_or(DownloadError::Executor)?
        .to_rfc3339(),
    };
    value.validate().map_err(|_| DownloadError::InvalidInput)?;
    Ok(value)
}
impl OAuthProvider for YouTubeOAuthProvider {
    fn timeout(&self) -> Duration {
        self.timeout
    }
    fn start(&self) -> ProviderFuture<'_, ProviderDevice> {
        Box::pin(async move {
            let v=self.post("device/code",serde_json::json!({"client_id":CLIENT_ID,"scope":"http://gdata.youtube.com https://www.googleapis.com/auth/youtube","device_id":uuid::Uuid::new_v4().simple().to_string(),"device_model":"ytlr::"})).await?;
            Ok(ProviderDevice {
                device_code: v["device_code"]
                    .as_str()
                    .ok_or(DownloadError::Executor)?
                    .into(),
                verification_url: v["verification_url"]
                    .as_str()
                    .ok_or(DownloadError::Executor)?
                    .into(),
                user_code: v["user_code"]
                    .as_str()
                    .ok_or(DownloadError::Executor)?
                    .into(),
                expires_in: v["expires_in"].as_i64().unwrap_or(SESSION_TTL),
                interval: v["interval"].as_i64().unwrap_or(5),
            })
        })
    }
    fn poll<'a>(&'a self, code: &'a str, now: i64) -> ProviderFuture<'a, ProviderPoll> {
        Box::pin(async move {
            let v=self.post("token",serde_json::json!({"client_id":CLIENT_ID,"client_secret":CLIENT_SECRET,"code":code,"grant_type":"http://oauth.net/grant_type/device/1.0"})).await?;
            Ok(match v["error"].as_str() {
                Some("authorization_pending") => ProviderPoll::Pending,
                Some("slow_down") => ProviderPoll::SlowDown,
                Some("expired_token") => ProviderPoll::Expired,
                Some(_) => ProviderPoll::Error,
                None => ProviderPoll::Ready(tokens(&v, None, now)?),
            })
        })
    }
    fn refresh<'a>(
        &'a self,
        c: &'a OAuthCredential,
        now: i64,
    ) -> ProviderFuture<'a, OAuthCredential> {
        Box::pin(async move {
            let v=self.post("token",serde_json::json!({"client_id":CLIENT_ID,"client_secret":CLIENT_SECRET,"refresh_token":c.refresh_token,"grant_type":"refresh_token"})).await?;
            tokens(&v, Some(&c.refresh_token), now)
        })
    }
}
fn db(_: yoink_db::Error) -> DownloadError {
    DownloadError::Database
}
/// Single-service poll ownership shared by every DownloadService clone.
/// Exactly one live service ownership registry per database (all clones share it).
/// Runtime must stop HTTP admission, call drain_cookie_oauth, then close the DB.
/// Drain closes admission atomically and waits for bounded work, not HTTP callers.
/// Total accepted poll budget includes DB waits, provider I/O and persistence.
/// Temporary expiry protection ends at acceptance + budget + 1s; timeout releases
/// the ownership slot and leaves any session collectible by cleanup/start.
/// New Ready gets result_retention for capacity retries; retries do not renew it.
/// A hung DB beyond this budget or forced process termination can lose an unstaged
/// provider response. Runtime must schedule cleanup_cookie_oauth periodically and
/// allow at least budget plus scheduling margin to drain before closing the store.
#[derive(Clone, Copy, Debug)]
pub struct OAuthPollConfig {
    pub budget: Duration,
    pub result_retention: Duration,
    pub max_inflight: usize,
}
impl Default for OAuthPollConfig {
    fn default() -> Self {
        Self {
            budget: Duration::from_secs(120),
            result_retention: Duration::from_secs(1800),
            max_inflight: 128,
        }
    }
}
impl OAuthPollConfig {
    fn validate(self) -> Result<Self, DownloadError> {
        if !(Duration::from_secs(61)..=Duration::from_secs(300)).contains(&self.budget)
            || !(Duration::from_secs(60)..=Duration::from_secs(86400))
                .contains(&self.result_retention)
            || !(1..=10000).contains(&self.max_inflight)
        {
            return Err(DownloadError::InvalidConfig);
        }
        Ok(self)
    }
}
#[derive(Default)]
pub(crate) struct OAuthWork {
    config: OAuthPollConfig,
    state: std::sync::Mutex<(bool, std::collections::HashSet<String>)>,
    tasks: tokio_util::task::TaskTracker,
}
struct PollGuard(Arc<OAuthWork>, String);
impl Drop for PollGuard {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().1.remove(&self.1);
    }
}
impl DownloadService {
    /// Configure before cloning/serving. Reconfiguration with active work is forbidden.
    pub fn with_cookie_oauth_poll_config(
        mut self,
        config: OAuthPollConfig,
    ) -> Result<Self, DownloadError> {
        let work = Arc::get_mut(&mut self.cookie_oauth_work).ok_or(DownloadError::InvalidConfig)?;
        work.config = config.validate()?;
        Ok(self)
    }
    /// Runtime may invoke periodically; start also performs this bounded expiry cleanup.
    /// Accepted work protects expiry only until its finite budget ends. No schema changes.
    pub async fn cleanup_cookie_oauth(&self, now: i64) -> Result<(), DownloadError> {
        query("DELETE FROM cookie_oauth_sessions WHERE expires_at<=?1")
            .bind(now)
            .execute(self.store.database())
            .await
            .map_err(db)?;
        Ok(())
    }
    pub fn with_cookie_oauth_provider(mut self, provider: Arc<dyn OAuthProvider>) -> Self {
        self.cookie_oauth_provider = Some(provider);
        self
    }
    fn oauth_provider(&self) -> Result<&Arc<dyn OAuthProvider>, DownloadError> {
        self.cookie_oauth_provider
            .as_ref()
            .ok_or(DownloadError::UnsupportedAuthentication)
    }
    pub async fn oauth_start(&self, actor: i64, now: i64) -> Result<DeviceStart, DownloadError> {
        {
            let mut conn = self.store.database().connection().await.map_err(db)?;
            let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
                .await
                .map_err(|_| DownloadError::Database)?;
            self.web_authorize(&tx, actor, now).await?;
            self.oauth_session_capacity(&tx, actor, now).await?;
            tx.commit().await.map_err(|_| DownloadError::Database)?;
        }
        let started = std::time::Instant::now();
        let device = tokio::time::timeout(
            self.oauth_provider()?.timeout(),
            self.oauth_provider()?.start(),
        )
        .await
        .map_err(|_| DownloadError::Executor)??;
        let now = now.saturating_add(started.elapsed().as_secs() as i64);
        let url = url::Url::parse(&device.verification_url).map_err(|_| DownloadError::Executor)?;
        if url.scheme() != "https"
            || !matches!(
                url.host_str(),
                Some(
                    "www.google.com"
                        | "google.com"
                        | "www.youtube.com"
                        | "youtube.com"
                        | "accounts.google.com"
                )
            )
            || !url.username().is_empty()
            || url.password().is_some()
            || device.device_code.is_empty()
            || device.device_code.len() > 4096
            || device.user_code.is_empty()
            || device.user_code.len() > 128
            || device.user_code.chars().any(char::is_control)
        {
            return Err(DownloadError::Executor);
        }
        let session_id = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let expires_in = device.expires_in.clamp(1, SESSION_TTL);
        let interval = device.interval.clamp(1, 60);
        let mut conn = self.store.database().connection().await.map_err(db)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        self.web_authorize(&tx, actor, now).await?;
        query("DELETE FROM cookie_oauth_sessions WHERE expires_at<=?1")
            .bind(now)
            .execute(&*tx)
            .await
            .map_err(db)?;
        self.oauth_session_capacity(&tx, actor, now).await?;
        query("INSERT INTO cookie_oauth_sessions(session_hash,user_id,device_code,expires_at,poll_interval,next_poll_at) VALUES(?1,?2,?3,?4,?5,?6)").bind(hash(&session_id)?).bind(actor).bind(device.device_code).bind(now.checked_add(expires_in).ok_or(DownloadError::InvalidInput)?).bind(interval).bind(now).execute(&*tx).await.map_err(db)?;
        tx.commit().await.map_err(|_| DownloadError::Database)?;
        Ok(DeviceStart {
            session_id,
            verification_url: device.verification_url,
            user_code: device.user_code,
            expires_in,
            interval,
        })
    }
    /// No new accepted polls after drain begins. Already accepted polls finish even
    /// when their request is cancelled. The runtime drain deadline must exceed
    /// the poll budget; forced runtime termination can still lose provider results.
    pub async fn drain_cookie_oauth(&self) {
        self.cookie_oauth_work.state.lock().unwrap().0 = true;
        self.cookie_oauth_work.tasks.close();
        self.cookie_oauth_work.tasks.wait().await;
    }
    pub async fn oauth_poll(
        &self,
        actor: i64,
        session: &str,
        now: i64,
    ) -> Result<DevicePoll, DownloadError> {
        let key = hash(session)?;
        let work = self.cookie_oauth_work.clone();
        let receive = {
            let mut state = work.state.lock().unwrap();
            if state.0 || state.1.len() >= work.config.max_inflight {
                return Err(DownloadError::Capacity);
            }
            // Reject overlap rather than reporting pending to a foreign caller.
            if !state.1.insert(key.clone()) {
                return Err(DownloadError::ChangedCredentials);
            }
            let guard = PollGuard(work.clone(), key);
            let service = self.clone();
            let session = session.to_owned();
            let (send, receive) = tokio::sync::oneshot::channel();
            work.tasks.spawn(async move {
                let _guard = guard;
                let result = tokio::time::timeout(
                    service.cookie_oauth_work.config.budget,
                    service.oauth_poll_owned(actor, &session, now),
                )
                .await
                .unwrap_or(Err(DownloadError::Executor));
                let _ = send.send(result);
            });
            receive
        };
        receive.await.map_err(|_| DownloadError::Executor)?
    }
    async fn oauth_poll_owned(
        &self,
        actor: i64,
        session: &str,
        now: i64,
    ) -> Result<DevicePoll, DownloadError> {
        let started = std::time::Instant::now();
        let lease_until =
            now.saturating_add(self.cookie_oauth_work.config.budget.as_secs() as i64 + 1);
        let session_hash = hash(session)?;
        let (code, generation, interval, ready, expires) = {
            let mut conn = self.store.database().connection().await.map_err(db)?;
            let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
                .await
                .map_err(|_| DownloadError::Database)?;
            let now = now.saturating_add(started.elapsed().as_secs() as i64);
            self.web_authorize(&tx, actor, now).await?;
            let row=query("SELECT user_id,device_code,expires_at,next_poll_at,poll_interval,generation,ready_content FROM cookie_oauth_sessions WHERE session_hash=?1").bind(&session_hash).fetch_optional(&*tx).await.map_err(db)?;
            let Some(row) = row else {
                tx.commit().await.map_err(|_| DownloadError::Database)?;
                return Ok(DevicePoll {
                    status: "expired",
                    detail: None,
                });
            };
            if row.try_get::<i64, _>("user_id").map_err(db)? != actor {
                return Err(DownloadError::Denied);
            }
            if row.try_get::<i64, _>("expires_at").map_err(db)? <= now {
                query("DELETE FROM cookie_oauth_sessions WHERE session_hash=?1")
                    .bind(&session_hash)
                    .execute(&*tx)
                    .await
                    .map_err(db)?;
                tx.commit().await.map_err(|_| DownloadError::Database)?;
                return Ok(DevicePoll {
                    status: "expired",
                    detail: None,
                });
            }
            let ready: Option<String> = row.try_get("ready_content").map_err(db)?;
            if ready.is_none() && row.try_get::<i64, _>("next_poll_at").map_err(db)? > now {
                tx.commit().await.map_err(|_| DownloadError::Database)?;
                return Ok(DevicePoll {
                    status: "pending",
                    detail: None,
                });
            }
            let expires = row.try_get::<i64, _>("expires_at").map_err(db)?;
            let generation = row.try_get::<i64, _>("generation").map_err(db)? + 1;
            let interval = row.try_get::<i64, _>("poll_interval").map_err(db)?;
            query("UPDATE cookie_oauth_sessions SET generation=?2,next_poll_at=?3,expires_at=CASE WHEN ready_content IS NULL THEN MAX(expires_at,?4) ELSE expires_at END WHERE session_hash=?1").bind(&session_hash).bind(generation).bind(lease_until).bind(lease_until).execute(&*tx).await.map_err(db)?;
            let code: String = row.try_get("device_code").map_err(db)?;
            tx.commit().await.map_err(|_| DownloadError::Database)?;
            (code, generation, interval, ready, expires)
        };
        let outcome = if let Some(ready) = ready {
            ProviderPoll::Ready(OAuthCredential::decode(&ready)?)
        } else {
            match tokio::time::timeout(
                self.oauth_provider()?.timeout(),
                self.oauth_provider()?.poll(&code, now),
            )
            .await
            {
                Ok(Ok(v)) => v,
                _ => ProviderPoll::Error,
            }
        };
        let mut conn = self.store.database().connection().await.map_err(db)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        let now = now.saturating_add(started.elapsed().as_secs() as i64);
        self.web_authorize(&tx, actor, now).await?;
        let current=query("SELECT generation,ready_content,expires_at FROM cookie_oauth_sessions WHERE session_hash=?1 AND user_id=?2 AND expires_at>?3").bind(&session_hash).bind(actor).bind(now).fetch_optional(&*tx).await.map_err(db)?.ok_or(DownloadError::ChangedCredentials)?;
        let staged: Option<String> = current.try_get("ready_content").map_err(db)?;
        // Ready is not generation-fenced: consuming a one-shot provider response
        // must win over a later Pending/Error, but never over a staged success.
        if !matches!(outcome, ProviderPoll::Ready(_))
            && (current.try_get::<i64, _>("generation").map_err(db)? != generation
                || staged.is_some())
        {
            return Err(DownloadError::ChangedCredentials);
        }
        // Release temporary expiry protection for non-success results. Success
        // receives a finite retry window even if the device expired in flight.
        query("UPDATE cookie_oauth_sessions SET expires_at=?2 WHERE session_hash=?1")
            .bind(&session_hash)
            .bind(
                if matches!(outcome, ProviderPoll::Ready(_)) && staged.is_none() {
                    now.saturating_add(
                        self.cookie_oauth_work.config.result_retention.as_secs() as i64
                    )
                } else if staged.is_some() {
                    current.try_get::<i64, _>("expires_at").map_err(db)?
                } else {
                    expires
                },
            )
            .execute(&*tx)
            .await
            .map_err(db)?;
        let result = match outcome {
            ProviderPoll::Pending | ProviderPoll::SlowDown => {
                let interval = if matches!(outcome, ProviderPoll::SlowDown) {
                    (interval + 5).min(60)
                } else {
                    interval
                };
                query("UPDATE cookie_oauth_sessions SET poll_interval=?2,next_poll_at=?3 WHERE session_hash=?1").bind(&session_hash).bind(interval).bind(now+interval).execute(&*tx).await.map_err(db)?;
                DevicePoll {
                    status: "pending",
                    detail: None,
                }
            }
            ProviderPoll::Expired => {
                query("DELETE FROM cookie_oauth_sessions WHERE session_hash=?1")
                    .bind(&session_hash)
                    .execute(&*tx)
                    .await
                    .map_err(db)?;
                DevicePoll {
                    status: "expired",
                    detail: None,
                }
            }
            ProviderPoll::Error => {
                query("UPDATE cookie_oauth_sessions SET next_poll_at=?2 WHERE session_hash=?1")
                    .bind(&session_hash)
                    .bind(now + interval)
                    .execute(&*tx)
                    .await
                    .map_err(db)?;
                DevicePoll {
                    status: "error",
                    detail: Some("Provider authentication failed"),
                }
            }
            ProviderPoll::Ready(credential) => {
                let content = match staged {
                    Some(content) => content,
                    None => credential.encode()?,
                };
                query("UPDATE cookie_oauth_sessions SET ready_content=COALESCE(ready_content,?2) WHERE session_hash=?1")
                    .bind(&session_hash)
                    .bind(&content)
                    .execute(&*tx)
                    .await
                    .map_err(db)?;
                if !self.oauth_capacity(&tx, actor, &content).await? {
                    tx.commit().await.map_err(|_| DownloadError::Database)?;
                    return Err(DownloadError::PersonalCookieCapacity);
                }
                self.oauth_store(&tx, actor, &content, now).await?;
                query("DELETE FROM cookie_oauth_sessions WHERE session_hash=?1")
                    .bind(&session_hash)
                    .execute(&*tx)
                    .await
                    .map_err(db)?;
                DevicePoll {
                    status: "ok",
                    detail: None,
                }
            }
        };
        tx.commit().await.map_err(|_| DownloadError::Database)?;
        Ok(result)
    }
    async fn oauth_capacity(
        &self,
        tx: &Transaction<'_>,
        actor: i64,
        content: &str,
    ) -> Result<bool, DownloadError> {
        let row = query(include_str!("personal_cookies_usage.sql"))
            .bind(actor)
            .bind("youtube.com")
            .fetch_one(&**tx)
            .await
            .map_err(db)?;
        let v = |k| row.try_get::<i64, _>(k).map_err(db);
        let l = self.network_config.personal_cookie_limits;
        let exceeds = |old: i64, new: i64, limit: u64| new > old && new as u64 > limit;
        Ok(!(exceeds(
            v("actor_domains")?,
            v("actor_domains")? + i64::from(v("replaced_domains")? == 0),
            l.domains,
        ) || exceeds(
            v("actor_bytes")?,
            v("actor_bytes")? - v("replaced_bytes")?
                + content.len() as i64 * v("replaced_domains")?.max(1),
            l.actor_bytes,
        ) || exceeds(
            v("global_bytes")?,
            v("global_bytes")? - v("replaced_bytes")?
                + content.len() as i64 * v("replaced_domains")?.max(1),
            l.global_bytes,
        )))
    }
    async fn oauth_store(
        &self,
        tx: &Transaction<'_>,
        actor: i64,
        content: &str,
        now: i64,
    ) -> Result<(), DownloadError> {
        let timestamp = DateTime::from_timestamp(now, 0).ok_or(DownloadError::InvalidInput)?;
        let updated=query("UPDATE cookies SET content=?2,is_valid=1,updated_at=?3,validated_at=NULL,content_hash=NULL WHERE user_id=?1 AND is_pool=0 AND LOWER(TRIM(domain,'.'))='youtube.com'").bind(actor).bind(content).bind(timestamp).execute(&**tx).await.map_err(db)?;
        if updated.rows_affected() == 0 {
            query("INSERT INTO cookies(user_id,domain,content,is_valid,is_pool,created_at,updated_at) VALUES(?1,'youtube.com',?2,1,0,?3,?3)").bind(actor).bind(content).bind(timestamp).execute(&**tx).await.map_err(db)?;
        }
        Ok(())
    }
    /// Imported and newly stored personal __oauth2__ contents share one typed contract.
    /// Only the owning actor can resolve secrets; no HTTP endpoint returns tokens.
    pub async fn oauth_credential(
        &self,
        actor: i64,
        now: i64,
    ) -> Result<Option<(OAuthReference, OAuthCredential)>, DownloadError> {
        let mut conn = self.store.database().connection().await.map_err(db)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        self.web_authorize(&tx, actor, now).await?;
        let row=query("SELECT id,domain,content,updated_at FROM cookies WHERE user_id=?1 AND is_pool=0 AND is_valid=1 AND LOWER(TRIM(domain,'.'))='youtube.com' ORDER BY id LIMIT 1").bind(actor).fetch_optional(&*tx).await.map_err(db)?;
        let result = if let Some(row) = row {
            let content: String = row.try_get("content").map_err(db)?;
            if content.starts_with(PREFIX) {
                Some((
                    reference(
                        row.try_get("id").map_err(db)?,
                        actor,
                        &row.try_get::<String, _>("domain").map_err(db)?,
                        &content,
                        row.try_get("updated_at").map_err(db)?,
                    )?,
                    OAuthCredential::decode(&content)?,
                ))
            } else {
                None
            }
        } else {
            None
        };
        tx.commit().await.map_err(|_| DownloadError::Database)?;
        Ok(result)
    }
    /// Refresh outside the DB lock, then CAS the *full* content/version and recheck
    /// current actor authority. A stale provider result cannot overwrite newer credentials.
    pub async fn oauth_refresh(
        &self,
        actor: i64,
        expected: &OAuthReference,
        now: i64,
    ) -> Result<OAuthReference, DownloadError> {
        let (current, credential) = self
            .oauth_credential(actor, now)
            .await?
            .ok_or(DownloadError::ChangedCredentials)?;
        if &current != expected {
            return Err(DownloadError::ChangedCredentials);
        }
        let started = std::time::Instant::now();
        let refreshed = tokio::time::timeout(
            self.oauth_provider()?.timeout(),
            self.oauth_provider()?.refresh(&credential, now),
        )
        .await
        .map_err(|_| DownloadError::Executor)??;
        let content = refreshed.encode()?;
        let now = now.saturating_add(started.elapsed().as_secs() as i64);
        let mut conn = self.store.database().connection().await.map_err(db)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        self.web_authorize(&tx, actor, now).await?;
        let row=query("SELECT domain,content,updated_at FROM cookies WHERE id=?1 AND user_id=?2 AND is_pool=0 AND is_valid=1").bind(i64::from(current.cookie.id())).bind(actor).fetch_optional(&*tx).await.map_err(db)?.ok_or(DownloadError::ChangedCredentials)?;
        if reference(
            i64::from(current.cookie.id()),
            actor,
            &row.try_get::<String, _>("domain").map_err(db)?,
            &row.try_get::<String, _>("content").map_err(db)?,
            row.try_get("updated_at").map_err(db)?,
        )? != current
        {
            return Err(DownloadError::ChangedCredentials);
        }
        if !self.oauth_capacity(&tx, actor, &content).await? {
            return Err(DownloadError::PersonalCookieCapacity);
        }
        self.oauth_store(&tx, actor, &content, now).await?;
        tx.commit().await.map_err(|_| DownloadError::Database)?;
        reference(
            i64::from(current.cookie.id()),
            actor,
            &row.try_get::<String, _>("domain").map_err(db)?,
            &content,
            DateTime::from_timestamp(now, 0).ok_or(DownloadError::InvalidInput)?,
        )
    }
}
impl DownloadService {
    /// Execution integration convenience: same existing CookieReference in/out.
    /// Invoke before admission; never refresh the credentials of an admitted job.
    pub async fn oauth_refresh_reference(
        &self,
        actor: i64,
        expected: &CookieReference,
        now: i64,
    ) -> Result<CookieReference, DownloadError> {
        self.oauth_refresh(
            actor,
            &OAuthReference {
                cookie: expected.clone(),
                kind: CREDENTIAL_KIND,
            },
            now,
        )
        .await
        .map(|r| r.cookie)
    }
}
impl DownloadService {
    async fn oauth_session_capacity(
        &self,
        tx: &Transaction<'_>,
        actor: i64,
        now: i64,
    ) -> Result<(), DownloadError> {
        let usage=query("SELECT count(*) AS total,COALESCE(SUM(user_id=?1),0) AS actor_count FROM cookie_oauth_sessions WHERE expires_at>?2").bind(actor).bind(now).fetch_one(&**tx).await.map_err(db)?;
        if usage.try_get::<i64, _>("total").map_err(db)? >= 10000
            || usage.try_get::<i64, _>("actor_count").map_err(db)? >= 8
        {
            return Err(DownloadError::Capacity);
        }
        Ok(())
    }
}
