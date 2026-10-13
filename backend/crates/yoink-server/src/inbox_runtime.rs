use crate::{inbox_config::InboxConfig, runtime::ServerError};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use yoink_inbox::{
    contracts::Call,
    github::{Github, GithubClient, StarSnapshot},
};
yoink_insight::impl_inbox_adapter!(pub RuntimeInsight);

#[derive(Clone)]
pub struct CancelGithub {
    pub client: GithubClient,
    pub cancel: CancellationToken,
}
impl CancelGithub {
    fn read<'a, T: Send + 'a>(&'a self, call: Call<'a, T>) -> Call<'a, T> {
        Box::pin(async move {
            tokio::select! {biased; _=self.cancel.cancelled()=>Err(yoink_inbox::Error::Cancelled), r=call=>r}
        })
    }
    fn write<'a, T: Send + 'a>(&'a self, call: Call<'a, T>) -> Call<'a, T> {
        Box::pin(async move {
            tokio::select! {biased; _=self.cancel.cancelled()=>Err(yoink_inbox::Error::Uncertain), r=call=>r}
        })
    }
}
impl Github for CancelGithub {
    fn stars<'a>(&'a self, t: &'a str, e: Option<&'a str>) -> Call<'a, StarSnapshot> {
        self.read(self.client.stars(t, e))
    }
    fn star<'a>(&'a self, t: &'a str, o: &'a str, r: &'a str, s: bool) -> Call<'a, ()> {
        self.write(self.client.star(t, o, r, s))
    }
    fn raw_file<'a>(&'a self, t: Option<&'a str>, p: &'a str) -> Call<'a, String> {
        self.read(self.client.raw_file(t, p))
    }
    fn get_json<'a>(&'a self, t: Option<&'a str>, p: &'a str) -> Call<'a, serde_json::Value> {
        self.read(self.client.get_json(t, p))
    }
    fn is_starred<'a>(&'a self, t: &'a str, o: &'a str, r: &'a str) -> Call<'a, bool> {
        self.read(self.client.is_starred(t, o, r))
    }
    fn graphql<'a>(
        &'a self,
        t: &'a str,
        q: &'a str,
        v: serde_json::Value,
        m: bool,
    ) -> Call<'a, serde_json::Value> {
        let call = self.client.graphql(t, q, v, m);
        if m { self.write(call) } else { self.read(call) }
    }
}
pub struct InboxRuntime {
    pub config: InboxConfig,
    pub store: yoink_inbox::store::Store,
    pub auth: Arc<yoink_http::InboxAuthorization>,
    pub insight: yoink_insight::Insight,
    pub github: GithubClient,
    pub executor: yoink_executor::Executor,
}
impl InboxRuntime {
    #[cfg(test)]
    pub fn new(
        config: InboxConfig,
        storage: &yoink_storage::Store,
        identities: Arc<yoink_identity::IdentityStore>,
        owner: Option<i64>,
    ) -> Result<Self, ServerError> {
        Self::new_with_insight(config, storage, identities, owner, None)
    }
    pub fn new_with_insight(
        mut config: InboxConfig,
        storage: &yoink_storage::Store,
        identities: Arc<yoink_identity::IdentityStore>,
        owner: Option<i64>,
        shared: Option<yoink_insight::Insight>,
    ) -> Result<Self, ServerError> {
        config.insight.owner_id = owner;
        let insight = if let Some(shared) = shared {
            config.insight = shared.config.clone();
            shared
        } else {
            let completion = yoink_insight::provider::HttpCompletion::new(
                config.insight.request_timeout,
                1024 * 1024,
            )
            .map_err(|_| ServerError::InboxStartup)?
            .with_tunnel(
                config
                    .insight
                    .ai_proxy
                    .as_deref()
                    .ok_or(ServerError::InboxStartup)?,
            )
            .map_err(|_| ServerError::InboxStartup)?;
            let insight = yoink_insight::Insight::completion_only(
                storage.database().clone(),
                config.insight.clone(),
                Arc::new(completion),
            )
            .map_err(|_| ServerError::InboxStartup)?;
            insight.with_access_change_callback(Arc::new({
                let identities = identities.clone();
                move |_| identities.notify_committed_access_change()
            }))
        };
        let mut github = GithubClient::new(config.github_timeout, None)
            .map_err(|_| ServerError::InboxStartup)?;
        if let Some(endpoint) = config.github_endpoint.clone() {
            github = github
                .with_endpoint(endpoint)
                .map_err(|_| ServerError::InboxStartup)?;
        }
        let executor = yoink_executor::Executor::new(config.executor.clone())
            .map_err(|_| ServerError::InboxStartup)?;
        Ok(Self {
            config,
            store: yoink_inbox::store::Store::new(storage.database().clone()),
            auth: Arc::new(yoink_http::InboxAuthorization::new(identities)),
            insight,
            github,
            executor,
        })
    }
    pub fn http(&self, cancel: CancellationToken) -> yoink_inbox::http::HttpState {
        yoink_inbox::http::HttpState {
            store: self.store.clone(),
            auth: self.auth.clone(),
            insight: Arc::new(RuntimeInsight::new(self.insight.clone(), cancel.clone())),
            github: Arc::new(CancelGithub {
                client: self.github.clone(),
                cancel,
            }),
        }
    }
    pub async fn run(self, cancel: CancellationToken) -> Result<(), ServerError> {
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            for _ in 0..self.config.batch {
                if cancel.is_cancelled() {
                    return Ok(());
                }
                let job = cancel.child_token();
                let stages = yoink_inbox::content_stages::ContentStages::new(
                    self.executor.clone(),
                    None,
                    job.clone(),
                );
                let github = CancelGithub {
                    client: self.github.clone(),
                    cancel: job.clone(),
                };
                let insight = RuntimeInsight::new(self.insight.clone(), job.clone());
                let content = yoink_inbox::enrich::FallbackContent {
                    stages: &stages,
                    github: &github,
                    insight: &insight,
                    max_chars: self.config.insight.max_chars,
                };
                static SEQUENCE: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(0);
                let token = format!(
                    "runtime-{}-{}-{}",
                    std::process::id(),
                    now(),
                    SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                );
                let Some(claim) = self
                    .store
                    .claim(now(), self.config.lease.as_micros() as i64, &token)
                    .await
                    .map_err(|_| ServerError::Inbox)?
                else {
                    break;
                };
                let execution = yoink_inbox::pipeline::execute(
                    &self.store,
                    &claim,
                    &*self.auth,
                    &insight,
                    &content,
                    &github,
                    &now,
                );
                let result = {
                    tokio::pin!(execution);
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            job.cancel();
                            let _ = tokio::time::timeout(self.config.drain, &mut execution).await;
                            Err(yoink_inbox::Error::Cancelled)
                        },
                        _ = tokio::time::sleep(self.config.job) => {
                            job.cancel();
                            let _ = tokio::time::timeout(self.config.drain, &mut execution).await;
                            // The adapter's cancellation is cleanup, not an owning
                            // shutdown interruption. A deadline consumes this attempt.
                            tracing::warn!("Inbox job exceeded its whole-job deadline");
                            Err(yoink_inbox::Error::External)
                        },
                        result = &mut execution => match result {
                            // Only cancellation by the owning runtime restores an
                            // attempt; an independently cancelled adapter is a failure.
                            Err(yoink_inbox::Error::Cancelled) if !cancel.is_cancelled() =>
                                Err(yoink_inbox::Error::External),
                            result => result,
                        },
                    }
                };
                if let Err(error) = result {
                    let interrupted = matches!(error, yoink_inbox::Error::Cancelled);
                    let retry = interrupted
                        || (claim.attempts < self.config.max_attempts
                            && matches!(
                                error,
                                yoink_inbox::Error::External
                                    | yoink_inbox::Error::RateLimited
                                    | yoink_inbox::Error::Database(_)
                            ));
                    let code = match error {
                        yoink_inbox::Error::Cancelled => "cancelled",
                        yoink_inbox::Error::Forbidden => "auth_failed",
                        yoink_inbox::Error::Completion(code) => code,
                        _ => "error",
                    };
                    self.store
                        .fail_job(&claim, now(), code, retry.then(|| now() + 1_000_000))
                        .await
                        .map_err(|_| ServerError::Inbox)?;
                }
            }
            tokio::select! {biased;_=cancel.cancelled()=>return Ok(()),_=tokio::time::sleep(self.config.interval)=>()}
        }
    }
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_micros()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn concrete_github_read_cancellation_is_not_external_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = GithubClient::new(Duration::from_secs(5), None)
            .unwrap()
            .with_endpoint(
                url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap(),
            )
            .unwrap();
        let cancel = CancellationToken::new();
        let github = CancelGithub {
            client,
            cancel: cancel.clone(),
        };
        let request = github.get_json(None, "repos/a/b");
        tokio::pin!(request);
        tokio::select! {r=&mut request=>panic!("request ended early: {r:?}"),socket=listener.accept()=>{let _socket=socket.unwrap();cancel.cancel();}}
        assert!(matches!(request.await, Err(yoink_inbox::Error::Cancelled)));
    }
    #[tokio::test]
    async fn owned_admitted_worker_shutdown_restores_attempt_and_releases_lease() {
        interrupted_job(false).await;
    }
    #[tokio::test]
    async fn repeated_job_deadlines_exhaust_budget_and_next_work_proceeds() {
        interrupted_job(true).await;
    }
    async fn interrupted_job(deadline: bool) {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let storage = yoink_storage::Store::open(
            &dir.path().join("runtime.db"),
            true,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        storage.migrate().await.unwrap();
        let identities = Arc::new(yoink_identity::IdentityStore::new(
            storage.database().clone(),
            Some(1),
            false,
        ));
        let workspace = dir.path().to_str().unwrap();
        let config = InboxConfig::from_lookup(
            &mut |key| {
                Ok(match key {
                    "INBOX_ENABLED" => Some("true".into()),
                    "INSIGHT_AI_PROXY" => Some("socks5://127.0.0.1:9".into()),
                    "INBOX_WORKSPACE" => Some(workspace.into()),
                    _ => None,
                })
            },
            Duration::from_secs(90),
            true,
        )
        .unwrap()
        .unwrap();
        identities
            .register(&yoink_identity::TrustedProfile {
                user_id: 1,
                username: None,
                first_name: None,
                photo_url: None,
            })
            .await
            .unwrap();
        yoink_db::query(
            "INSERT INTO insight_user_settings(user_id,github_token) VALUES(1,'fixture-token')",
        )
        .execute(storage.database())
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut runtime = InboxRuntime::new(config, &storage, identities, Some(1)).unwrap();
        runtime.github = runtime
            .github
            .with_endpoint(
                url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap(),
            )
            .unwrap();
        runtime
            .store
            .enqueue(1, None, "stars_sync", now())
            .await
            .unwrap();
        if deadline {
            runtime.config.job = Duration::from_millis(50);
            runtime.config.interval = Duration::from_millis(5);
            runtime.config.max_attempts = 3;
        }
        let cancel = CancellationToken::new();
        let future = runtime.run(cancel.clone());
        tokio::pin!(future);
        if deadline {
            let exercise = async {
                let first = yoink_db::query("SELECT id FROM inbox_jobs")
                    .fetch_one(storage.database())
                    .await
                    .unwrap()
                    .try_get::<i64, _>("id")
                    .unwrap();
                for attempt in 1..=3 {
                    let socket = listener.accept().await.unwrap();
                    // Keep the real GitHub transport stalled until recovery is durable.
                    wait_for_job(
                        &storage,
                        first,
                        if attempt == 3 { "failed" } else { "ready" },
                    )
                    .await;
                    let row = yoink_db::query("SELECT attempts,lease_token,lease_until,last_error FROM inbox_jobs WHERE id=?")
                        .bind(first).fetch_one(storage.database()).await.unwrap();
                    assert_eq!(row.try_get::<i64, _>("attempts").unwrap(), attempt);
                    assert!(
                        row.try_get::<Option<String>, _>("lease_token")
                            .unwrap()
                            .is_none()
                    );
                    assert!(
                        row.try_get::<Option<i64>, _>("lease_until")
                            .unwrap()
                            .is_none()
                    );
                    assert_eq!(
                        row.try_get::<String, _>("last_error").unwrap(),
                        "external operation failed"
                    );
                    drop(socket);
                    // Advance only persisted retry eligibility, not the production
                    // retry delay or wall clock, to keep this regression fast.
                    yoink_db::query("UPDATE inbox_jobs SET available_at=? WHERE id=?")
                        .bind(now())
                        .bind(first)
                        .execute(storage.database())
                        .await
                        .unwrap();
                }
                assert!(
                    tokio::time::timeout(Duration::from_millis(100), listener.accept())
                        .await
                        .is_err(),
                    "terminal job dispatched again"
                );
                let store = yoink_inbox::store::Store::new(storage.database().clone());
                store.enqueue(1, None, "stars_sync", now()).await.unwrap();
                let next = yoink_db::query("SELECT max(id) AS id FROM inbox_jobs")
                    .fetch_one(storage.database())
                    .await
                    .unwrap()
                    .try_get::<i64, _>("id")
                    .unwrap();
                assert_ne!(first, next);
                let (mut socket, _) = listener.accept().await.unwrap();
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut request = [0; 4096];
                let received = socket.read(&mut request).await.unwrap();
                assert!(received > 0, "next job did not send a GitHub request");
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]").await.unwrap();
                wait_for_job(&storage, next, "done").await;
                let rows = yoink_db::query(
                    "SELECT state,attempts,lease_token,lease_until FROM inbox_jobs ORDER BY id",
                )
                .fetch_all(storage.database())
                .await
                .unwrap();
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].try_get::<String, _>("state").unwrap(), "failed");
                assert_eq!(rows[0].try_get::<i64, _>("attempts").unwrap(), 3);
                assert_eq!(rows[1].try_get::<String, _>("state").unwrap(), "done");
                assert_eq!(rows[1].try_get::<i64, _>("attempts").unwrap(), 1);
                for row in rows {
                    assert!(
                        row.try_get::<Option<String>, _>("lease_token")
                            .unwrap()
                            .is_none()
                    );
                    assert!(
                        row.try_get::<Option<i64>, _>("lease_until")
                            .unwrap()
                            .is_none()
                    );
                }
            };
            tokio::select! {
                result = &mut future => panic!("worker ended early: {result:?}"),
                result = tokio::time::timeout(Duration::from_secs(5), exercise) => result.unwrap(),
            }
            cancel.cancel();
        } else {
            let socket = tokio::select! {r=&mut future=>panic!("worker ended early: {r:?}"),socket=listener.accept()=>socket.unwrap()};
            cancel.cancel();
            drop(socket);
        }
        tokio::time::timeout(Duration::from_secs(1), future)
            .await
            .unwrap()
            .unwrap();
        if !deadline {
            let row = yoink_db::query("SELECT state,attempts,lease_token FROM inbox_jobs")
                .fetch_one(storage.database())
                .await
                .unwrap();
            assert_eq!(row.try_get::<String, _>("state").unwrap(), "ready");
            assert_eq!(row.try_get::<i64, _>("attempts").unwrap(), 0);
            assert!(
                row.try_get::<Option<String>, _>("lease_token")
                    .unwrap()
                    .is_none()
            );
        }
        storage.close().await.unwrap();
    }

    async fn wait_for_job(storage: &yoink_storage::Store, id: i64, expected: &str) {
        loop {
            let row = yoink_db::query("SELECT state FROM inbox_jobs WHERE id=?")
                .bind(id)
                .fetch_one(storage.database())
                .await
                .unwrap();
            if row.try_get::<String, _>("state").unwrap() == expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}
