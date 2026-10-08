//! Cancellable, bounded GitHub device lifecycle; no star/write operations.
use crate::{Error, Insight, invalid};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use turso::transaction::{Transaction, TransactionBehavior};
use yoink_db::query;
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub user_code: String,
    pub verification_uri: String,
    #[serde(serialize_with = "epoch")]
    pub expires_at: DateTime<Utc>,
    pub interval: u64,
    pub status: String,
    pub error: Option<String>,
    pub username: Option<String>,
}
struct Flow {
    status: Status,
    cancel: CancellationToken,
    generation: u64,
}
#[derive(Clone)]
pub struct DeviceOAuth {
    service: Insight,
    flows: Arc<Mutex<HashMap<(i64, bool), Flow>>>,
    client: reqwest::Client,
    base: String,
    user_url: String,
    read_client: String,
    write_client: Option<String>,
    shutdown: CancellationToken,
    tracker: tokio_util::task::TaskTracker,
    username_timeout: Duration,
    #[cfg(test)]
    persistence_waiting: Arc<tokio::sync::Notify>,
}
/// Trusted operator/fixture configuration, never deserialized from user requests.
#[derive(Clone, Debug)]
pub struct OAuthOptions {
    pub request_timeout: Duration,
    pub username_timeout: Duration,
    pub device_base_url: String,
    pub user_url: String,
}
impl Default for OAuthOptions {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(15),
            username_timeout: Duration::from_secs(10),
            device_base_url: "https://github.com".into(),
            user_url: "https://api.github.com/user".into(),
        }
    }
}
impl OAuthOptions {
    fn validate(&self) -> Result<(), Error> {
        if self.request_timeout.is_zero()
            || self.request_timeout > Duration::from_secs(300)
            || self.username_timeout.is_zero()
            || self.username_timeout > self.request_timeout
        {
            return Err(invalid());
        }
        for endpoint in [&self.device_base_url, &self.user_url] {
            let url = url::Url::parse(endpoint).map_err(|_| invalid())?;
            let loopback = url.host_str().is_some_and(|host| {
                host == "localhost"
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            });
            if endpoint.len() > 2048
                || !(url.scheme() == "https" || url.scheme() == "http" && loopback)
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
impl DeviceOAuth {
    pub fn new(
        service: Insight,
        read_client: String,
        write_client: Option<String>,
        shutdown: CancellationToken,
    ) -> Result<Self, Error> {
        Self::with_options(
            service,
            read_client,
            write_client,
            shutdown,
            OAuthOptions::default(),
        )
    }
    pub fn with_options(
        service: Insight,
        read_client: String,
        write_client: Option<String>,
        shutdown: CancellationToken,
        options: OAuthOptions,
    ) -> Result<Self, Error> {
        options.validate()?;
        Ok(Self {
            service,
            flows: Arc::new(Mutex::new(HashMap::new())),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(options.request_timeout)
                .build()
                .map_err(|_| invalid())?,
            base: options.device_base_url.trim_end_matches('/').into(),
            user_url: options.user_url,
            read_client,
            write_client,
            shutdown,
            tracker: tokio_util::task::TaskTracker::new(),
            username_timeout: options.username_timeout,
            #[cfg(test)]
            persistence_waiting: Arc::new(tokio::sync::Notify::new()),
        })
    }
    async fn authorized(&self, uid: i64, write: bool) -> Result<(), Error> {
        let c = self.service.db.connection().await?;
        self.service
            .authorize(&*c, uid, if write { None } else { Some("tldr") }, false)
            .await
    }
    async fn post(
        &self,
        path: &str,
        body: Value,
        cancel: &CancellationToken,
    ) -> Result<Value, Error> {
        let run = async {
            let mut r = self
                .client
                .post(format!("{}{path}", self.base))
                .header("accept", "application/json")
                .header("content-type", "application/json")
                .body(body.to_string())
                .send()
                .await
                .map_err(|_| Error::Code("oauth_unavailable"))?;
            if !r.status().is_success() {
                return Err(Error::Code("oauth_unavailable"));
            }
            let mut bytes = Vec::new();
            while let Some(c) = r
                .chunk()
                .await
                .map_err(|_| Error::Code("oauth_unavailable"))?
            {
                if bytes.len() + c.len() > 32768 {
                    return Err(Error::Code("oauth_invalid_response"));
                }
                bytes.extend_from_slice(&c)
            }
            serde_json::from_slice(&bytes).map_err(|_| Error::Code("oauth_invalid_response"))
        };
        tokio::select! {biased;_=cancel.cancelled()=>Err(Error::Code("cancelled")),r=run=>r}
    }
    async fn username(&self, token: &str, cancel: &CancellationToken) -> Option<String> {
        let lookup = async {
            let mut response = self
                .client
                .get(&self.user_url)
                .header("authorization", format!("token {token}"))
                .header("accept", "application/json")
                .header("user-agent", "yoink-insight-device-flow/1.0")
                .send()
                .await
                .ok()?;
            if !response.status().is_success() {
                return None;
            }
            if response.content_length().is_some_and(|n| n > 32768) {
                return None;
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.ok()? {
                if bytes.len() + chunk.len() > 32768 {
                    return None;
                }
                bytes.extend_from_slice(&chunk)
            }
            let body: Value = serde_json::from_slice(&bytes).ok()?;
            // GitHub login constraints keep untrusted text/control chars out of UI.
            body["login"]
                .as_str()
                .filter(|s| {
                    !s.is_empty()
                        && s.len() <= 39
                        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
                .map(str::to_owned)
        };
        tokio::select! {biased;_=cancel.cancelled()=>None,result=tokio::time::timeout(self.username_timeout,lookup)=>result.ok().flatten()}
    }
    pub async fn start(&self, uid: i64, write: bool) -> Result<Status, Error> {
        self.authorized(uid, write).await?;
        let client = if write {
            self.write_client
                .as_ref()
                .ok_or(Error::Code("oauth_not_configured"))?
        } else {
            &self.read_client
        };
        if client.is_empty() {
            return Err(Error::Code("oauth_not_configured"));
        }
        let cancel = self.shutdown.child_token();
        let generation = {
            let mut flows = self.flows.lock().await;
            flows.retain(|_, f| {
                f.status.status == "pending"
                    || f.status.expires_at + chrono::Duration::hours(1) > Utc::now()
            });
            if flows.len() >= 1024 && !flows.contains_key(&(uid, write)) {
                return Err(Error::Code("oauth_capacity"));
            }
            let generation = flows
                .get(&(uid, write))
                .map(|f| f.generation + 1)
                .unwrap_or(1);
            if let Some(old) = flows.remove(&(uid, write)) {
                old.cancel.cancel()
            }
            flows.insert(
                (uid, write),
                Flow {
                    status: Status {
                        user_code: String::new(),
                        verification_uri: "https://github.com/login/device".into(),
                        expires_at: Utc::now() + chrono::Duration::seconds(30),
                        interval: 5,
                        status: "pending".into(),
                        error: None,
                        username: None,
                    },
                    cancel: cancel.clone(),
                    generation,
                },
            );
            generation
        };
        let data = self
            .post(
                "/login/device/code",
                json!({"client_id":client,"scope":if write{"public_repo"}else{"read:user"}}),
                &cancel,
            )
            .await;
        let data = match data {
            Ok(data) => data,
            Err(e) => {
                self.finish(uid, write, generation, "error").await;
                return Err(e);
            }
        };
        let code = data["device_code"]
            .as_str()
            .ok_or(Error::Code("oauth_invalid_response"))?
            .to_owned();
        let user_code = data["user_code"]
            .as_str()
            .ok_or(Error::Code("oauth_invalid_response"))?
            .to_owned();
        let expires = data["expires_in"].as_u64().unwrap_or(900).clamp(1, 1800);
        let interval = data["interval"].as_u64().unwrap_or(5).clamp(1, 60);
        let status = Status {
            user_code,
            verification_uri: "https://github.com/login/device".into(),
            expires_at: Utc::now() + chrono::Duration::seconds(expires as i64),
            interval,
            status: "pending".into(),
            error: None,
            username: None,
        };
        self.authorized(uid, write).await?;
        let mut flows = self.flows.lock().await;
        let Some(flow) = flows.get_mut(&(uid, write)) else {
            return Err(Error::Code("cancelled"));
        };
        if flow.generation != generation || cancel.is_cancelled() {
            return Err(Error::Code("cancelled"));
        }
        flow.status = status.clone();
        drop(flows);
        let this = self.clone();
        let client = client.clone();
        self.tracker.spawn(async move {
            this.poll(uid, write, generation, client, code, cancel)
                .await;
        });
        Ok(status)
    }
    pub async fn shutdown(&self) {
        self.shutdown.cancel();
        self.tracker.close();
        self.tracker.wait().await;
    }
    pub async fn status(&self, uid: i64, write: bool) -> Result<Value, Error> {
        self.authorized(uid, write).await?;
        let flows = self.flows.lock().await;
        Ok(flows
            .get(&(uid, write))
            .map(|f| serde_json::to_value(&f.status).unwrap_or(json!({"status":"error"})))
            .unwrap_or(json!({"status":"none"})))
    }
    pub async fn cancel(&self, uid: i64, write: bool) -> Result<(), Error> {
        {
            let mut flows = self.flows.lock().await;
            if let Some(f) = flows.remove(&(uid, write)) {
                f.cancel.cancel()
            }
        } // Never hold the generation mutex while acquiring the database.
        let c = self.service.db.connection().await?;
        self.service.authorize(&*c, uid, None, false).await?;
        let sql = if write {
            "UPDATE insight_user_settings SET github_token_public_repo=NULL WHERE user_id=?"
        } else {
            "UPDATE insight_user_settings SET github_token=NULL WHERE user_id=?"
        };
        query(sql).bind(uid).execute(&*c).await?;
        Ok(())
    }
    async fn finish(&self, uid: i64, write: bool, generation: u64, status: &str) {
        if let Some(f) = self.flows.lock().await.get_mut(&(uid, write))
            && f.generation == generation
        {
            f.status.status = status.into();
            if status == "error" {
                f.status.error = Some("oauth_failed".into())
            }
        }
    }
    async fn poll(
        &self,
        uid: i64,
        write: bool,
        generation: u64,
        client: String,
        code: String,
        cancel: CancellationToken,
    ) {
        loop {
            let state = {
                let flows = self.flows.lock().await;
                let Some(f) = flows.get(&(uid, write)) else {
                    return;
                };
                if f.generation != generation {
                    return;
                }
                f.status.clone()
            };
            if Utc::now() >= state.expires_at {
                self.finish(uid, write, generation, "expired").await;
                return;
            }
            tokio::select! {biased;_=cancel.cancelled()=>{self.finish(uid,write,generation,"cancelled").await;return},_=tokio::time::sleep(Duration::from_secs(state.interval))=>{}}
            if Utc::now() >= state.expires_at {
                self.finish(uid, write, generation, "expired").await;
                return;
            }
            if self.authorized(uid, write).await.is_err() {
                self.finish(uid, write, generation, "error").await;
                return;
            }
            let result=self.post("/login/oauth/access_token",json!({"client_id":client,"device_code":code,"grant_type":"urn:ietf:params:oauth:grant-type:device_code"}),&cancel).await;
            let data = match result {
                Ok(d) => d,
                Err(_) => {
                    self.finish(uid, write, generation, "error").await;
                    return;
                }
            };
            match data["error"].as_str() {
                Some("authorization_pending") => continue,
                Some("slow_down") => {
                    if let Some(f) = self.flows.lock().await.get_mut(&(uid, write))
                        && f.generation == generation
                    {
                        f.status.interval = (f.status.interval + 5).min(60)
                    }
                    continue;
                }
                Some("expired_token") => {
                    self.finish(uid, write, generation, "expired").await;
                    return;
                }
                Some(_) => {
                    self.finish(uid, write, generation, "error").await;
                    return;
                }
                None => {}
            }
            let Some(token) = data["access_token"]
                .as_str()
                .filter(|t| !t.is_empty() && t.len() <= 256)
            else {
                self.finish(uid, write, generation, "error").await;
                return;
            };
            if write
                && !data["scope"].as_str().is_some_and(|s| {
                    s.split([',', ' '])
                        .any(|s| s == "public_repo" || s == "repo")
                })
            {
                self.finish(uid, write, generation, "error").await;
                return;
            }
            // Both executable legacy flows perform a best-effort /user lookup.
            // No generation/DB mutex is held across this bounded network call.
            let username = self.username(token, &cancel).await;
            // Do not hold the generation mutex while waiting for the database:
            // disconnect/restart must be able to cancel this flow during that wait.
            #[cfg(test)]
            self.persistence_waiting.notify_one();
            let remaining = (state.expires_at - Utc::now()).to_std().unwrap_or_default();
            let connection = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    self.finish(uid, write, generation, "cancelled").await;
                    return;
                }
                _ = tokio::time::sleep(remaining) => {
                    self.finish(uid, write, generation, "expired").await;
                    return;
                }
                result = self.service.db.connection() => result,
            };
            let Ok(mut c) = connection else {
                self.finish(uid, write, generation, "error").await;
                return;
            };
            // Generation is locked only after acquisition. Keep it locked through
            // the transaction so disconnect/restart and persistence linearize.
            let mut flows = self.flows.lock().await;
            let Some(f) = flows.get_mut(&(uid, write)) else {
                return;
            };
            if f.generation != generation {
                return;
            }
            let live = || {
                if cancel.is_cancelled() {
                    Err(Error::Code("cancelled"))
                } else if Utc::now() >= state.expires_at {
                    Err(Error::Code("expired"))
                } else {
                    Ok(())
                }
            };
            let saved = async {
                live()?; // Recheck after acquiring the database and generation lock.
                let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
                    .await.map_err(|_| Error::Database)?;
                let result = async {
                    self.service.authorize(&*tx, uid, if write { None } else { Some("tldr") }, false).await?;
                    live()?; // Authorization awaits too: fence immediately before writing.
                    let sql = if write {
                        "INSERT INTO insight_user_settings(user_id,github_token_public_repo) VALUES(?,?) ON CONFLICT(user_id) DO UPDATE SET github_token_public_repo=excluded.github_token_public_repo"
                    } else {
                        "INSERT INTO insight_user_settings(user_id,github_token) VALUES(?,?) ON CONFLICT(user_id) DO UPDATE SET github_token=excluded.github_token"
                    };
                    query(sql).bind(uid).bind(token).execute(&*tx).await?;
                    live()?; // Roll back if cancellation/expiry occurred during the write.
                    Ok::<_, Error>(())
                }.await;
                match result {
                    Ok(()) => tx.commit().await.map_err(|_| Error::Database),
                    Err(error) => { let _ = tx.rollback().await; Err(error) }
                }
            }.await;
            f.status.status = match saved {
                Ok(()) => {
                    f.status.username = username;
                    "success"
                }
                Err(Error::Code("cancelled")) => "cancelled",
                Err(Error::Code("expired")) => "expired",
                Err(_) => {
                    f.status.error = Some("oauth_failed".into());
                    "error"
                }
            }
            .into();
            return;
        }
    }
}

fn epoch<S: serde::Serializer>(date: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_f64(date.timestamp_millis() as f64 / 1000.0)
}
#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
impl DeviceOAuth {
    pub fn write_configured(&self) -> bool {
        self.write_client.as_ref().is_some_and(|s| !s.is_empty())
    }
}
