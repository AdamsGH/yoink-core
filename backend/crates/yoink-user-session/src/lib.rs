//! Bounded tdlight HTTP adapter. Credentials exist only at the wire boundary.
use serde_json::Value;
use std::{io::Read, path::PathBuf, time::Duration};
use url::Url;
#[derive(Clone)]
pub struct Config {
    pub origin: Url,
    pub token_file: PathBuf,
    pub timeout: Duration,
    pub token_timeout: Duration,
    pub max_response_bytes: usize,
    pub max_token_bytes: usize,
    pub max_scan_pages: usize,
    pub max_scan_topics: usize,
}
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("User-mode session not configured or token missing")]
    Unavailable,
    #[error("User session unauthorized")]
    Unauthorized,
    #[error("Access denied")]
    Forbidden,
    #[error("Invalid user-session configuration or request")]
    Invalid,
    #[error("User session token file rejected")]
    Token,
    #[error("User session transport failed")]
    Network,
    #[error("Invalid tdlight response")]
    Response,
    #[error("User session bound exceeded")]
    Bounds,
}
#[derive(Clone, Copy, Debug)]
pub enum Method {
    GetMe,
    GetForumTopics,
    GetForumTopic,
    GetForumTopicLink,
    SearchChatMessages,
    GetChatMembers,
    GetChatHistory,
    GetMessageViewers,
    GetMessageLink,
    GetMessageReadDate,
    GetMessageThread,
    GetChatMessageByDate,
}
impl Method {
    pub fn wire(self) -> &'static str {
        match self {
            Self::GetMe => "getMe",
            Self::GetForumTopics => "getForumTopics",
            Self::GetForumTopic => "getForumTopic",
            Self::GetForumTopicLink => "getForumTopicLink",
            Self::SearchChatMessages => "searchChatMessages",
            Self::GetChatMembers => "getChatMembers",
            Self::GetChatHistory => "getChatHistory",
            Self::GetMessageViewers => "getMessageViewers",
            Self::GetMessageLink => "getMessageLink",
            Self::GetMessageReadDate => "getMessageReadDate",
            Self::GetMessageThread => "getMessageThread",
            Self::GetChatMessageByDate => "getChatMessageByDate",
        }
    }
}
pub struct Client {
    config: Config,
    http: reqwest::Client,
    reads: std::sync::Arc<tokio::sync::Semaphore>,
}
impl Client {
    pub fn new(config: Config) -> Result<Self, Error> {
        let u = &config.origin;
        if !matches!(u.scheme(), "http" | "https")
            || u.host_str().is_none()
            || !u.username().is_empty()
            || u.password().is_some()
            || u.query().is_some()
            || u.fragment().is_some()
            || u.path() != "/"
            || !config.token_file.is_absolute()
            || config.timeout.is_zero()
            || config.token_timeout.is_zero()
            || config.max_response_bytes == 0
            || config.max_token_bytes == 0
            || config.max_scan_pages == 0
            || config.max_scan_topics == 0
        {
            return Err(Error::Invalid);
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(config.timeout)
            .build()
            .map_err(|_| Error::Invalid)?;
        Ok(Self {
            config,
            http,
            reads: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        })
    }
    async fn token(&self) -> Result<String, Error> {
        let path = self.config.token_file.clone();
        let max = self.config.max_token_bytes;
        let permit = tokio::time::timeout(
            self.config.token_timeout,
            self.reads.clone().acquire_owned(),
        )
        .await
        .map_err(|_| Error::Bounds)?
        .map_err(|_| Error::Token)?;
        tokio::time::timeout(
            self.config.token_timeout,
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                // Walk from / with NOFOLLOW on every component; never accept symlinks or devices.
                use rustix::fs::{Mode, OFlags, open, openat};
                let mut fd = open(
                    "/",
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| Error::Token)?;
                let parts: Vec<_> = path.components().collect();
                for (i, part) in parts.iter().enumerate().skip(1) {
                    let std::path::Component::Normal(p) = part else {
                        return Err(Error::Token);
                    };
                    let flags =
                        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
                    fd = openat(
                        &fd,
                        *p,
                        if i + 1 == parts.len() {
                            flags
                        } else {
                            flags | OFlags::DIRECTORY
                        },
                        Mode::empty(),
                    )
                    .map_err(|e| {
                        if e == rustix::io::Errno::NOENT {
                            Error::Unavailable
                        } else {
                            Error::Token
                        }
                    })?;
                }
                let mut file = std::fs::File::from(fd);
                use std::os::unix::fs::MetadataExt;
                let m = file.metadata().map_err(|_| Error::Token)?;
                if m.uid() != rustix::process::geteuid().as_raw()
                    || !m.is_file()
                    || m.mode() & 0o077 != 0
                    || m.nlink() != 1
                {
                    return Err(Error::Token);
                }
                let mut bytes = Vec::new();
                file.by_ref()
                    .take(max as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| Error::Token)?;
                if bytes.len() > max {
                    return Err(Error::Bounds);
                }
                let token = std::str::from_utf8(&bytes)
                    .map_err(|_| Error::Token)?
                    .trim();
                if token.is_empty() {
                    return Err(Error::Unavailable);
                }
                if !token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'_' | b'-'))
                {
                    return Err(Error::Token);
                }
                Ok(token.to_owned())
            }),
        )
        .await
        .map_err(|_| Error::Bounds)?
        .map_err(|_| Error::Token)?
    }
    pub async fn available(&self) -> Result<bool, Error> {
        match self.token().await {
            Ok(_) => Ok(true),
            Err(Error::Unavailable) => Ok(false),
            Err(e) => Err(e),
        }
    }
    pub async fn call(&self, method: Method, params: &[(String, String)]) -> Result<Value, Error> {
        let token = self.token().await?;
        let mut url = self.config.origin.clone();
        url.set_path(&format!("/user{token}/{}", method.wire()));
        let mut response = self
            .http
            .post(url)
            .form(params)
            .send()
            .await
            .map_err(|_| Error::Network)?;
        let status = response.status();
        if matches!(status.as_u16(), 401 | 403) {
            return Err(Error::Unauthorized);
        }
        if !status.is_success() {
            return Err(Error::Network);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
            if chunk.len() > self.config.max_response_bytes.saturating_sub(bytes.len()) {
                return Err(Error::Bounds);
            }
            bytes.extend_from_slice(&chunk);
        }
        let data: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Response)?;
        if data["ok"] != true {
            return Err(if matches!(data["error_code"].as_i64(), Some(401 | 403)) {
                Error::Unauthorized
            } else {
                Error::Response
            });
        }
        data.get("result").cloned().ok_or(Error::Response)
    }
    pub async fn scan(&self, chat: i64) -> Result<Vec<Value>, Error> {
        self.scan_checked(chat, || async { Ok(()) }).await
    }
    pub async fn scan_checked<F, Fut>(
        &self,
        chat: i64,
        mut authorize: F,
    ) -> Result<Vec<Value>, Error>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<(), Error>>,
    {
        let mut topics = Vec::new();
        let mut cursor = (0, 0, 0);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..self.config.max_scan_pages {
            let p = [
                ("chat_id", chat),
                ("limit", 100),
                ("offset_date", cursor.0),
                ("offset_message_id", cursor.1),
                ("offset_forum_topic_id", cursor.2),
            ]
            .map(|(k, v)| (k.to_owned(), v.to_string()));
            authorize().await?;
            let result = self.call(Method::GetForumTopics, &p).await?;
            let batch = result["topics"].as_array().ok_or(Error::Response)?;
            if batch.len() > 100
                || batch.len() > self.config.max_scan_topics.saturating_sub(topics.len())
            {
                return Err(Error::Bounds);
            }
            for t in batch {
                let id = t["message_thread_id"]
                    .as_i64()
                    .filter(|id| *id > 0)
                    .ok_or(Error::Response)?;
                if !seen.insert(id)
                    || t.get("name")
                        .is_some_and(|n| n.as_str().is_none_or(|s| s.chars().count() > 256))
                {
                    return Err(Error::Response);
                }
                topics.push(t.clone());
            }
            for key in [
                "next_offset_date",
                "next_offset_message_id",
                "next_offset_forum_topic_id",
            ] {
                if result.get(key).is_some_and(|v| v.as_i64().is_none()) {
                    return Err(Error::Response);
                }
            }
            let next = (
                result["next_offset_date"].as_i64().unwrap_or(0),
                result["next_offset_message_id"].as_i64().unwrap_or(0),
                result["next_offset_forum_topic_id"].as_i64().unwrap_or(0),
            );
            if batch.len() < 100 || next.0 == 0 {
                return Ok(topics);
            }
            if next == cursor {
                return Err(Error::Response);
            }
            cursor = next;
        }
        Err(Error::Bounds)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn config(path: PathBuf, origin: Url) -> Config {
        Config {
            origin,
            token_file: path,
            timeout: Duration::from_secs(2),
            token_timeout: Duration::from_secs(1),
            max_response_bytes: 8192,
            max_token_bytes: 128,
            max_scan_pages: 2,
            max_scan_topics: 200,
        }
    }
    fn token(p: &std::path::Path, s: &str) {
        std::fs::write(p, s).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    async fn fake(body: &'static str) -> (Url, tokio::task::JoinHandle<()>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let u = format!("http://{}/", l.local_addr().unwrap())
            .parse()
            .unwrap();
        let app = axum::Router::new().fallback(move || async move { body });
        let t = tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (u, t)
    }
    #[tokio::test]
    async fn rotation_and_wire_mapping() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("token");
        token(&p, "first:abc");
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let u = format!("http://{}/", l.local_addr().unwrap())
            .parse()
            .unwrap();
        let app = axum::Router::new().fallback(|uri: axum::http::Uri| async move {
            axum::Json(serde_json::json!({"ok":true,"result":uri.path()}))
        });
        let t = tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let c = Client::new(config(p.clone(), u)).unwrap();
        assert_eq!(
            c.call(Method::GetMe, &[]).await.unwrap(),
            "/userfirst:abc/getMe"
        );
        token(&p, "second");
        assert_eq!(
            c.call(Method::GetMessageViewers, &[]).await.unwrap(),
            "/usersecond/getMessageViewers"
        );
        t.abort();
    }
    #[tokio::test]
    async fn file_boundary() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("token");
        let c = Client::new(config(p.clone(), "http://127.0.0.1/".parse().unwrap())).unwrap();
        assert_eq!(c.available().await, Ok(false));
        token(&p, "");
        assert_eq!(c.available().await, Ok(false));
        token(&p, "bad/token");
        assert_eq!(c.available().await, Err(Error::Token));
        token(&p, "secret");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(c.available().await, Err(Error::Token));
        std::fs::remove_file(&p).unwrap();
        std::os::unix::fs::symlink(d.path().join("missing"), &p).unwrap();
        assert_eq!(c.available().await, Err(Error::Token));
    }
    #[tokio::test]
    async fn upstream_errors_and_bounds() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("token");
        token(&p, "secret");
        for (body, e) in [
            ("not json", Error::Response),
            (
                r#"{"ok":false,"error_code":403,"description":"secret"}"#,
                Error::Unauthorized,
            ),
            (r#"{"ok":true}"#, Error::Response),
        ] {
            let (u, t) = fake(body).await;
            let c = Client::new(config(p.clone(), u)).unwrap();
            assert_eq!(c.call(Method::GetMe, &[]).await, Err(e));
            t.abort();
        }
        let (u, t) = fake("long response").await;
        let mut cfg = config(p, u);
        cfg.max_response_bytes = 2;
        assert_eq!(
            Client::new(cfg).unwrap().call(Method::GetMe, &[]).await,
            Err(Error::Bounds)
        );
        t.abort();
    }
    #[tokio::test]
    async fn network_and_scan_failure() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("token");
        token(&p, "secret");
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let u = format!("http://{}/", l.local_addr().unwrap())
            .parse()
            .unwrap();
        drop(l);
        let c = Client::new(config(p.clone(), u)).unwrap();
        assert_eq!(c.call(Method::GetMe, &[]).await, Err(Error::Network));
        let (u, t) = fake(
            r#"{"ok":true,"result":{"topics":[{"message_thread_id":1},{"message_thread_id":1}]}}"#,
        )
        .await;
        assert_eq!(
            Client::new(config(p, u)).unwrap().scan(-1).await,
            Err(Error::Response)
        );
        t.abort();
    }
    #[test]
    fn origins_are_closed() {
        let d = tempfile::tempdir().unwrap();
        for u in [
            "http://user:secret@localhost/",
            "http://localhost/path",
            "http://localhost/?token=secret",
            "ftp://localhost/",
        ] {
            assert!(Client::new(config(d.path().join("token"), u.parse().unwrap())).is_err());
        }
    }
}
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UserSessionConfig { [configured boundary redacted] }")
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ChatMemberFilter {
    Members,
    Administrators,
    Banned,
    Restricted,
}
impl ChatMemberFilter {
    fn wire(self) -> &'static str {
        match self {
            Self::Members => "members",
            Self::Administrators => "administrators",
            Self::Banned => "banned",
            Self::Restricted => "restricted",
        }
    }
}
#[derive(Clone, Debug, serde::Deserialize, PartialEq, Eq)]
pub struct UserSessionUser {
    pub id: i64,
    pub username: Option<String>,
    #[serde(alias = "firstName")]
    pub first_name: Option<String>,
    #[serde(alias = "lastName")]
    pub last_name: Option<String>,
}
#[derive(Clone, Debug, serde::Deserialize, PartialEq, Eq)]
pub struct UserSessionMember {
    pub user: UserSessionUser,
    pub status: String,
    pub joined_date: Option<i64>,
}
pub type UserSessionError = Error;
impl Client {
    pub async fn get_chat_members(
        &self,
        chat_id: i64,
        filter: ChatMemberFilter,
        offset: u64,
        limit: u32,
    ) -> Result<Vec<UserSessionMember>, Error> {
        if chat_id >= 0 || !(1..=200).contains(&limit) || offset > i64::MAX as u64 {
            return Err(Error::Invalid);
        }
        let params = vec![
            ("chat_id".into(), chat_id.to_string()),
            ("filter".into(), filter.wire().into()),
            ("offset".into(), offset.to_string()),
            ("limit".into(), limit.to_string()),
        ];
        let value = self.call(Method::GetChatMembers, &params).await?;
        let members: Vec<UserSessionMember> =
            serde_json::from_value(value).map_err(|_| Error::Response)?;
        if members.len() > limit as usize {
            return Err(Error::Bounds);
        }
        if members.iter().any(|m| {
            m.user.id <= 0
                || m.joined_date
                    .is_some_and(|d| d.checked_mul(1_000_000).is_none())
        }) {
            return Err(Error::Response);
        }
        Ok(members)
    }
}
#[cfg(test)]
mod member_tests {
    use super::*;
    #[tokio::test]
    async fn roster_plain_list_and_form_contract() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("token");
        std::fs::write(&p, "fixture").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", l.local_addr().unwrap())
            .parse()
            .unwrap();
        let app=axum::Router::new().route("/userfixture/getChatMembers",axum::routing::post(|axum::Form(f):axum::Form<std::collections::HashMap<String,String>>|async move{assert_eq!(f["chat_id"],"-123");assert_eq!(f["filter"],"members");assert_eq!(f["offset"],"200");assert_eq!(f["limit"],"200");axum::Json(serde_json::json!({"ok":true,"result":[{"user":{"id":1,"firstName":"First","lastName":"Last"},"status":"member","joined_date":1700000000}]}))}));
        let t = tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let c = Client::new(Config {
            origin,
            token_file: p,
            timeout: Duration::from_secs(2),
            token_timeout: Duration::from_secs(1),
            max_response_bytes: 8192,
            max_token_bytes: 128,
            max_scan_pages: 1,
            max_scan_topics: 100,
        })
        .unwrap();
        let m = c
            .get_chat_members(-123, ChatMemberFilter::Members, 200, 200)
            .await
            .unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].user.first_name.as_deref(), Some("First"));
        assert_eq!(m[0].user.last_name.as_deref(), Some("Last"));
        assert_eq!(m[0].joined_date, Some(1700000000));
        t.abort();
    }
}
#[cfg(test)]
mod pagination_tests {
    use super::*;
    #[tokio::test]
    async fn bounded_pages_and_fresh_authority() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("token");
        std::fs::write(&p, "fixture").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", l.local_addr().unwrap())
            .parse()
            .unwrap();
        let app=axum::Router::new().fallback(||async {axum::Json(serde_json::json!({"ok":true,"result":{"topics":(1..=100).map(|id|serde_json::json!({"message_thread_id":id})).collect::<Vec<_>>(),"next_offset_date":1}}))});
        let t = tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let mut cfg = Config {
            origin,
            token_file: p,
            timeout: Duration::from_secs(2),
            token_timeout: Duration::from_secs(1),
            max_response_bytes: 8192,
            max_token_bytes: 128,
            max_scan_pages: 1,
            max_scan_topics: 100,
        };
        assert_eq!(
            Client::new(cfg.clone()).unwrap().scan(-123).await,
            Err(Error::Bounds)
        );
        cfg.max_scan_pages = 2;
        let c = Client::new(cfg).unwrap();
        let mut checked = 0;
        let result = c
            .scan_checked(-123, || {
                checked += 1;
                let ok = checked == 1;
                async move { if ok { Ok(()) } else { Err(Error::Forbidden) } }
            })
            .await;
        assert_eq!(result, Err(Error::Forbidden));
        assert_eq!(checked, 2);
        t.abort();
    }
}
