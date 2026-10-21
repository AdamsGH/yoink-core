use crate::*;
use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{Mutex, Semaphore},
    time::{Instant, timeout},
};
use tokio_util::sync::CancellationToken;
use yoink_access::{AccessPolicy, AccessScope, Role};
use yoink_executor::{Executor, ExecutorError, ValidatedArtifact};
use yoink_identity::IdentityStore;
use yoink_worker_protocol::{MusicRequest, PROTOCOL_VERSION};
pub type MusicFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, MusicError>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MusicActor {
    pub user_id: i64,
    pub group: Option<(i64, Option<i64>)>,
}
pub trait MusicAuthority: Send + Sync {
    fn allowed(&self, actor: MusicActor) -> MusicFuture<'_, bool>;
}
pub struct NativeMusicAuthority(pub Arc<IdentityStore>);
impl MusicAuthority for NativeMusicAuthority {
    fn allowed(&self, actor: MusicActor) -> MusicFuture<'_, bool> {
        Box::pin(async move {
            let Some(snapshot) = self
                .0
                .snapshot(actor.user_id, actor.group)
                .await
                .map_err(|_| MusicError::Persistence)?
            else {
                return Ok(false);
            };
            let policy = AccessPolicy {
                min_role: Role::Restricted,
                scopes: vec![AccessScope::Private, AccessScope::Group],
                check_group_enabled: true,
                check_thread_policy: true,
            };
            Ok(snapshot
                .authorize(&policy, Some(("music", "inline")))
                .decision
                .allowed)
        })
    }
}
pub trait MusicWorker: Send + Sync {
    /// Owners must complete worker reap/cleanup before product drain returns.
    fn shutdown(&self) -> MusicFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn lookup(
        &self,
        request: MusicRequest,
        cancel: CancellationToken,
        timeout: Duration,
    ) -> MusicFuture<'_, MusicResult>;
    fn audio(
        &self,
        request: MusicRequest,
        cancel: CancellationToken,
        timeout: Duration,
        max_bytes: u64,
    ) -> MusicFuture<'_, Vec<ValidatedArtifact>>;
}
fn map_executor(e: ExecutorError) -> MusicError {
    if !matches!(e, ExecutorError::Cancelled) {
        tracing::warn!(failure = ?e, "Music worker execution failed");
    }
    match e {
        ExecutorError::Cancelled => MusicError::Cancelled,
        ExecutorError::ResourceLimit
        | ExecutorError::Worker(yoink_worker_protocol::FailureClass::ResourceLimit) => {
            MusicError::ResourceLimit
        }
        _ => MusicError::SourceUnavailable,
    }
}
impl MusicWorker for Executor {
    fn shutdown(&self) -> MusicFuture<'_, ()> {
        Box::pin(async move {
            self.shutdown_music().await;
            Ok(())
        })
    }
    fn lookup(
        &self,
        request: MusicRequest,
        cancel: CancellationToken,
        budget: Duration,
    ) -> MusicFuture<'_, MusicResult> {
        Box::pin(async move {
            self.execute_music(&request, cancel, budget)
                .await
                .map_err(map_executor)
        })
    }
    fn audio(
        &self,
        request: MusicRequest,
        cancel: CancellationToken,
        budget: Duration,
        max_bytes: u64,
    ) -> MusicFuture<'_, Vec<ValidatedArtifact>> {
        Box::pin(async move {
            self.with_operation_limits(budget, max_bytes, 1)
                .map_err(map_executor)?
                .execute_music_audio(&request, cancel)
                .await
                .map_err(map_executor)
        })
    }
}
pub trait ResolveAnalytics: Send + Sync {
    fn record(&self, actor: i64, track: Track) -> MusicFuture<'_, ()>;
}
#[derive(Clone)]
pub struct MusicSettings {
    pub request_timeout: Duration,
    pub lookup_timeout: Duration,
    pub resolve_timeout: Duration,
    pub audio_timeout: Duration,
    pub delivery_timeout: Duration,
    pub persistence_timeout: Duration,
    pub cache_ttl: Duration,
    pub cache_entries: usize,
    pub worker_concurrency: usize,
    pub response_max_bytes: u32,
    pub max_requests: u16,
    pub audio_max_bytes: u64,
    pub max_links_per_command: usize,
    pub download_enabled: bool,
    pub credentials: MusicCredentials,
    pub proxies: BTreeMap<MusicProvider, String>,
}
impl MusicSettings {
    pub fn validate(&self) -> Result<(), MusicError> {
        if [
            self.request_timeout,
            self.lookup_timeout,
            self.resolve_timeout,
            self.audio_timeout,
            self.delivery_timeout,
            self.persistence_timeout,
            self.cache_ttl,
        ]
        .iter()
        .any(|d| d.is_zero() || *d > Duration::from_secs(86400))
            || self.lookup_timeout > Duration::from_secs(120)
            || self.request_timeout > Duration::from_secs(120)
            || self.audio_timeout > Duration::from_secs(3600)
            || self.audio_timeout.subsec_nanos() != 0
            || !(1..=10000).contains(&self.cache_entries)
            || !(1..=32).contains(&self.worker_concurrency)
            || !(1024..=8388608).contains(&self.response_max_bytes)
            || !(1..=100).contains(&self.max_requests)
            || self.audio_max_bytes == 0
            || self.audio_max_bytes > 49 * 1024 * 1024
            || !(1..=10).contains(&self.max_links_per_command)
        {
            return Err(MusicError::InvalidInput);
        }
        for p in MusicProvider::ALL {
            yoink_worker_protocol::encode_music_request(&self.request(
                MusicTask::MusicSearch {
                    provider: p,
                    query: "test".into(),
                    title: "test".into(),
                    artist: String::new(),
                },
                p,
            ))
            .map_err(|_| MusicError::InvalidInput)?;
        }
        Ok(())
    }
    pub(crate) fn request(&self, task: MusicTask, provider: MusicProvider) -> MusicRequest {
        MusicRequest {
            version: PROTOCOL_VERSION,
            operation_id: uuid::Uuid::new_v4(),
            attempt: 1,
            task,
            proxy_url: self.proxies.get(&provider).cloned(),
            credentials: self.credentials.clone(),
            request_timeout_ms: self.request_timeout.as_millis() as u32,
            response_max_bytes: self.response_max_bytes,
            max_requests: self.max_requests,
        }
    }
}
type ResolutionCache = VecDeque<(String, Instant, Resolution)>;
#[derive(Clone)]
pub struct MusicService {
    pub(crate) authority: Arc<dyn MusicAuthority>,
    pub(crate) worker: Arc<dyn MusicWorker>,
    pub(crate) settings: Arc<MusicSettings>,
    analytics: Arc<dyn ResolveAnalytics>,
    cache: Arc<Mutex<ResolutionCache>>,
    pub(crate) slots: Arc<Semaphore>,
    tasks: tokio_util::task::TaskTracker,
    stop: CancellationToken,
    admission: Arc<std::sync::Mutex<()>>,
}
impl MusicService {
    pub fn new(
        authority: Arc<dyn MusicAuthority>,
        worker: Arc<dyn MusicWorker>,
        analytics: Arc<dyn ResolveAnalytics>,
        settings: MusicSettings,
    ) -> Result<Self, MusicError> {
        settings.validate()?;
        Ok(Self {
            authority,
            worker,
            analytics,
            slots: Arc::new(Semaphore::new(settings.worker_concurrency)),
            settings: Arc::new(settings),
            cache: Arc::new(Mutex::new(VecDeque::new())),
            tasks: tokio_util::task::TaskTracker::new(),
            stop: CancellationToken::new(),
            admission: Arc::new(std::sync::Mutex::new(())),
        })
    }
    pub fn persistence_timeout(&self) -> Duration {
        self.settings.persistence_timeout
    }
    pub(crate) async fn persistence_admission<T>(
        &self,
        cancel: &CancellationToken,
        future: impl std::future::Future<Output = Result<T, MusicError>>,
    ) -> Result<T, MusicError> {
        tokio::select! {
            _ = cancel.cancelled() => Err(MusicError::Cancelled),
            _ = self.stop.cancelled() => Err(MusicError::Cancelled),
            result = timeout(self.settings.persistence_timeout,future) => result.map_err(|_|MusicError::Persistence)?,
        }
    }
    pub fn cancel_all(&self) {
        let _admission = self
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.stop.cancel();
        self.tasks.close();
    }
    pub async fn shutdown(&self) -> Result<(), MusicError> {
        self.cancel_all();
        self.tasks.wait().await;
        self.worker.shutdown().await
    }
    pub(crate) async fn authorize(&self, actor: MusicActor) -> Result<(), MusicError> {
        tokio::select! {
            _ = self.stop.cancelled() => Err(MusicError::Cancelled),
            result = self.authorize_inner(actor) => result,
        }
    }
    async fn authorize_inner(&self, actor: MusicActor) -> Result<(), MusicError> {
        if actor.user_id <= 0
            || !timeout(
                self.settings.persistence_timeout,
                self.authority.allowed(actor),
            )
            .await
            .map_err(|_| MusicError::Persistence)??
        {
            return Err(MusicError::Denied);
        }
        Ok(())
    }
    pub(crate) async fn lookup(
        &self,
        task: MusicTask,
        provider: MusicProvider,
        cancel: CancellationToken,
    ) -> Result<MusicResult, MusicError> {
        let result = tokio::select! { _=cancel.cancelled()=>Err(MusicError::Cancelled), result=async {
            let _slot = self.slots.acquire().await.map_err(|_|MusicError::Cancelled)?;
            timeout(self.settings.lookup_timeout,self.worker.lookup(self.settings.request(task,provider),cancel.clone(),self.settings.lookup_timeout)).await.map_err(|_|MusicError::SourceUnavailable)?
        } => result };
        if let Err(error) = &result
            && !matches!(error, MusicError::Cancelled | MusicError::Denied)
        {
            tracing::warn!(provider = provider.key(), error = ?error, "Music lookup failed");
        }
        result
    }
    pub async fn resolve(
        &self,
        actor: MusicActor,
        input: &str,
        cancel: CancellationToken,
    ) -> Result<Resolution, MusicError> {
        let owned_cancel = cancel.child_token();
        let drop_cancel = owned_cancel.clone().drop_guard();
        let service = self.clone();
        let handle = {
            let _admission = self
                .admission
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.stop.is_cancelled() {
                return Err(MusicError::Cancelled);
            }
            let input = input.to_owned();
            self.tasks.spawn(async move {
                tokio::select! {
                    _ = service.stop.cancelled() => { owned_cancel.cancel(); Err(MusicError::Cancelled) },
                    result = service.resolve_inner(actor, &input, owned_cancel.clone()) => result,
                }
            })
        };
        let result = handle.await.map_err(|_| MusicError::Cancelled)?;
        drop_cancel.disarm();
        result
    }
    async fn resolve_inner(
        &self,
        actor: MusicActor,
        input: &str,
        cancel: CancellationToken,
    ) -> Result<Resolution, MusicError> {
        self.authorize(actor).await?;
        if cancel.is_cancelled() {
            return Err(MusicError::Cancelled);
        }
        let source = parse_link(input)?;
        let key = source.url.as_url().to_string();
        let cached = {
            let mut cache = self.cache.lock().await;
            cache.retain(|(_, t, _)| t.elapsed() < self.settings.cache_ttl);
            cache
                .iter()
                .find(|(k, _, _)| k == &key)
                .map(|(_, _, r)| r.clone())
        };
        let result = if let Some(r) = cached {
            r
        } else {
            let r = tokio::select! { _=cancel.cancelled()=>return Err(MusicError::Cancelled), r=timeout(self.settings.resolve_timeout,self.resolve_uncached(source,cancel.clone()))=>r.map_err(|_|MusicError::SourceUnavailable)?? };
            let mut cache = self.cache.lock().await;
            if cache.len() >= self.settings.cache_entries {
                cache.pop_front();
            }
            cache.push_back((key, Instant::now(), r.clone()));
            r
        };
        self.authorize(actor).await?;
        if let Resolution::Track(t) = &result
            && !matches!(
                timeout(
                    self.settings.persistence_timeout,
                    self.analytics.record(actor.user_id, t.clone())
                )
                .await,
                Ok(Ok(()))
            )
        {
            tracing::warn!("music resolve analytics persistence failed");
        }
        Ok(result)
    }
    async fn resolve_uncached(
        &self,
        source: MusicLink,
        cancel: CancellationToken,
    ) -> Result<Resolution, MusicError> {
        if source.provider == MusicProvider::Spotify
            && source.url.as_url().path().split('/').any(|p| p == "artist")
        {
            let result = self
                .lookup(
                    MusicTask::MusicArtist {
                        url: source.url.clone(),
                    },
                    source.provider,
                    cancel,
                )
                .await?;
            if !matches!(result, MusicResult::Artist { .. }) || !result.valid() {
                return Err(MusicError::SourceUnavailable);
            }
            return Ok(Resolution::Artist {
                source: source.url,
                result,
            });
        }
        let MusicResult::Metadata { mut metadata } = self
            .lookup(
                MusicTask::MusicParse {
                    provider: source.provider,
                    url: source.url.clone(),
                },
                source.provider,
                cancel.clone(),
            )
            .await?
        else {
            return Err(MusicError::SourceUnavailable);
        };
        if !(MusicResult::Metadata {
            metadata: metadata.clone(),
        })
        .valid()
        {
            return Err(MusicError::SourceUnavailable);
        }
        let query = metadata
            .artist
            .as_ref()
            .map(|a| format!("{a} {}", metadata.title))
            .unwrap_or_else(|| metadata.title.clone());
        let mut tasks = Vec::new();
        let search_cancel = cancel.child_token();
        let _search_guard = search_cancel.clone().drop_guard();
        for provider in MusicProvider::ALL {
            if provider == source.provider || provider == MusicProvider::Youtube {
                continue;
            }
            let service = self.clone();
            let cancel = search_cancel.clone();
            let task = MusicTask::MusicSearch {
                provider,
                query: query.clone(),
                title: metadata.title.clone(),
                artist: metadata.artist.clone().unwrap_or_default(),
            };
            tasks
                .push(self.tasks.spawn(async move {
                    (provider, service.lookup(task, provider, cancel).await)
                }));
        }
        let mut found = BTreeMap::new();
        for task in tasks {
            let result = task.await;
            if let Ok((provider, Ok(MusicResult::Link { url: Some(url) }))) = result
                && let Ok(link) = parse_link(url.as_url().as_str())
                && link.provider == provider
            {
                found.insert(provider, link);
            }
        }
        if cancel.is_cancelled() {
            return Err(MusicError::Cancelled);
        }
        if metadata.artist.as_ref().is_none_or(|a| a.is_empty())
            && let Some(link) = found.get(&MusicProvider::Deezer)
            && let Ok(MusicResult::Metadata { metadata: hint }) = self
                .lookup(
                    MusicTask::MusicParse {
                        provider: MusicProvider::Deezer,
                        url: link.url.clone(),
                    },
                    MusicProvider::Deezer,
                    cancel,
                )
                .await
        {
            metadata.artist = hint.artist;
        }
        let mut links = vec![source.clone()];
        for provider in MusicProvider::ALL {
            if let Some(link) = found.remove(&provider) {
                links.push(link);
            }
        }
        Ok(Resolution::Track(Track {
            metadata,
            source,
            links,
        }))
    }
    /// Transport adapters send each card before requesting its optional audio.
    pub async fn command(
        &self,
        actor: MusicActor,
        text: &str,
        hidden: &[String],
        cancel: CancellationToken,
    ) -> Result<Vec<Resolution>, MusicError> {
        self.authorize(actor).await?;
        let links = extract_links(text, hidden, self.settings.max_links_per_command);
        if links.is_empty() {
            return Err(MusicError::InvalidInput);
        }
        let mut results = Vec::new();
        let mut failure = MusicError::SourceUnavailable;
        for link in links {
            match self
                .resolve(actor, link.url.as_url().as_str(), cancel.clone())
                .await
            {
                Ok(r) => results.push(r),
                Err(MusicError::Denied) => return Err(MusicError::Denied),
                Err(MusicError::Cancelled) => return Err(MusicError::Cancelled),
                Err(error) => failure = error,
            }
        }
        self.authorize(actor).await?;
        if results.is_empty() {
            Err(failure)
        } else {
            Ok(results)
        }
    }
    pub async fn inline(
        &self,
        user_id: i64,
        query: &str,
        cancel: CancellationToken,
    ) -> Result<InlineAnswer, MusicError> {
        let actor = MusicActor {
            user_id,
            group: None,
        };
        self.authorize(actor).await?;
        let results = if query.trim().is_empty() {
            Vec::new()
        } else {
            self.command(actor, query, &[], cancel)
                .await?
                .iter()
                .map(MusicCard::from_resolution)
                .collect()
        };
        self.authorize(actor).await?;
        Ok(InlineAnswer {
            results,
            is_personal: true,
            cache_time: 0,
        })
    }
}
#[derive(Debug, Clone)]
pub struct InlineAnswer {
    pub results: Vec<MusicCard>,
    pub is_personal: bool,
    pub cache_time: u32,
}
