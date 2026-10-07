//! Cookie web contracts. Credentials are never Debug, and token consumption shares
//! the immediate transaction with *all* cookie writes and capacity checks.
use crate::{CookieDomain, DownloadError, DownloadService, profile::valid_cookie};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use turso::transaction::{Transaction, TransactionBehavior};
use yoink_db::query;

pub const TOKEN_TTL: i64 = 600;
pub const SCHEMA: &str = include_str!("../../yoink-storage/migrations-turso/0019_cookie_web.sql");
#[derive(Serialize)]
pub struct CookieToken {
    pub token: String,
    pub expires_in: i64,
    pub submit_url: &'static str,
}
#[derive(Deserialize)]
pub struct BrowserCookie {
    pub domain: String,
    pub path: String,
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub secure: bool,
    #[serde(default, rename = "httpOnly")]
    pub http_only: bool,
    #[serde(default, rename = "expirationDate")]
    pub expiration: Option<f64>,
}
#[derive(Deserialize)]
pub struct CookieSubmit {
    pub token: String,
    pub cookies: BTreeMap<String, Vec<BrowserCookie>>,
}
#[derive(Deserialize)]
pub struct CookieInput {
    pub domain: String,
    pub content: String,
}
#[derive(Serialize)]
pub struct CookieMetadata {
    pub id: i64,
    pub user_id: i64,
    pub domain: String,
    pub is_valid: bool,
    pub is_pool: bool,
    pub is_oauth: bool,
    pub label: Option<String>,
    pub avatar_url: Option<String>,
    pub validated_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub inherited: bool,
}
fn db(_: yoink_db::Error) -> DownloadError {
    DownloadError::Database
}
pub(super) fn hash(token: &str) -> Result<String, DownloadError> {
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(DownloadError::Denied);
    }
    Ok(format!("{:x}", Sha256::digest(token.as_bytes())))
}
pub(super) fn validate(input: &CookieInput) -> Result<CookieDomain, DownloadError> {
    let domain = CookieDomain::parse(&input.domain)?;
    if input.content.len() > yoink_worker_protocol::MAX_COOKIE_JAR_BYTES
        || !valid_cookie(&input.content)
    {
        return Err(DownloadError::InvalidInput);
    }
    for line in input
        .content
        .lines()
        .filter(|l| l.starts_with("#HttpOnly_") || (!l.starts_with('#') && !l.is_empty()))
    {
        let host = line
            .split('\t')
            .next()
            .unwrap_or("")
            .trim_start_matches("#HttpOnly_");
        let host = CookieDomain::parse(host)?;
        if host.as_str() != domain.as_str()
            && !host.as_str().ends_with(&format!(".{}", domain.as_str()))
        {
            return Err(DownloadError::InvalidInput);
        }
    }
    Ok(domain)
}
impl CookieSubmit {
    pub fn inputs(&self) -> Result<Vec<CookieInput>, DownloadError> {
        if self.cookies.is_empty() || self.cookies.len() > 64 {
            return Err(DownloadError::InvalidInput);
        }
        let mut result = vec![];
        let mut total = 0usize;
        for (domain, cookies) in &self.cookies {
            if cookies.is_empty() || cookies.len() > 4096 {
                return Err(DownloadError::InvalidInput);
            }
            let mut content = String::from("# Netscape HTTP Cookie File\n");
            for c in cookies {
                if [&c.domain, &c.path, &c.name, &c.value]
                    .iter()
                    .any(|s| s.chars().any(|ch| ch.is_control()))
                    || !c.path.starts_with('/')
                    || c.name.is_empty()
                {
                    return Err(DownloadError::InvalidInput);
                }
                let exp = c.expiration.filter(|v| *v != 0.0).unwrap_or(2147483647.0);
                if !exp.is_finite() || exp < 0.0 || exp > i64::MAX as f64 {
                    return Err(DownloadError::InvalidInput);
                }
                content.push_str(&format!(
                    "{}.{}\tTRUE\t{}\t{}\t{}\t{}\t{}\n",
                    if c.http_only { "#HttpOnly_" } else { "" },
                    c.domain.trim_start_matches('.'),
                    c.path,
                    if c.secure { "TRUE" } else { "FALSE" },
                    exp as i64,
                    c.name,
                    c.value
                ));
                if content.len() > yoink_worker_protocol::MAX_COOKIE_JAR_BYTES {
                    return Err(DownloadError::InvalidInput);
                }
            }
            total += content.len();
            if total > 2 * yoink_worker_protocol::MAX_COOKIE_JAR_BYTES {
                return Err(DownloadError::InvalidInput);
            }
            let input = CookieInput {
                domain: domain.clone(),
                content,
            };
            validate(&input)?;
            result.push(input);
        }
        Ok(result)
    }
}
impl DownloadService {
    /// Explicit schema hook for disposable tests; production migration belongs to coordinator.
    pub async fn install_cookie_web_schema(&self) -> Result<(), DownloadError> {
        yoink_db::raw_sql(SCHEMA)
            .execute(self.store.database())
            .await
            .map_err(db)?;
        Ok(())
    }
    pub async fn issue_cookie_token(
        &self,
        actor: i64,
        now: i64,
    ) -> Result<CookieToken, DownloadError> {
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let mut conn = self.store.database().connection().await.map_err(db)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        self.web_authorize(&tx, actor, now).await?;
        query("DELETE FROM cookie_web_tokens WHERE expires_at<=?1")
            .bind(now)
            .execute(&*tx)
            .await
            .map_err(db)?;
        let usage=query("SELECT count(*) AS total,COALESCE(SUM(user_id=?1),0) AS actor_count FROM cookie_web_tokens").bind(actor).fetch_one(&*tx).await.map_err(db)?;
        if usage.try_get::<i64, _>("total").map_err(db)? >= 100000
            || usage.try_get::<i64, _>("actor_count").map_err(db)? >= 128
        {
            return Err(DownloadError::Capacity);
        }
        query("INSERT INTO cookie_web_tokens(token_hash,user_id,expires_at,created_at) VALUES(?1,?2,?3,?4)").bind(hash(&token)?).bind(actor).bind(now.checked_add(TOKEN_TTL).ok_or(DownloadError::InvalidInput)?).bind(now).execute(&*tx).await.map_err(db)?;
        tx.commit().await.map_err(|_| DownloadError::Database)?;
        Ok(CookieToken {
            token,
            expires_in: TOKEN_TTL,
            submit_url: "/api/v1/dl/cookies/submit",
        })
    }
    pub async fn revoke_cookie_tokens(&self, actor: i64, now: i64) -> Result<(), DownloadError> {
        let mut conn = self.store.database().connection().await.map_err(db)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        self.web_authorize(&tx, actor, now).await?;
        query("DELETE FROM cookie_web_tokens WHERE user_id=?1")
            .bind(actor)
            .execute(&*tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(|_| DownloadError::Database)
    }
    pub(super) async fn web_authorize(
        &self,
        tx: &Transaction<'_>,
        actor: i64,
        now: i64,
    ) -> Result<(), DownloadError> {
        if actor <= 0 {
            return Err(DownloadError::Denied);
        }
        let now = DateTime::from_timestamp(now, 0).ok_or(DownloadError::InvalidInput)?;
        if query(include_str!("personal_cookies_auth.sql"))
            .bind(actor)
            .bind(now)
            .bind(self.identities.is_configured_owner(actor))
            .fetch_optional(&**tx)
            .await
            .map_err(db)?
            .is_none()
        {
            return Err(DownloadError::Denied);
        }
        Ok(())
    }
    pub async fn submit_cookie_token(
        &self,
        body: &CookieSubmit,
        now: i64,
    ) -> Result<(), DownloadError> {
        let inputs = body.inputs()?;
        self.web_write(None, Some(&body.token), &inputs, now).await
    }
    pub async fn upload_web_cookie(
        &self,
        actor: i64,
        input: &CookieInput,
        now: i64,
    ) -> Result<(), DownloadError> {
        let domain = validate(input)?;
        if crate::cookie_web_account::label(domain.as_str(), &input.content).is_none() {
            return Err(DownloadError::InvalidInput);
        }
        self.web_write(Some(actor), None, std::slice::from_ref(input), now)
            .await
    }
    async fn web_write(
        &self,
        actor: Option<i64>,
        token: Option<&str>,
        inputs: &[CookieInput],
        now: i64,
    ) -> Result<(), DownloadError> {
        let domains = inputs.iter().map(validate).collect::<Result<Vec<_>, _>>()?;
        let unique = domains
            .iter()
            .map(|d| d.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        if unique.len() != domains.len() {
            return Err(DownloadError::InvalidInput);
        }
        let mut conn = self.store.database().connection().await.map_err(db)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        let actor = if let Some(token) = token {
            query("SELECT user_id FROM cookie_web_tokens WHERE token_hash=?1 AND expires_at>?2")
                .bind(hash(token)?)
                .bind(now)
                .fetch_optional(&*tx)
                .await
                .map_err(db)?
                .ok_or(DownloadError::Denied)?
                .try_get::<i64, _>("user_id")
                .map_err(db)?
        } else {
            actor.ok_or(DownloadError::Denied)?
        };
        self.web_authorize(&tx, actor, now).await?;
        for (input, domain) in inputs.iter().zip(domains) {
            let usage = query(include_str!("personal_cookies_usage.sql"))
                .bind(actor)
                .bind(domain.as_str())
                .fetch_one(&*tx)
                .await
                .map_err(db)?;
            let v = |key| usage.try_get::<i64, _>(key).map_err(db);
            let limits = self.network_config.personal_cookie_limits;
            let exceeds = |old: i64, new: i64, limit: u64| new > old && new as u64 > limit;
            if exceeds(
                v("actor_domains")?,
                v("actor_domains")? + i64::from(v("replaced_domains")? == 0),
                limits.domains,
            ) || exceeds(
                v("actor_bytes")?,
                v("actor_bytes")? - v("replaced_bytes")?
                    + input.content.len() as i64 * v("replaced_domains")?.max(1),
                limits.actor_bytes,
            ) || exceeds(
                v("global_bytes")?,
                v("global_bytes")? - v("replaced_bytes")?
                    + input.content.len() as i64 * v("replaced_domains")?.max(1),
                limits.global_bytes,
            ) {
                return Err(DownloadError::PersonalCookieCapacity);
            }
            let timestamp = DateTime::from_timestamp(now, 0).ok_or(DownloadError::InvalidInput)?;
            // Preserve imported row IDs, labels, avatars and created_at on replacement.
            let updated=query("UPDATE cookies SET content=?3,is_valid=?5,updated_at=?4,validated_at=NULL,content_hash=NULL WHERE user_id=?1 AND is_pool=0 AND LOWER(TRIM(domain,'.'))=?2").bind(actor).bind(domain.as_str()).bind(&input.content).bind(timestamp).bind(crate::cookie_web_account::label(domain.as_str(),&input.content).is_some()).execute(&*tx).await.map_err(db)?;
            if updated.rows_affected() == 0 {
                query(include_str!("cookie_web_insert.sql"))
                    .bind(actor)
                    .bind(domain.as_str())
                    .bind(&input.content)
                    .bind(timestamp)
                    .bind(
                        crate::cookie_web_account::label(domain.as_str(), &input.content).is_some(),
                    )
                    .execute(&*tx)
                    .await
                    .map_err(db)?;
            }
        }
        if let Some(token) = token {
            query("DELETE FROM cookie_web_tokens WHERE token_hash=?1")
                .bind(hash(token)?)
                .execute(&*tx)
                .await
                .map_err(db)?;
        }
        tx.commit().await.map_err(|_| DownloadError::Database)
    }
}
impl DownloadService {
    pub async fn web_cookie_list(
        &self,
        actor: i64,
        now: i64,
    ) -> Result<Vec<CookieMetadata>, DownloadError> {
        self.web_cookie_scoped_list(actor, 0, now).await
    }
    pub async fn web_cookie_scoped_list(
        &self,
        actor: i64,
        scope: i64,
        now: i64,
    ) -> Result<Vec<CookieMetadata>, DownloadError> {
        if !(0..=2).contains(&scope) {
            return Err(DownloadError::InvalidInput);
        }
        let mut conn = self.store.database().connection().await.map_err(db)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        self.web_authorize(&tx, actor, now).await?;
        let admin = self.web_admin(&tx, actor).await?;
        if scope != 0 && !admin {
            return Err(DownloadError::Denied);
        }
        let shared=admin || query("SELECT 1 FROM user_permissions WHERE user_id=?1 AND plugin='dl' AND feature='shared_cookies' AND (expires_at IS NULL OR expires_at>?2)").bind(actor).bind(DateTime::from_timestamp(now,0).ok_or(DownloadError::InvalidInput)?).fetch_optional(&*tx).await.map_err(db)?.is_some();
        let rows = query(include_str!("cookie_web_list.sql"))
            .bind(actor)
            .bind(scope)
            .bind(shared)
            .fetch_all(&*tx)
            .await
            .map_err(db)?;
        let result = rows
            .into_iter()
            .map(|r| {
                Ok(CookieMetadata {
                    id: r.try_get("id").map_err(db)?,
                    user_id: r.try_get("user_id").map_err(db)?,
                    domain: r.try_get("domain").map_err(db)?,
                    is_valid: r.try_get("is_valid").map_err(db)?,
                    is_pool: r.try_get("is_pool").map_err(db)?,
                    is_oauth: r.try_get("is_oauth").map_err(db)?,
                    label: r.try_get("label").map_err(db)?,
                    avatar_url: r.try_get("avatar_url").map_err(db)?,
                    validated_at: r.try_get("validated_at").map_err(db)?,
                    created_at: r.try_get("created_at").map_err(db)?,
                    updated_at: r.try_get("updated_at").map_err(db)?,
                    inherited: r.try_get("inherited").map_err(db)?,
                })
            })
            .collect();
        tx.commit().await.map_err(|_| DownloadError::Database)?;
        result
    }
    pub async fn web_cookie_delete(
        &self,
        actor: i64,
        id: Option<i64>,
        domain: Option<&str>,
        now: i64,
    ) -> Result<bool, DownloadError> {
        let domain = domain.map(CookieDomain::parse).transpose()?;
        let mut conn = self.store.database().connection().await.map_err(db)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        self.web_authorize(&tx, actor, now).await?;
        let admin = id.is_some() && self.web_admin(&tx, actor).await?;
        let count=query("DELETE FROM cookies WHERE ((user_id=?1 AND is_pool=0) OR ?4=1) AND ((?2 IS NOT NULL AND id=?2) OR (?3 IS NOT NULL AND LOWER(TRIM(domain,'.'))=?3))").bind(actor).bind(id).bind(domain.as_ref().map(|d|d.as_str())).bind(admin).execute(&*tx).await.map_err(db)?.rows_affected();
        tx.commit().await.map_err(|_| DownloadError::Database)?;
        Ok(count > 0)
    }
}
impl DownloadService {
    pub fn with_cookie_web_origin(mut self, origin: &str) -> Result<Self, DownloadError> {
        let url = url::Url::parse(origin).map_err(|_| DownloadError::InvalidConfig)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(DownloadError::InvalidConfig);
        }
        self.cookie_web_origin = Some(url);
        Ok(self)
    }
    pub fn cookie_web_link(&self, path: &str) -> Option<url::Url> {
        self.cookie_web_origin.as_ref()?.join(path).ok()
    }
    pub async fn issue_telegram_cookie_token(
        &self,
        actor: u64,
        target: yoink_domain::DeliveryTarget,
    ) -> Result<CookieToken, DownloadError> {
        self.authorize_cookie_control(actor, target).await?;
        self.issue_cookie_token(actor as i64, Utc::now().timestamp())
            .await
    }
}
impl DownloadService {
    pub(super) async fn web_admin(
        &self,
        tx: &Transaction<'_>,
        actor: i64,
    ) -> Result<bool, DownloadError> {
        Ok(self.identities.is_configured_owner(actor)
            || query("SELECT 1 FROM users WHERE id=?1 AND role IN ('admin','owner')")
                .bind(actor)
                .fetch_optional(&**tx)
                .await
                .map_err(db)?
                .is_some())
    }
    pub async fn web_pool_delete(
        &self,
        actor: i64,
        id: i64,
        now: i64,
    ) -> Result<bool, DownloadError> {
        let mut conn = self.store.database().connection().await.map_err(db)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        self.web_authorize(&tx, actor, now).await?;
        if !self.web_admin(&tx, actor).await? {
            return Err(DownloadError::Denied);
        }
        let count = query("DELETE FROM cookies WHERE id=?1 AND is_pool=1")
            .bind(id)
            .execute(&*tx)
            .await
            .map_err(db)?
            .rows_affected();
        tx.commit().await.map_err(|_| DownloadError::Database)?;
        Ok(count > 0)
    }
}
impl DownloadService {
    pub async fn web_pool_upload(
        &self,
        actor: i64,
        input: &CookieInput,
        now: i64,
    ) -> Result<i64, DownloadError> {
        let domain = validate(input)?;
        let content_hash = format!("{:x}", Sha256::digest(input.content.as_bytes()));
        let session = crate::cookie_web_account::session(domain.as_str(), &input.content);
        let raw_session = crate::cookie_web_account::raw_session(domain.as_str(), &input.content);
        self.web_admin_preflight(actor, now).await?;
        let started = std::time::Instant::now();
        let fetched = if let Some(provider) = &self.cookie_account_provider {
            tokio::time::timeout(
                provider.timeout(),
                provider.fetch(domain.as_str(), &input.content, now),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|r| r.info)
        } else {
            None
        };
        let now = now.saturating_add(started.elapsed().as_secs() as i64);
        let avatar = fetched.as_ref().and_then(|i| i.avatar_url.as_deref());
        let label = fetched
            .as_ref()
            .map(|i| i.label.clone())
            .or_else(|| crate::cookie_web_account::label(domain.as_str(), &input.content));
        let timestamp = DateTime::from_timestamp(now, 0).ok_or(DownloadError::InvalidInput)?;
        let mut conn = self.store.database().connection().await.map_err(db)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        self.web_authorize(&tx, actor, now).await?;
        if !self.web_admin(&tx, actor).await? {
            return Err(DownloadError::Denied);
        }
        let row=query("SELECT id,content FROM cookies WHERE user_id=?1 AND domain=?2 AND is_pool=1 AND ((?3 IS NOT NULL AND (session_key=?3 OR session_key=?5)) OR content_hash=?4) ORDER BY id LIMIT 1").bind(actor).bind(domain.as_str()).bind(session.as_deref()).bind(&content_hash).bind(raw_session.as_deref()).fetch_optional(&*tx).await.map_err(db)?;
        let id = if let Some(row) = row {
            let old: String = row.try_get("content").map_err(db)?;
            if input.content.len() > old.len() {
                let bytes=query("SELECT COALESCE(SUM(length(CAST(content AS BLOB))),0) AS n FROM cookies WHERE is_pool=1").fetch_one(&*tx).await.map_err(db)?.try_get::<i64,_>("n").map_err(db)?;
                if bytes + (input.content.len() - old.len()) as i64 > 1024 * 1024 * 1024 {
                    return Err(DownloadError::PersonalCookieCapacity);
                }
            }
            let id: i64 = row.try_get("id").map_err(db)?;
            query("UPDATE cookies SET content=?2,content_hash=?3,session_key=?4,is_valid=1,updated_at=?5,validated_at=NULL,label=COALESCE(?6,label),avatar_url=COALESCE(?7,avatar_url) WHERE id=?1").bind(id).bind(&input.content).bind(&content_hash).bind(session.as_deref()).bind(timestamp).bind(label.as_deref()).bind(avatar).execute(&*tx).await.map_err(db)?;
            id
        } else {
            // Finite pool bound, independent of personal limits. Imported over-limit
            // records may be deleted or replaced but never grow the over-limit pool.
            let usage=query("SELECT COALESCE(SUM(length(CAST(content AS BLOB))),0) AS n FROM cookies WHERE is_pool=1").fetch_one(&*tx).await.map_err(db)?.try_get::<i64,_>("n").map_err(db)?;
            if usage + input.content.len() as i64 > 1024 * 1024 * 1024 {
                return Err(DownloadError::PersonalCookieCapacity);
            }
            query("INSERT INTO cookies(user_id,domain,content,is_valid,is_pool,content_hash,session_key,label,created_at,updated_at,avatar_url) VALUES(?1,?2,?3,1,1,?4,?5,?6,?7,?7,?8) RETURNING id").bind(actor).bind(domain.as_str()).bind(&input.content).bind(&content_hash).bind(session.as_deref()).bind(label.as_deref()).bind(timestamp).bind(avatar).fetch_one(&*tx).await.map_err(db)?.try_get("id").map_err(db)?
        };
        tx.commit().await.map_err(|_| DownloadError::Database)?;
        Ok(id)
    }
}
impl DownloadService {
    pub(super) async fn web_admin_preflight(
        &self,
        actor: i64,
        now: i64,
    ) -> Result<(), DownloadError> {
        let mut conn = self.store.database().connection().await.map_err(db)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        self.web_authorize(&tx, actor, now).await?;
        if !self.web_admin(&tx, actor).await? {
            return Err(DownloadError::Denied);
        }
        tx.commit().await.map_err(|_| DownloadError::Database)
    }
}
