//! Product adapter for the immutable shared full-fetch executor.
use crate::{
    Error, FutureResult,
    content::{Citation, Content, ContentSource, FetchOptions, normalize},
    provider::Route,
    research::Research,
};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use yoink_executor::{Executor, ExecutorError};
use yoink_worker_protocol::{
    FullContentError, FullContentOptions, FullContentRequest, FullContentTask, HttpSourceUrl,
    PROTOCOL_VERSION, full_content_token_relevant,
};

/// Resolve the currently configured research route per call (including DB/BYOK).
/// Provider secrets are never passed to the content worker.
pub trait ResearchRoute: Send + Sync {
    fn resolve(&self) -> FutureResult<'_, Route>;
}
#[derive(Clone)]
pub struct WorkerContent {
    executor: Executor,
    timeout: Duration,
    max_response_bytes: usize,
    proxy_url: Option<String>,
    research: Option<(Arc<dyn Research>, Arc<dyn ResearchRoute>)>,
}
impl WorkerContent {
    /// Search is explicitly disabled until with_research installs a real adapter.
    /// No executable or argument configuration is accepted by this bridge.
    pub fn new(
        executor: Executor,
        timeout: Duration,
        max_response_bytes: usize,
        proxy_url: Option<String>,
    ) -> Result<Self, Error> {
        if timeout.is_zero()
            || timeout > Duration::from_secs(300)
            || !(1024..=8_388_608).contains(&max_response_bytes)
        {
            return Err(Error::Code("invalid_request"));
        }
        Ok(Self {
            executor,
            timeout,
            max_response_bytes,
            proxy_url,
            research: None,
        })
    }
    pub fn with_research(
        mut self,
        research: Arc<dyn Research>,
        routes: Arc<dyn ResearchRoute>,
    ) -> Self {
        self.research = Some((research, routes));
        self
    }
    /// Join tracked cleanup before shutting down the asynchronous runtime.
    pub async fn shutdown(&self) {
        self.executor.shutdown_full_content().await;
    }
}
impl ContentSource for WorkerContent {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        options: FetchOptions<'a>,
        cancel: CancellationToken,
    ) -> FutureResult<'a, Content> {
        Box::pin(async move {
            let url = normalize(url)?;
            let url =
                HttpSourceUrl::parse(url.as_str()).map_err(|_| Error::Code("invalid_request"))?;
            let token = options
                .github_token
                .filter(|_| full_content_token_relevant(&url))
                .map(str::to_owned);
            let request = FullContentRequest {
                version: PROTOCOL_VERSION,
                operation_id: uuid::Uuid::new_v4(),
                attempt: 1,
                task: FullContentTask::FullContentFetch,
                url,
                options: FullContentOptions {
                    transcript_only: options.transcript_only,
                    transcript_languages: options.transcript_languages.to_vec(),
                    max_chars: options.max_chars,
                    github_token: token,
                },
                timeout_secs: self.timeout.as_secs_f64(),
                max_response_bytes: self.max_response_bytes,
                proxy_url: self.proxy_url.clone(),
            };
            let result = self
                .executor
                .execute_full_content(&request, cancel)
                .await
                .map_err(map_error)?;
            Ok(Content {
                text: result.text,
                title: result.title,
                video_id: result.video_id,
                video_seconds: result.video_seconds,
                via: result.via,
            })
        })
    }
    fn research<'a>(
        &'a self,
        query: &'a str,
        cancel: CancellationToken,
    ) -> FutureResult<'a, Vec<Citation>> {
        Box::pin(async move {
            let Some((research, routes)) = &self.research else {
                return Err(Error::Code("research_unavailable"));
            };
            let route = tokio::select! { biased; _ = cancel.cancelled() => return Err(Error::Code("cancelled")), route = routes.resolve() => route? };
            research.search(&route, query, cancel).await
        })
    }
}
fn map_error(error: ExecutorError) -> Error {
    Error::Code(match error {
        ExecutorError::FullContent(e) => match e {
            FullContentError::NoContent => "no_content",
            FullContentError::NoTranscript => "no_transcript",
            FullContentError::ContentNetworkDenied => "content_network_denied",
            FullContentError::ContentTimeout => "content_timeout",
            FullContentError::ContentTooLarge => "content_too_large",
            FullContentError::ContentHttpError => "content_http_error",
            FullContentError::ContentRedirectLimit => "content_redirect_limit",
            FullContentError::InvalidRequest => "invalid_request",
            FullContentError::ContentInvalidResponse => "content_invalid_response",
        },
        ExecutorError::Cancelled => "cancelled",
        ExecutorError::Timeout => "content_timeout",
        ExecutorError::Spawn | ExecutorError::InvalidConfig => "content_worker_unavailable",
        ExecutorError::ResourceLimit => "content_worker_resource_limit",
        ExecutorError::Protocol | ExecutorError::UnsafeArtifact => "content_worker_protocol_error",
        _ => "content_worker_failed",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_and_worker_failures_remain_distinct_from_content_errors() {
        for (error, expected) in [
            (ExecutorError::Cancelled, "cancelled"),
            (ExecutorError::Timeout, "content_timeout"),
            (ExecutorError::Spawn, "content_worker_unavailable"),
            (
                ExecutorError::ResourceLimit,
                "content_worker_resource_limit",
            ),
            (ExecutorError::Protocol, "content_worker_protocol_error"),
            (
                ExecutorError::FullContent(FullContentError::ContentTooLarge),
                "content_too_large",
            ),
            (
                ExecutorError::FullContent(FullContentError::NoTranscript),
                "no_transcript",
            ),
        ] {
            assert_eq!(map_error(error).to_string(), expected);
        }
    }
    #[tokio::test]
    async fn disabled_search_is_explicitly_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let executor = Executor::new(yoink_executor::ExecutorConfig {
            program: "/bin/false".into(),
            args: vec![],
            workspace_root: root.path().to_path_buf(),
            timeout: Duration::from_secs(2),
            processing_timeout: Duration::from_secs(2),
            socket_timeout: Duration::from_secs(1),
            max_artifact_bytes: 1024,
            max_workspace_bytes: 1024,
            max_output_bytes: 1024,
            max_artifacts: 1,
            allowed_proxy_origins: vec![],
            allowed_private_origins: vec![],
        })
        .unwrap();
        let adapter = WorkerContent::new(executor, Duration::from_secs(2), 1024, None).unwrap();
        assert_eq!(
            adapter
                .research("query", CancellationToken::new())
                .await
                .unwrap_err()
                .to_string(),
            "research_unavailable"
        );
        adapter.shutdown().await;
    }
}
