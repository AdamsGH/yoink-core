//! Source-backed YouTube/Instagram/Twitter live account validation and token rotation.
use crate::{
    DownloadError, DownloadService,
    cookie_oauth::ProviderFuture,
    cookie_web::{CookieInput, validate},
    cookie_web_account::{label, values},
};
use chrono::{DateTime, Utc};
use sha1::{Digest, Sha1};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use turso::transaction::{Transaction, TransactionBehavior};
use yoink_db::query;
pub struct AccountInfo {
    pub label: String,
    pub avatar_url: Option<String>,
}
pub struct AccountResult {
    pub info: Option<AccountInfo>,
    pub updates: BTreeMap<String, String>,
}
pub trait AccountProvider: Send + Sync {
    fn timeout(&self) -> Duration {
        Duration::from_secs(10)
    }
    fn fetch<'a>(
        &'a self,
        domain: &'a str,
        content: &'a str,
        now: i64,
    ) -> ProviderFuture<'a, AccountResult>;
}
pub struct LiveAccountProvider {
    timeout: Duration,
    client: reqwest::Client,
}
impl LiveAccountProvider {
    pub fn new() -> Result<Self, DownloadError> {
        Self::with_config(AccountProviderConfig::default())
    }
    pub fn with_config(config: AccountProviderConfig) -> Result<Self, DownloadError> {
        Ok(Self {
            timeout: config.timeout(),
            client: crate::cookie_web_network::client(config.timeout())?,
        })
    }
}
fn site(d: &str, b: &str) -> bool {
    d == b || d.ends_with(&format!(".{b}"))
}
fn find<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    match v {
        serde_json::Value::Object(m) => {
            m.get(key).or_else(|| m.values().find_map(|v| find(v, key)))
        }
        serde_json::Value::Array(a) => a.iter().find_map(|v| find(v, key)),
        _ => None,
    }
}
impl AccountProvider for LiveAccountProvider {
    fn timeout(&self) -> Duration {
        self.timeout
    }
    fn fetch<'a>(
        &'a self,
        domain: &'a str,
        content: &'a str,
        now: i64,
    ) -> ProviderFuture<'a, AccountResult> {
        Box::pin(async move {
            let cookies = values(content);
            let header = cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ");
            let youtube = site(domain, "youtube.com") || site(domain, "google.com");
            let instagram = site(domain, "instagram.com");
            let twitter = site(domain, "twitter.com") || site(domain, "x.com");
            let empty = || AccountResult {
                info: None,
                updates: BTreeMap::new(),
            };
            let mut req = if youtube {
                let Some(sapisid) = cookies
                    .get("SAPISID")
                    .or_else(|| cookies.get("__Secure-3PAPISID"))
                else {
                    return Ok(empty());
                };
                let digest = format!(
                    "{:x}",
                    Sha1::digest(format!("{now} {sapisid} https://www.youtube.com").as_bytes())
                );
                self.client.post("https://www.youtube.com/youtubei/v1/account/account_menu").header("authorization",format!("SAPISIDHASH {now}_{digest}")).header("origin","https://www.youtube.com").header("x-youtube-client-name","1").header("x-youtube-client-version","2.20240101.00.00").json(&serde_json::json!({"context":{"client":{"clientName":"WEB","clientVersion":"2.20240101.00.00","hl":"en"}}}))
            } else if instagram {
                if !cookies.contains_key("sessionid") {
                    return Ok(empty());
                }
                self.client
                    .get("https://www.instagram.com/api/v1/accounts/current_user/?edit=true")
                    .header("x-ig-app-id", "936619743392459")
            } else if twitter {
                if !cookies.contains_key("auth_token") {
                    return Ok(empty());
                }
                self.client.get("https://api.twitter.com/1.1/account/verify_credentials.json").header("x-csrf-token",cookies.get("ct0").copied().unwrap_or("")).header("authorization","Bearer AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA")
            } else {
                return Ok(empty());
            };
            req=req.header("cookie",header).header("user-agent","Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36");
            let mut response = match req.send().await {
                Ok(r) => r,
                Err(_) => return Ok(empty()),
            };
            let mut updates = BTreeMap::new();
            if youtube {
                for h in response.headers().get_all("set-cookie") {
                    if let Ok(h) = h.to_str()
                        && let Some((k, v)) = h.split(';').next().unwrap_or("").split_once('=')
                        && k.len() <= 256
                        && v.len() <= 16384
                        && !k.chars().chain(v.chars()).any(char::is_control)
                    {
                        updates.insert(k.trim().into(), v.trim().into());
                    }
                }
            }
            if !response.status().is_success() {
                return Ok(AccountResult {
                    info: None,
                    updates,
                });
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| DownloadError::Executor)?
            {
                if bytes.len() + chunk.len() > 1024 * 1024 {
                    return Err(DownloadError::Executor);
                }
                bytes.extend_from_slice(&chunk)
            }
            let v: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| DownloadError::Executor)?;
            let (name, avatar) = if youtube {
                (
                    find(&v, "accountName")
                        .and_then(|x| x["simpleText"].as_str())
                        .or_else(|| {
                            find(&v, "channelHandle").and_then(|x| x["simpleText"].as_str())
                        })
                        .map(str::to_owned),
                    find(&v, "accountPhoto")
                        .and_then(|x| x["thumbnails"].as_array())
                        .and_then(|a| a.last())
                        .and_then(|x| x["url"].as_str())
                        .map(str::to_owned),
                )
            } else if instagram {
                (
                    v["user"]["username"]
                        .as_str()
                        .or(v["user"]["full_name"].as_str())
                        .map(|x| format!("@{x}")),
                    v["user"]["profile_pic_url_hd"]
                        .as_str()
                        .or(v["user"]["profile_pic_url"].as_str())
                        .map(str::to_owned),
                )
            } else {
                (
                    v["screen_name"]
                        .as_str()
                        .or(v["name"].as_str())
                        .map(|x| format!("@{x}")),
                    v["profile_image_url_https"]
                        .as_str()
                        .map(|x| x.replace("_normal", "")),
                )
            };
            let info = name
                .filter(|n| !n.is_empty() && n.len() <= 128 && !n.chars().any(char::is_control))
                .map(|label| AccountInfo {
                    label,
                    avatar_url: avatar.filter(|a| {
                        a.len() <= 512
                            && url::Url::parse(a).is_ok_and(|u| {
                                u.scheme() == "https"
                                    && u.host_str().is_some()
                                    && u.username().is_empty()
                                    && u.password().is_none()
                            })
                    }),
                });
            Ok(AccountResult { info, updates })
        })
    }
}
fn db(_: yoink_db::Error) -> DownloadError {
    DownloadError::Database
}
fn merge(content: &str, updates: &BTreeMap<String, String>) -> String {
    let mut result = String::new();
    for line in content.lines() {
        if line.starts_with("#HttpOnly_") || !line.starts_with('#') {
            let mut fields = line.split('\t').collect::<Vec<_>>();
            if fields.len() == 7
                && let Some(v) = updates.get(fields[5])
            {
                fields[6] = v;
                result.push_str(&fields.join("\t"));
                result.push('\n');
                continue;
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    result
}
impl DownloadService {
    pub fn with_cookie_account_provider(mut self, p: Arc<dyn AccountProvider>) -> Self {
        self.cookie_account_provider = Some(p);
        self
    }
    pub async fn web_cookie_validate(
        &self,
        actor: i64,
        id: i64,
        now: i64,
    ) -> Result<(), DownloadError> {
        let (domain, content, updated, initial_owner, initial_pool) = {
            let mut conn = self.store.database().connection().await.map_err(db)?;
            let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
                .await
                .map_err(|_| DownloadError::Database)?;
            self.web_authorize(&tx, actor, now).await?;
            let row =
                query("SELECT user_id,is_pool,domain,content,updated_at FROM cookies WHERE id=?1")
                    .bind(id)
                    .fetch_optional(&*tx)
                    .await
                    .map_err(db)?
                    .ok_or(DownloadError::InvalidInput)?;
            if !self.web_admin(&tx, actor).await?
                && (row.try_get::<i64, _>("user_id").map_err(db)? != actor
                    || row.try_get::<bool, _>("is_pool").map_err(db)?)
            {
                return Err(DownloadError::Denied);
            }
            let result = (
                row.try_get::<String, _>("domain").map_err(db)?,
                row.try_get::<String, _>("content").map_err(db)?,
                row.try_get::<DateTime<Utc>, _>("updated_at").map_err(db)?,
                row.try_get::<i64, _>("user_id").map_err(db)?,
                row.try_get::<bool, _>("is_pool").map_err(db)?,
            );
            tx.commit().await.map_err(|_| DownloadError::Database)?;
            result
        };
        // OAuth credentials are not Netscape cookies. Never invalidate or rotate
        // them via the ordinary account provider.
        if content.starts_with(crate::cookie_oauth::CONTENT_PREFIX) {
            return Err(DownloadError::UnsupportedAuthentication);
        }
        let input = CookieInput {
            domain: domain.clone(),
            content: content.clone(),
        };
        let started = std::time::Instant::now();
        let result = if validate(&input).is_ok() {
            tokio::time::timeout(
                self.cookie_account_provider
                    .as_ref()
                    .ok_or(DownloadError::UnsupportedAuthentication)?
                    .timeout(),
                self.cookie_account_provider
                    .as_ref()
                    .ok_or(DownloadError::UnsupportedAuthentication)?
                    .fetch(&domain, &content, now),
            )
            .await
            .map_err(|_| DownloadError::Executor)??
        } else {
            AccountResult {
                info: None,
                updates: BTreeMap::new(),
            }
        };
        let now = now.saturating_add(started.elapsed().as_secs() as i64);
        let new_content = merge(&content, &result.updates);
        let valid = result.info.is_some();
        if !result.updates.is_empty() {
            validate(&CookieInput {
                domain: domain.clone(),
                content: new_content.clone(),
            })?;
        }
        let mut conn = self.store.database().connection().await.map_err(db)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        self.web_authorize(&tx, actor, now).await?;
        let row =
            query("SELECT user_id,is_pool,domain,content,updated_at FROM cookies WHERE id=?1")
                .bind(id)
                .fetch_optional(&*tx)
                .await
                .map_err(db)?
                .ok_or(DownloadError::ChangedCredentials)?;
        let owner = row.try_get::<i64, _>("user_id").map_err(db)?;
        let pool = row.try_get::<bool, _>("is_pool").map_err(db)?;
        if !self.web_admin(&tx, actor).await? && (owner != actor || pool) {
            return Err(DownloadError::Denied);
        }
        if row.try_get::<String, _>("content").map_err(db)? != content
            || row.try_get::<DateTime<Utc>, _>("updated_at").map_err(db)? != updated
            || row.try_get::<String, _>("domain").map_err(db)? != domain
            || owner != initial_owner
            || pool != initial_pool
        {
            return Err(DownloadError::ChangedCredentials);
        }
        if new_content.len() > content.len() {
            let usage = query(include_str!("personal_cookies_usage.sql"))
                .bind(owner)
                .bind(domain.as_str())
                .fetch_one(&*tx)
                .await
                .map_err(db)?;
            let growth = (new_content.len() - content.len()) as i64;
            let limits = self.network_config.personal_cookie_limits;
            if pool {
                let bytes=query("SELECT COALESCE(SUM(length(CAST(content AS BLOB))),0) AS n FROM cookies WHERE is_pool=1").fetch_one(&*tx).await.map_err(db)?.try_get::<i64,_>("n").map_err(db)?;
                if bytes + growth > 1024 * 1024 * 1024 {
                    return Err(DownloadError::PersonalCookieCapacity);
                }
            }
            if !pool
                && (usage.try_get::<i64, _>("actor_bytes").map_err(db)? + growth
                    > limits.actor_bytes as i64
                    || usage.try_get::<i64, _>("global_bytes").map_err(db)? + growth
                        > limits.global_bytes as i64)
            {
                return Err(DownloadError::PersonalCookieCapacity);
            }
        }
        let timestamp = DateTime::from_timestamp(now, 0).ok_or(DownloadError::InvalidInput)?;
        query("UPDATE cookies SET is_valid=?2,validated_at=?3,content=?4,updated_at=CASE WHEN content!=?4 THEN ?3 ELSE updated_at END WHERE id=?1").bind(id).bind(valid).bind(timestamp).bind(if result.updates.is_empty(){&content}else{&new_content}).execute(&*tx).await.map_err(db)?;
        tx.commit().await.map_err(|_| DownloadError::Database)
    }
    pub async fn web_pool_refresh_labels(
        &self,
        actor: i64,
        now: i64,
    ) -> Result<u64, DownloadError> {
        let rows = self.web_cookie_scoped_list(actor, 2, now).await?;
        let mut count = 0;
        for metadata in rows {
            let content = query("SELECT content FROM cookies WHERE id=?1 AND is_pool=1")
                .bind(metadata.id)
                .fetch_optional(self.store.database())
                .await
                .map_err(db)?;
            let Some(content) = content else { continue };
            let content: String = content.try_get("content").map_err(db)?;
            let fetched = if validate(&CookieInput {
                domain: metadata.domain.clone(),
                content: content.clone(),
            })
            .is_ok()
            {
                tokio::time::timeout(
                    self.cookie_account_provider
                        .as_ref()
                        .ok_or(DownloadError::UnsupportedAuthentication)?
                        .timeout(),
                    self.cookie_account_provider
                        .as_ref()
                        .ok_or(DownloadError::UnsupportedAuthentication)?
                        .fetch(&metadata.domain, &content, now),
                )
                .await
                .ok()
                .and_then(Result::ok)
                .and_then(|v| v.info)
            } else {
                None
            };
            let (label, avatar) = if let Some(info) = fetched {
                (Some(info.label), info.avatar_url)
            } else {
                (label(&metadata.domain, &content), None)
            };
            if label == metadata.label && avatar == metadata.avatar_url {
                continue;
            }
            let mut conn = self.store.database().connection().await.map_err(db)?;
            let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
                .await
                .map_err(|_| DownloadError::Database)?;
            self.web_authorize(&tx, actor, now).await?;
            if !self.web_admin(&tx, actor).await? {
                return Err(DownloadError::Denied);
            }
            count+=query("UPDATE cookies SET label=?2,avatar_url=?3 WHERE id=?1 AND is_pool=1 AND content=?4 AND updated_at=?5").bind(metadata.id).bind(label.as_deref()).bind(avatar.as_deref()).bind(&content).bind(metadata.updated_at).execute(&*tx).await.map_err(db)?.rows_affected();
            tx.commit().await.map_err(|_| DownloadError::Database)?;
        }
        Ok(count)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct AccountProviderConfig {
    timeout: Duration,
}
impl Default for AccountProviderConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
        }
    }
}
impl AccountProviderConfig {
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
