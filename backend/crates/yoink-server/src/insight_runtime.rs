//! Composition owns concrete lifecycle handles; clones use one service.
use crate::{insight_config::InsightConfig, runtime::ServerError};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
pub struct InsightRuntime {
    pub service: yoink_insight::Insight,
    pub content: yoink_insight::worker_content::WorkerContent,
    pub oauth: yoink_insight::oauth::DeviceOAuth,
}
impl InsightRuntime {
    pub fn new(
        config: InsightConfig,
        storage: &yoink_storage::Store,
        identities: Arc<yoink_identity::IdentityStore>,
        owner: Option<i64>,
        shutdown: CancellationToken,
    ) -> Result<Self, ServerError> {
        let completion = yoink_insight::provider::HttpCompletion::new(
            config.service.request_timeout,
            config.response_bytes,
        )
        .map_err(|_| ServerError::InsightStartup)?;
        Self::compose(config, storage, identities, owner, shutdown, completion)
    }

    #[cfg(feature = "test-fixtures")]
    pub fn with_fixture_root(
        config: InsightConfig,
        storage: &yoink_storage::Store,
        identities: Arc<yoink_identity::IdentityStore>,
        owner: Option<i64>,
        shutdown: CancellationToken,
        root_der: &[u8],
    ) -> Result<Self, ServerError> {
        let completion = yoink_insight::provider::HttpCompletion::with_additional_roots(
            config.service.request_timeout,
            config.response_bytes,
            &[root_der],
        )
        .map_err(|_| ServerError::InsightStartup)?;
        Self::compose(config, storage, identities, owner, shutdown, completion)
    }

    fn compose(
        mut config: InsightConfig,
        storage: &yoink_storage::Store,
        identities: Arc<yoink_identity::IdentityStore>,
        owner: Option<i64>,
        shutdown: CancellationToken,
        mut completion: yoink_insight::provider::HttpCompletion,
    ) -> Result<Self, ServerError> {
        config.service.owner_id = owner;
        let executor = yoink_executor::Executor::new(config.executor)
            .map_err(|_| ServerError::InsightStartup)?;
        let content = yoink_insight::worker_content::WorkerContent::new(
            executor,
            config.content_timeout,
            config.response_bytes,
            config.content_proxy,
        )
        .map_err(|_| ServerError::InsightStartup)?;
        if let Some(proxy) = config.openrouter_proxy {
            completion = completion
                .with_operator_openrouter_proxy(&config.service.openrouter_base, &proxy)
                .map_err(|_| ServerError::InsightStartup)?;
        }
        let research = yoink_insight::research::ResponsesResearch::new(
            config.research_timeout,
            config.response_bytes,
        )
        .map_err(|_| ServerError::InsightStartup)?;
        let service = yoink_insight::Insight::new(
            storage.database().clone(),
            config.service,
            Arc::new(content.clone()),
            Arc::new(completion),
        )
        .map_err(|_| ServerError::InsightStartup)?
        .with_research(Arc::new(research))
        .with_access_change_callback(Arc::new(move |_| {
            identities.notify_committed_access_change()
        }));
        let oauth = yoink_insight::oauth::DeviceOAuth::with_options(
            service.clone(),
            config.read_client,
            config.write_client,
            shutdown,
            config.oauth,
        )
        .map_err(|_| ServerError::InsightStartup)?;
        Ok(Self {
            service,
            content,
            oauth,
        })
    }
    pub fn http(&self, shutdown: CancellationToken) -> yoink_insight::http::HttpState {
        yoink_insight::http::HttpState {
            insight: self.service.clone(),
            oauth: self.oauth.clone(),
            shutdown,
        }
    }
    /// Must be awaited even after task-drain timeout, before database close.
    pub async fn shutdown(&self) {
        tokio::join!(self.oauth.shutdown(), self.content.shutdown());
        self.service.shutdown_quota_cleanup().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn full_service_shared_inbox_callback_and_shutdown_fence() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let store = yoink_storage::Store::open(
            &dir.path().join("native.db"),
            true,
            std::time::Duration::from_secs(1),
        )
        .await
        .unwrap();
        store.migrate().await.unwrap();
        let identities = Arc::new(yoink_identity::IdentityStore::new(
            store.database().clone(),
            Some(1),
            true,
        ));
        for user_id in [1, 2] {
            identities
                .register(&yoink_identity::TrustedProfile {
                    user_id,
                    username: None,
                    first_name: None,
                    photo_url: None,
                })
                .await
                .unwrap();
        }
        let mut changes = identities.subscribe_changes();
        changes.borrow_and_update();
        let workspace = dir.path().to_str().unwrap();
        let config = InsightConfig::from_lookup(
            &mut |key| {
                Ok(match key {
                    "INSIGHT_ENABLED" => Some("true".into()),
                    "INSIGHT_WORKSPACE" => Some(workspace.into()),
                    _ => None,
                })
            },
            std::time::Duration::from_secs(150),
            true,
        )
        .unwrap()
        .unwrap();
        let cancel = CancellationToken::new();
        let runtime =
            InsightRuntime::new(config, &store, identities.clone(), Some(1), cancel.clone())
                .unwrap();
        let inbox_config = crate::inbox_config::InboxConfig::from_lookup_with_insight(
            &mut |key| {
                Ok(match key {
                    "INBOX_ENABLED" => Some("true".into()),
                    "INBOX_WORKSPACE" => Some(workspace.into()),
                    _ => None,
                })
            },
            std::time::Duration::from_secs(150),
            true,
            Some(&runtime.service.config),
        )
        .unwrap()
        .unwrap();
        let inbox = crate::inbox_runtime::InboxRuntime::new_with_insight(
            inbox_config,
            &store,
            identities,
            Some(1),
            Some(runtime.service.clone()),
        )
        .unwrap();
        assert!(Arc::ptr_eq(
            &runtime.service.completion,
            &inbox.insight.completion
        ));
        assert!(Arc::ptr_eq(
            runtime.service.content.as_ref().unwrap(),
            inbox.insight.content.as_ref().unwrap()
        ));
        assert!(runtime.service.research.is_some());
        inbox.insight.grant(1, 2, "en").await.unwrap();
        assert!(
            changes.has_changed().unwrap(),
            "committed mutation must notify shared identity through Inbox clone"
        );
        cancel.cancel();
        runtime.shutdown().await;
        runtime.shutdown().await; // joins are idempotent, including failure cleanup
        use yoink_insight::content::{ContentSource, FetchOptions};
        let result = runtime
            .content
            .fetch(
                "https://example.com/",
                FetchOptions {
                    transcript_only: false,
                    transcript_languages: &[],
                    max_chars: 1000,
                    github_token: None,
                },
                CancellationToken::new(),
            )
            .await;
        assert_eq!(
            result.unwrap_err().to_string(),
            "cancelled",
            "joined worker must fence later admissions without spawning"
        );
        let closed = inbox
            .insight
            .summarize(
                2,
                yoink_insight::Request {
                    command: "tldr".into(),
                    url: "invalid-source".into(),
                    alias: None,
                    focus: None,
                },
                CancellationToken::new(),
            )
            .await;
        assert_eq!(
            closed.unwrap_err().to_string(),
            "shutting_down",
            "concrete runtime shutdown must close quota admission on the Inbox clone"
        );
        store.close().await.unwrap();
    }
}
