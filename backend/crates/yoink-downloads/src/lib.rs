mod cache;
mod cache_control;
mod chat_access;
mod cut;
mod language;
pub use cut::{CutSession, CutStage, CutSubmission};
pub use yoink_identity::LanguagePreference;
mod personal_cookies;
pub mod profile;
mod progress;
mod quality;
pub use personal_cookies::{
    CookieDomain, PersonalCookieEntry, PersonalCookiePage, PersonalCookieStatus,
};
pub use quality::{QualityChoice, QualitySession, QualityStage, TelegramSubmission};
mod settings;
pub use settings::{EditableSettings, ProxySettings, SettingChange, SettingField};

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use sha2::{Digest, Sha256};

use thiserror::Error;
use tokio::{sync::mpsc, task::JoinSet, time};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
use uuid::Uuid;
use yoink_domain::CachedMedia;
use yoink_domain::{
    DeliveryTarget, DownloadParams, DownloadPreferenceOverrides, DownloadRequest,
    MediaMode as DomainMediaMode, NetworkSelection,
};
use yoink_executor::{Executor, ExecutorError, ValidatedArtifact};
use yoink_identity::IdentityStore;
use yoink_storage::{Admission, AdmissionPolicy, Claim, OperationRecord, Store, StoreError};
use yoink_worker_protocol::{
    HttpSourceUrl, MediaMode, NetworkCredentials, PROTOCOL_VERSION, TaskKind, WorkerRequest,
};

use profile::{NetworkContext, ProfileError, ProfileStore, normalize_domain};

pub type DeliveryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<DeliveredMedia>, DeliveryError>> + Send + 'a>>;

#[derive(Clone, Eq, PartialEq)]
pub struct DeliveredMedia {
    pub message_id: i64,
    pub file_id: Option<String>,
}

impl std::fmt::Debug for DeliveredMedia {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeliveredMedia")
            .field("message_id", &self.message_id)
            .field("file_id", &self.file_id.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

pub trait Delivery: Send + Sync {
    fn bot_id(&self) -> i64;
    fn deliver(
        &self,
        target: DeliveryTarget,
        send_as_file: bool,
        artifacts: Vec<DeliveryArtifact>,
    ) -> DeliveryFuture<'_>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryError {
    Rejected,
    InvalidFileId,
    RetryAfter(Duration),
    Unknown,
}

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Rejected => "delivery was rejected",
            Self::InvalidFileId => "cached media identifier was rejected",
            Self::RetryAfter(_) => "delivery was rate limited",
            Self::Unknown => "delivery outcome is unknown",
        })
    }
}
impl std::error::Error for DeliveryError {}

#[derive(Debug)]
pub enum DeliveryArtifact {
    Upload(ValidatedArtifact),
    Cached(CachedMedia),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadConfig {
    pub admission: AdmissionPolicy,
    pub concurrency: usize,
    pub lease: Duration,
    pub heartbeat_interval: Duration,
    pub poll_interval: Duration,
    pub delivery_timeout: Duration,
    pub cache_ttl: Duration,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            admission: AdmissionPolicy {
                max_attempts: 3,
                retry_delay: Duration::from_secs(2),
                max_pending_global: 50,
                max_pending_actor: 5,
            },
            concurrency: 2,
            lease: Duration::from_secs(30),
            heartbeat_interval: Duration::from_secs(5),
            poll_interval: Duration::from_millis(100),
            delivery_timeout: Duration::from_secs(20),
            cache_ttl: Duration::from_secs(2_592_000),
        }
    }
}

#[derive(Clone)]
pub struct DownloadService {
    store: Arc<Store>,
    identities: Arc<IdentityStore>,
    executor: Executor,
    delivery: Arc<dyn Delivery>,
    config: DownloadConfig,
    profile: ProfileStore,
    network_config: DownloadNetworkConfig,
    groups_enabled: bool,
}

#[derive(Clone, Eq, PartialEq)]
pub struct DownloadNetworkConfig {
    system_proxy: Option<String>,
    no_cookie_domains: Vec<String>,
}

impl std::fmt::Debug for DownloadNetworkConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadNetworkConfig")
            .field(
                "system_proxy",
                &self.system_proxy.as_ref().map(|_| "[REDACTED]"),
            )
            .field("no_cookie_domains", &self.no_cookie_domains)
            .finish()
    }
}

impl DownloadNetworkConfig {
    pub fn new(
        system_proxy: Option<String>,
        no_cookie_domains: Vec<String>,
    ) -> Result<Self, DownloadError> {
        if no_cookie_domains.len() > 64 {
            return Err(DownloadError::InvalidConfig);
        }
        let mut normalized = Vec::with_capacity(no_cookie_domains.len());
        for domain in no_cookie_domains {
            if domain.is_empty()
                || domain
                    .chars()
                    .any(|c| c.is_whitespace() || "/?#@".contains(c))
                || domain.contains("://")
                || (domain.parse::<std::net::IpAddr>().is_err() && domain.contains(':'))
            {
                return Err(DownloadError::InvalidConfig);
            }
            normalized.push(normalize_domain(&domain).map_err(|_| DownloadError::InvalidConfig)?);
        }
        normalized.sort();
        normalized.dedup();
        if let Some(proxy) = &system_proxy {
            NetworkCredentials {
                proxy_url: Some(proxy.clone()),
                cookie_jar: None,
            }
            .validate()
            .map_err(|_| DownloadError::InvalidConfig)?;
        }
        Ok(Self {
            system_proxy,
            no_cookie_domains: normalized,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum DownloadError {
    #[error("download input is invalid")]
    InvalidInput,
    #[error("download preferences contain invalid values")]
    InvalidPreferences,
    #[error("download credentials are invalid")]
    InvalidCredentials,
    #[error("proxy is enabled but no proxy is configured")]
    ProxyUnavailable,
    #[error("the selected authentication method is unsupported")]
    UnsupportedAuthentication,
    #[error("selected network credentials have changed or are no longer authorized")]
    ChangedCredentials,
    #[error("download service configuration is invalid")]
    InvalidConfig,
    #[error("actor is not authorized")]
    Denied,
    #[error("download capacity has been reached")]
    Capacity,
    #[error("idempotency key conflicts with an existing request")]
    Conflict,
    #[error("download storage operation failed")]
    Database,
    #[error("download executor operation failed")]
    Executor,
    #[error("download service task failed")]
    Task,
}

impl DownloadService {
    pub fn new(
        store: Arc<Store>,
        identities: Arc<IdentityStore>,
        executor: Executor,
        delivery: Arc<dyn Delivery>,
        config: DownloadConfig,
    ) -> Result<Self, DownloadError> {
        Self::new_with_network_config(
            store,
            identities,
            executor,
            delivery,
            config,
            DownloadNetworkConfig::new(None, vec![])?,
        )
    }

    pub fn new_with_network_config(
        store: Arc<Store>,
        identities: Arc<IdentityStore>,
        executor: Executor,
        delivery: Arc<dyn Delivery>,
        config: DownloadConfig,
        network_config: DownloadNetworkConfig,
    ) -> Result<Self, DownloadError> {
        validate_config(&config)?;
        if delivery.bot_id() <= 0 {
            return Err(DownloadError::InvalidConfig);
        }
        let profile = ProfileStore::new(store.database().clone());
        Ok(Self {
            store,
            identities,
            executor,
            delivery,
            config,
            profile,
            network_config,
            groups_enabled: false,
        })
    }

    pub fn with_network_config(mut self, config: DownloadNetworkConfig) -> Self {
        self.network_config = config;
        self
    }

    pub async fn check_profile_schema(&self) -> Result<(), DownloadError> {
        self.profile.check_schema().await.map_err(map_profile)
    }

    pub async fn submit(
        &self,
        actor_id: u64,
        source_url: &str,
        idempotency_key: &str,
    ) -> Result<Admission, DownloadError> {
        let target = DeliveryTarget::new(
            self.delivery.bot_id(),
            i64::try_from(actor_id).map_err(|_| DownloadError::InvalidInput)?,
            None,
            None,
        )
        .map_err(|_| DownloadError::InvalidInput)?;
        let request = DownloadRequest::new(
            actor_id,
            source_url,
            DomainMediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .map_err(|_| DownloadError::InvalidInput)?
        .with_target(target)
        .map_err(|_| DownloadError::InvalidInput)?;
        self.submit_request(&request, idempotency_key).await
    }

    pub async fn submit_preferred_request(
        &self,
        request: &DownloadRequest,
        idempotency_key: &str,
        overrides: DownloadPreferenceOverrides,
    ) -> Result<Admission, DownloadError> {
        if !request_format_defaults(request)
            || (request.media_mode() == DomainMediaMode::Formats
                && overrides != DownloadPreferenceOverrides::default())
            || request.preference_overrides().is_some()
            || request.source_url().as_str().len() > 4096
            || request
                .target()
                .is_none_or(|target| target.bot_id() != self.delivery.bot_id())
            || request.network() != &NetworkSelection::NONE
        {
            return Err(DownloadError::InvalidInput);
        }
        let intent = request.clone().with_preference_overrides(overrides.clone());
        let identity = self.authorize_request(request).await?;
        if let Some(admission) = self
            .store
            .replay_network_admission(&intent, idempotency_key)
            .await
            .map_err(map_store)?
        {
            return Ok(admission);
        }
        let base = request.params();
        let params = if request.media_mode() == DomainMediaMode::Formats {
            base.clone().with_send_as_file(true)
        } else {
            let preferences = self
                .profile
                .format_preferences(request.actor_id())
                .await
                .map_err(map_profile)?;
            let resolved = profile::resolve_format_values(
                request.media_mode(),
                preferences.as_ref(),
                &overrides,
            )
            .map_err(map_profile)?;
            base.clone()
                .with_max_height(resolved.max_height())
                .map_err(|_| DownloadError::InvalidPreferences)?
                .with_video_codec(resolved.video_codec())
                .with_audio_codec(resolved.audio_codec())
                .with_container(resolved.container())
                .with_send_as_file(resolved.send_as_file())
                .with_gallery_zip(resolved.gallery_zip())
                .with_subtitles(resolved.subtitles().cloned())
        };
        let prepared_intent = intent
            .with_params(params)
            .map_err(|_| DownloadError::InvalidInput)?;
        self.admit_network_request(&prepared_intent, idempotency_key, &identity)
            .await
    }

    pub async fn submit_request(
        &self,
        request: &DownloadRequest,
        idempotency_key: &str,
    ) -> Result<Admission, DownloadError> {
        if request.preference_overrides().is_some()
            || request.source_url().as_str().len() > 4096
            || request
                .target()
                .is_none_or(|target| target.bot_id() != self.delivery.bot_id())
        {
            return Err(DownloadError::InvalidInput);
        }
        if request.network() != &NetworkSelection::NONE {
            return Err(DownloadError::InvalidInput);
        }
        let identity = self.authorize_request(request).await?;
        if let Some(admission) = self
            .store
            .replay_network_admission(request, idempotency_key)
            .await
            .map_err(map_store)?
        {
            return Ok(admission);
        }
        self.admit_network_request(request, idempotency_key, &identity)
            .await
    }

    async fn admit_network_request(
        &self,
        request: &DownloadRequest,
        idempotency_key: &str,
        identity: &yoink_identity::IdentitySnapshot,
    ) -> Result<Admission, DownloadError> {
        let domain = request
            .source_url()
            .host_str()
            .ok_or(DownloadError::InvalidInput)?;
        let seed = selection_seed(request.actor_id(), self.delivery.bot_id(), idempotency_key);
        let selection = self
            .profile
            .prepare_network(
                self.network_context(request.actor_id(), domain, identity),
                seed,
            )
            .await
            .map_err(map_profile)?;
        let prepared = request
            .clone()
            .with_network(selection)
            .map_err(|_| DownloadError::InvalidInput)?;
        self.store
            .admit_network_snapshot(&prepared, idempotency_key, self.config.admission)
            .await
            .map_err(map_store)
    }

    pub async fn status(
        &self,
        actor_id: u64,
        id: Uuid,
    ) -> Result<Option<OperationRecord>, DownloadError> {
        let record = self.store.get(actor_id, id).await.map_err(map_store)?;
        if let Some(record) = &record {
            self.authorize_target(actor_id, record.target).await?;
        }
        Ok(record)
    }

    pub async fn cancel(&self, actor_id: u64, id: Uuid) -> Result<bool, DownloadError> {
        let Some(record) = self.store.get(actor_id, id).await.map_err(map_store)? else {
            return Ok(false);
        };
        self.authorize_target(actor_id, record.target).await?;
        self.store
            .request_cancel(actor_id, id)
            .await
            .map_err(map_store)
    }

    async fn authorize_request(
        &self,
        request: &DownloadRequest,
    ) -> Result<yoink_identity::IdentitySnapshot, DownloadError> {
        if request.media_mode() == DomainMediaMode::Formats
            && (!self
                .identities
                .is_configured_owner(request.actor_id() as i64)
                || request.target().is_none_or(|target| {
                    target.bot_id() != self.delivery.bot_id()
                        || target.chat_id() != request.actor_id() as i64
                        || target.thread_id().is_some()
                }))
        {
            return Err(DownloadError::Denied);
        }
        self.authorize_target(request.actor_id(), request.target().copied())
            .await
    }

    fn network_context<'a>(
        &'a self,
        actor_id: u64,
        domain: &'a str,
        identity: &'a yoink_identity::IdentitySnapshot,
    ) -> NetworkContext<'a> {
        NetworkContext {
            actor_id,
            domain,
            no_cookie_domains: &self.network_config.no_cookie_domains,
            identity,
            system_proxy: self.network_config.system_proxy.as_deref(),
        }
    }

    pub async fn run(&self, shutdown: CancellationToken) -> Result<(), DownloadError> {
        let worker_prefix = Uuid::new_v4().simple().to_string();
        let mut workers = JoinSet::new();
        for index in 0..self.config.concurrency {
            let service = self.clone();
            let stop = shutdown.child_token();
            let worker_id = format!("{worker_prefix}-{index}");
            workers.spawn(async move { service.worker_loop(worker_id, stop).await });
        }
        supervise_workers(&mut workers, &shutdown).await
    }

    async fn worker_loop(
        &self,
        worker_id: String,
        shutdown: CancellationToken,
    ) -> Result<(), DownloadError> {
        loop {
            if shutdown.is_cancelled() {
                return Ok(());
            }
            let claim = tokio::select! {
                _ = shutdown.cancelled() => return Ok(()),
                result = self.store.claim_for_bot(&worker_id, self.config.lease, self.delivery.bot_id()) => result.map_err(map_store)?,
            };
            match claim {
                Some(claim) => {
                    self.process_claim(claim, shutdown.child_token()).await?;
                }
                None => {
                    tokio::select! {
                        _ = shutdown.cancelled() => return Ok(()),
                        _ = time::sleep(self.config.poll_interval) => {}
                    }
                }
            }
        }
    }

    async fn process_claim(
        &self,
        claim: Claim,
        shutdown: CancellationToken,
    ) -> Result<(), DownloadError> {
        if claim.cancel_requested {
            return match self.store.acknowledge_cancel(&claim).await {
                Ok(()) => Ok(()),
                Err(error) if is_stale(&error) => Ok(()),
                Err(error) => Err(map_store(error)),
            };
        }
        let (heartbeat_tx, mut heartbeat_rx) = mpsc::channel(2);
        let heartbeat = self.start_heartbeat(claim.clone(), heartbeat_tx);
        let host = claim
            .request
            .source_url()
            .host_str()
            .unwrap_or_default()
            .to_owned();
        let identity = match self.authorize_request(&claim.request).await {
            Ok(identity) => identity,
            Err(DownloadError::Denied) => {
                self.store.fail(&claim, false).await.map_err(map_store)?;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let credentials = match self
            .profile
            .resolve(
                self.network_context(claim.request.actor_id(), &host, &identity),
                claim.request.network(),
            )
            .await
        {
            Ok(credentials) => credentials,
            Err(ProfileError::Database) => return Err(DownloadError::Database),
            Err(_) => {
                self.store.fail(&claim, false).await.map_err(map_store)?;
                return Ok(());
            }
        };
        let send_as_file = claim.request.params().send_as_file();
        let is_formats = claim.request.media_mode() == DomainMediaMode::Formats;
        let mut cached_revision = None;
        let cache_hit = if !is_formats
            && cache::cache_read_enabled(self.config.cache_ttl, claim.cache_bypassed)
        {
            match self.store.lookup_media_cache(&claim.request).await {
                Ok(Some(batch))
                    if cache::valid_cached_media(
                        &batch.media,
                        self.executor.artifact_limits(),
                        send_as_file,
                    ) =>
                {
                    cached_revision = Some(batch.revision);
                    Some(batch.media)
                }
                Ok(_) => None,
                Err(_) => {
                    tracing::warn!(
                        operation_id = %claim.operation_id,
                        "media cache lookup failed; falling back to download"
                    );
                    None
                }
            }
        } else {
            None
        };
        let artifacts: Vec<DeliveryArtifact> = if let Some(media) = cache_hit {
            media.into_iter().map(DeliveryArtifact::Cached).collect()
        } else {
            let worker_request = match worker_request(&claim, credentials) {
                Ok(request) => request,
                Err(()) => {
                    self.store.fail(&claim, false).await.map_err(map_store)?;
                    return Ok(());
                }
            };
            let child_cancel = shutdown.child_token();
            let (progress_tx, mut progress_rx) = tokio::sync::watch::channel(None);
            let execution = self.executor.execute_with_progress(
                &worker_request,
                child_cancel.clone(),
                Some(progress_tx),
            );
            tokio::pin!(execution);
            let mut progress_open = true;
            let artifacts = loop {
                tokio::select! {
                    result = &mut execution => match result {
                        Ok(artifacts) => break artifacts,
                        Err(error) => {
                            child_cancel.cancel();
                            drop(heartbeat);
                            return self.finish_executor_error(&claim, error, shutdown.is_cancelled()).await;
                        }
                    },
                    event = heartbeat_rx.recv() => {
                        child_cancel.cancel();
                        let _ = execution.await;
                        drop(heartbeat);
                        return self.finish_heartbeat_event(&claim, event).await;
                    },
                    changed = progress_rx.changed(), if progress_open => {
                        if changed.is_ok() {
                            let progress = *progress_rx.borrow_and_update();
                            if let Some(progress) = progress {
                                let stage = match progress.stage {
                                    yoink_worker_protocol::ProgressStage::Fetching => yoink_storage::ProgressStage::Fetching,
                                    yoink_worker_protocol::ProgressStage::Processing => yoink_storage::ProgressStage::Processing,
                                    yoink_worker_protocol::ProgressStage::WritingArtifact => yoink_storage::ProgressStage::WritingArtifact,
                                };
                                let percent = progress.percent.round().clamp(0.0, 100.0) as u8;
                                if let Err(error) = self.store.update_progress(&claim, percent, stage).await {
                                    child_cancel.cancel();
                                    let _ = execution.await;
                                    drop(heartbeat);
                                    return if is_stale(&error) { Ok(()) } else { Err(map_store(error)) };
                                }
                            }
                            continue;
                        }
                        progress_open = false;
                    },
                    _ = shutdown.cancelled() => {
                        child_cancel.cancel();
                        let _ = execution.await;
                        drop(heartbeat);
                        return match self.store.fail(&claim, true).await {
                            Ok(()) => Ok(()),
                            Err(error) if is_stale(&error) => Ok(()),
                            Err(error) => Err(map_store(error)),
                        };
                    }
                }
            };
            artifacts
                .into_iter()
                .map(DeliveryArtifact::Upload)
                .collect()
        };
        if artifacts.is_empty() || artifacts.len() > 100 {
            drop(heartbeat);
            self.store.fail(&claim, false).await.map_err(map_store)?;
            return Ok(());
        }
        if shutdown.is_cancelled() {
            drop(heartbeat);
            return match self.store.fail(&claim, true).await {
                Ok(()) => Ok(()),
                Err(error) if is_stale(&error) => Ok(()),
                Err(error) => Err(map_store(error)),
            };
        }
        if let Err(error) = self.authorize_request(&claim.request).await {
            drop(heartbeat);
            if error == DownloadError::Denied {
                self.store.fail(&claim, false).await.map_err(map_store)?;
                return Ok(());
            }
            return Err(error);
        }
        let source_metadata = artifacts
            .iter()
            .map(|artifact| cache_metadata(artifact, send_as_file))
            .collect::<Vec<_>>();
        let units = plan_delivery_units(&artifacts, send_as_file);
        let mut artifacts: Vec<Option<DeliveryArtifact>> =
            artifacts.into_iter().map(Some).collect();
        match heartbeat_rx.try_recv() {
            Ok(event) => {
                drop(heartbeat);
                return self.finish_heartbeat_event(&claim, Some(event)).await;
            }
            Err(mpsc::error::TryRecvError::Disconnected) => {
                drop(heartbeat);
                return self.finish_heartbeat_event(&claim, None).await;
            }
            Err(mpsc::error::TryRecvError::Empty) => {}
        }
        if shutdown.is_cancelled() {
            drop(heartbeat);
            return match self.store.fail(&claim, true).await {
                Ok(()) => Ok(()),
                Err(error) if is_stale(&error) => Ok(()),
                Err(error) => Err(map_store(error)),
            };
        }
        if !self
            .store
            .begin_delivery_batch(&claim, artifacts.len() as u32)
            .await
            .map_err(map_store)?
        {
            drop(heartbeat);
            return Ok(());
        }
        let target = claim
            .request
            .target()
            .copied()
            .or_else(|| {
                DeliveryTarget::new(
                    self.delivery.bot_id(),
                    claim.request.actor_id() as i64,
                    None,
                    None,
                )
                .ok()
            })
            .ok_or(DownloadError::InvalidInput)?;
        let mut cancellation_seen = false;
        let mut confirmed_file_ids = Vec::new();
        for (ordinal, unit) in units.into_iter().enumerate() {
            let ordinal = ordinal as u32;
            if shutdown.is_cancelled() || cancellation_seen {
                self.stop_delivery(
                    &claim,
                    if cancellation_seen {
                        yoink_storage::StopDeliveryReason::Cancel
                    } else {
                        yoink_storage::StopDeliveryReason::Shutdown
                    },
                )
                .await?;
                break;
            }
            let authorization = if is_formats {
                self.authorize_request(&claim.request).await
            } else {
                self.authorize_target(claim.request.actor_id(), Some(target))
                    .await
            };
            let identity = match authorization {
                Ok(identity) => identity,
                Err(DownloadError::Denied) => {
                    self.stop_delivery(&claim, yoink_storage::StopDeliveryReason::Denied)
                        .await?;
                    break;
                }
                Err(error) => {
                    drop(heartbeat);
                    return Err(error);
                }
            };
            let domain = claim.request.source_url().host_str().unwrap_or_default();
            match self
                .profile
                .resolve(
                    self.network_context(claim.request.actor_id(), domain, &identity),
                    claim.request.network(),
                )
                .await
            {
                Ok(_) => {}
                Err(ProfileError::Database) => {
                    drop(heartbeat);
                    return Err(DownloadError::Database);
                }
                Err(_) => {
                    self.stop_delivery(&claim, yoink_storage::StopDeliveryReason::Denied)
                        .await?;
                    break;
                }
            }
            if !self
                .store
                .begin_delivery_unit(&claim, ordinal, unit.len() as u32)
                .await
                .map_err(map_store)?
            {
                break;
            }
            let media = unit
                .iter()
                .filter_map(|index| artifacts[*index].take())
                .collect();
            let delivery =
                self.delivery
                    .deliver(target, claim.request.params().send_as_file(), media);
            tokio::pin!(delivery);
            let deadline = time::sleep(self.config.delivery_timeout);
            tokio::pin!(deadline);
            let mut heartbeat_closed = false;
            let mut heartbeat_failed = false;
            let outcome = loop {
                tokio::select! {
                    result = &mut delivery => {
                        break match result {
                            Ok(receipts) if receipts.len() == unit.len() && receipts.iter().all(|receipt| receipt.message_id > 0) && {
                                let mut ids = std::collections::HashSet::with_capacity(receipts.len());
                                receipts.iter().all(|receipt| ids.insert(receipt.message_id))
                            } => DeliveryOutcome::Delivered(receipts),
                            Err(DeliveryError::Rejected) => DeliveryOutcome::Rejected,
                            Err(DeliveryError::InvalidFileId) => DeliveryOutcome::InvalidFileId,
                            Err(DeliveryError::RetryAfter(delay)) => DeliveryOutcome::RetryAfter(delay),
                            _ => DeliveryOutcome::Unknown,
                        };
                    }
                    _ = &mut deadline => break DeliveryOutcome::Unknown,
                    event = heartbeat_rx.recv(), if !heartbeat_closed => match event {
                        Some(HeartbeatEvent::Database | HeartbeatEvent::Stale) => { heartbeat_failed = true; },
                        Some(HeartbeatEvent::Cancelled) => { cancellation_seen = true; },
                        None => heartbeat_closed = true,
                    },
                }
            };
            let result = match (outcome, cached_revision) {
                (DeliveryOutcome::InvalidFileId, Some(revision))
                    if !heartbeat_failed && ordinal == 0 =>
                {
                    match self
                        .store
                        .requeue_invalid_cache(&claim, ordinal, revision)
                        .await
                    {
                        Ok(true) => {
                            drop(heartbeat);
                            return Ok(());
                        }
                        Ok(false) => {
                            if self
                                .store
                                .invalidate_media_cache(&claim.request, revision)
                                .await
                                .is_err()
                            {
                                tracing::warn!(
                                    operation_id = %claim.operation_id,
                                    "media cache invalidation failed"
                                );
                            }
                            self.store
                                .reject_delivery_unit(&claim, ordinal, None)
                                .await
                                .map(|accepted| (accepted, true, None))
                        }
                        Err(error) => Err(error),
                    }
                }
                (DeliveryOutcome::Delivered(receipts), _) if !heartbeat_failed => self
                    .store
                    .complete_delivery_unit(
                        &claim,
                        ordinal,
                        &receipts
                            .iter()
                            .map(|receipt| receipt.message_id)
                            .collect::<Vec<_>>(),
                    )
                    .await
                    .map(|accepted| (accepted, false, Some(receipts))),
                (DeliveryOutcome::Delivered(_), _) => self
                    .store
                    .unknown_delivery_unit(&claim, ordinal)
                    .await
                    .map(|accepted| (accepted, true, None)),
                (DeliveryOutcome::InvalidFileId, Some(revision)) if !heartbeat_failed => {
                    if ordinal > 0
                        && self
                            .store
                            .invalidate_media_cache(&claim.request, revision)
                            .await
                            .is_err()
                    {
                        tracing::warn!(
                            operation_id = %claim.operation_id,
                            "media cache invalidation failed"
                        );
                    }
                    self.store
                        .reject_delivery_unit(&claim, ordinal, None)
                        .await
                        .map(|accepted| (accepted, true, None))
                }
                (DeliveryOutcome::Rejected | DeliveryOutcome::InvalidFileId, _)
                    if !heartbeat_failed =>
                {
                    self.store
                        .reject_delivery_unit(&claim, ordinal, None)
                        .await
                        .map(|accepted| (accepted, true, None))
                }
                (DeliveryOutcome::RetryAfter(delay), _) if !heartbeat_failed => self
                    .store
                    .reject_delivery_unit(&claim, ordinal, Some(delay))
                    .await
                    .map(|accepted| (accepted, true, None)),
                _ => self
                    .store
                    .unknown_delivery_unit(&claim, ordinal)
                    .await
                    .map(|accepted| (accepted, true, None)),
            };
            match result {
                Err(error) if is_stale(&error) => break,
                Err(error) => {
                    drop(heartbeat);
                    return Err(map_store(error));
                }
                Ok((false, _, _)) => break,
                Ok((true, true, _)) => break,
                Ok((true, false, Some(receipts))) => {
                    confirmed_file_ids.extend(receipts.into_iter().map(|receipt| receipt.file_id));
                }
                Ok((true, false, None)) => break,
            }
            if shutdown.is_cancelled() || cancellation_seen {
                self.stop_delivery(
                    &claim,
                    if cancellation_seen {
                        yoink_storage::StopDeliveryReason::Cancel
                    } else {
                        yoink_storage::StopDeliveryReason::Shutdown
                    },
                )
                .await?;
                break;
            }
        }
        match heartbeat_rx.try_recv() {
            Ok(HeartbeatEvent::Cancelled) => cancellation_seen = true,
            Ok(HeartbeatEvent::Stale | HeartbeatEvent::Database)
            | Err(mpsc::error::TryRecvError::Disconnected) => confirmed_file_ids.clear(),
            Err(mpsc::error::TryRecvError::Empty) => {}
        }
        if !is_formats
            && cached_revision.is_none()
            && !self.config.cache_ttl.is_zero()
            && !shutdown.is_cancelled()
            && !cancellation_seen
            && confirmed_file_ids.len() == source_metadata.len()
            && confirmed_file_ids.len() == artifacts.len()
            && let Some(media) = cache::build_cached_media(&source_metadata, confirmed_file_ids)
            && self
                .store
                .publish_media_cache(&claim, &media, self.config.cache_ttl)
                .await
                .is_err()
        {
            tracing::warn!(
                operation_id = %claim.operation_id,
                "media cache publication failed"
            );
        }
        drop(heartbeat);
        Ok(())
    }

    async fn stop_delivery(
        &self,
        claim: &Claim,
        reason: yoink_storage::StopDeliveryReason,
    ) -> Result<(), DownloadError> {
        match self.store.stop_delivery_batch(claim, reason).await {
            Ok(_) => Ok(()),
            Err(error) if is_stale(&error) => Ok(()),
            Err(error) => Err(map_store(error)),
        }
    }

    fn start_heartbeat(
        &self,
        claim: Claim,
        sender: mpsc::Sender<HeartbeatEvent>,
    ) -> AbortOnDropHandle<()> {
        let store = self.store.clone();
        let interval = self.config.heartbeat_interval;
        let lease = self.config.lease;
        AbortOnDropHandle::new(tokio::spawn(async move {
            let mut cancellation_reported = false;
            loop {
                time::sleep(interval).await;
                match store.heartbeat_state(&claim, lease).await {
                    Ok(state) if state.cancel_requested && !cancellation_reported => {
                        let _ = sender.try_send(HeartbeatEvent::Cancelled);
                        cancellation_reported = true;
                    }
                    Ok(_) => {}
                    Err(error) if is_stale(&error) => {
                        let _ = sender.try_send(HeartbeatEvent::Stale);
                        return;
                    }
                    Err(_) => {
                        let _ = sender.try_send(HeartbeatEvent::Database);
                        return;
                    }
                }
            }
        }))
    }

    async fn finish_heartbeat_event(
        &self,
        claim: &Claim,
        event: Option<HeartbeatEvent>,
    ) -> Result<(), DownloadError> {
        match event {
            Some(HeartbeatEvent::Cancelled) => match self.store.acknowledge_cancel(claim).await {
                Ok(()) => Ok(()),
                Err(error) if is_stale(&error) => Ok(()),
                Err(error) => Err(map_store(error)),
            },
            Some(HeartbeatEvent::Stale) | None => Ok(()),
            Some(HeartbeatEvent::Database) => Err(DownloadError::Database),
        }
    }

    async fn finish_executor_error(
        &self,
        claim: &Claim,
        error: ExecutorError,
        global_shutdown: bool,
    ) -> Result<(), DownloadError> {
        if error == ExecutorError::Cancelled {
            if global_shutdown {
                return match self.store.fail(claim, true).await {
                    Ok(()) => Ok(()),
                    Err(error) if is_stale(&error) => Ok(()),
                    Err(error) => Err(map_store(error)),
                };
            }
            return match self.store.acknowledge_cancel(claim).await {
                Ok(()) => Ok(()),
                Err(error) if is_stale(&error) => match self.store.fail(claim, true).await {
                    Ok(()) => Ok(()),
                    Err(error) if is_stale(&error) => Ok(()),
                    Err(error) => Err(map_store(error)),
                },
                Err(error) => Err(map_store(error)),
            };
        }
        let retryable = retryable_executor_error(error);
        tracing::warn!(
            operation_id = %claim.operation_id,
            attempt = claim.attempt,
            failure = ?error,
            retryable,
            "media attempt failed"
        );
        match self.store.fail(claim, retryable).await {
            Ok(()) => Ok(()),
            Err(error) if is_stale(&error) => Ok(()),
            Err(error) => Err(map_store(error)),
        }
    }
}

#[derive(Clone, Copy)]
enum HeartbeatEvent {
    Cancelled,
    Stale,
    Database,
}
enum DeliveryOutcome {
    Delivered(Vec<DeliveredMedia>),
    InvalidFileId,
    Rejected,
    RetryAfter(Duration),
    Unknown,
}

fn cache_metadata(
    artifact: &DeliveryArtifact,
    send_as_file: bool,
) -> Option<(yoink_domain::CachedMediaKind, u64)> {
    match artifact {
        DeliveryArtifact::Upload(upload) => Some(cache::upload_metadata(
            upload.media_type.as_str(),
            upload.size_bytes,
            send_as_file,
        )),
        DeliveryArtifact::Cached(_) => None,
    }
}

fn plan_delivery_units(artifacts: &[DeliveryArtifact], send_as_file: bool) -> Vec<Vec<usize>> {
    let categories = artifacts
        .iter()
        .map(|artifact| match artifact {
            DeliveryArtifact::Upload(upload) => {
                cache::category(cache::upload_kind(upload.media_type.as_str(), send_as_file))
            }
            DeliveryArtifact::Cached(media) => cache::category(media.kind()),
        })
        .collect::<Vec<_>>();
    plan_categories(&categories)
}

fn plan_categories(categories: &[cache::MediaCategory]) -> Vec<Vec<usize>> {
    let mut result = Vec::new();
    let mut start = 0;
    while start < categories.len() {
        let kind = categories[start];
        let mut end = start + 1;
        while end < categories.len() && end - start < 10 && categories[end] == kind {
            end += 1;
        }
        result.push((start..end).collect());
        start = end;
    }
    result
}

async fn supervise_workers(
    workers: &mut JoinSet<Result<(), DownloadError>>,
    shutdown: &CancellationToken,
) -> Result<(), DownloadError> {
    let mut failure = None;
    while let Some(result) = workers.join_next().await {
        let error = match result {
            Ok(Ok(())) => continue,
            Ok(Err(error)) => error,
            Err(_) => DownloadError::Task,
        };
        failure.get_or_insert(error);
        // Siblings stop executors or finish already-started uploads. The server
        // shutdown deadline aborts this JoinSet only as a last resort.
        shutdown.cancel();
    }
    failure.map_or(Ok(()), Err)
}

fn worker_request(claim: &Claim, credentials: NetworkCredentials) -> Result<WorkerRequest, ()> {
    let request = &claim.request;
    let params = request.params();
    let options = yoink_worker_protocol::DownloadOptions {
        max_height: params.max_height(),
        video_codec: match params.video_codec() {
            yoink_domain::VideoCodec::H264 => yoink_worker_protocol::VideoCodec::H264,
            yoink_domain::VideoCodec::H265 => yoink_worker_protocol::VideoCodec::H265,
            yoink_domain::VideoCodec::Av1 => yoink_worker_protocol::VideoCodec::Av1,
            yoink_domain::VideoCodec::Vp9 => yoink_worker_protocol::VideoCodec::Vp9,
            yoink_domain::VideoCodec::Any => yoink_worker_protocol::VideoCodec::Any,
        },
        audio_codec: match params.audio_codec() {
            yoink_domain::AudioCodec::Best => yoink_worker_protocol::AudioCodec::Best,
            yoink_domain::AudioCodec::Aac => yoink_worker_protocol::AudioCodec::Aac,
            yoink_domain::AudioCodec::Mp3 => yoink_worker_protocol::AudioCodec::Mp3,
            yoink_domain::AudioCodec::Opus => yoink_worker_protocol::AudioCodec::Opus,
        },
        container: match params.container() {
            yoink_domain::Container::Mp4 => yoink_worker_protocol::Container::Mp4,
            yoink_domain::Container::Mkv => yoink_worker_protocol::Container::Mkv,
            yoink_domain::Container::Webm => yoink_worker_protocol::Container::Webm,
        },
        engine: match params.engine() {
            yoink_domain::Engine::Auto => yoink_worker_protocol::Engine::Auto,
            yoink_domain::Engine::YtDlp => yoink_worker_protocol::Engine::YtDlp,
            yoink_domain::Engine::Gallery => yoink_worker_protocol::Engine::Gallery,
        },
        playlist: params
            .playlist()
            .map(|range| yoink_worker_protocol::PlaylistRange {
                start: range.start(),
                end: range.end(),
            }),
        clips: params
            .clips()
            .iter()
            .map(|range| yoink_worker_protocol::ClipRange {
                start_seconds: range.start_seconds(),
                end_seconds: range.end_seconds(),
            })
            .chain(
                request
                    .clip_range()
                    .map(|range| yoink_worker_protocol::ClipRange {
                        start_seconds: range.start_seconds(),
                        end_seconds: range.end_seconds(),
                    }),
            )
            .collect(),
        gallery_zip: params.gallery_zip(),
        subtitles: params
            .subtitles()
            .map(|s| yoink_worker_protocol::SubtitleOptions {
                language: s.language().into(),
                automatic: s.automatic(),
            }),
    };
    let media_mode = match request.media_mode() {
        DomainMediaMode::Video => MediaMode::Video,
        DomainMediaMode::Audio => MediaMode::Audio,
        DomainMediaMode::Image => MediaMode::Image,
        DomainMediaMode::Subtitles => MediaMode::Subtitles,
        DomainMediaMode::Formats => MediaMode::Formats,
    };
    Ok(WorkerRequest {
        version: PROTOCOL_VERSION,
        operation_id: claim.operation_id,
        attempt: claim.attempt,
        source_url: HttpSourceUrl::parse(request.source_url().as_str()).map_err(|_| ())?,
        task: TaskKind::Download { media_mode },
        options,
        network: if credentials.proxy_url.is_some() || credentials.cookie_jar.is_some() {
            Some(credentials)
        } else {
            None
        },
    })
}

fn retryable_executor_error(error: ExecutorError) -> bool {
    match error {
        ExecutorError::Timeout
        | ExecutorError::Spawn
        | ExecutorError::Exited(_)
        | ExecutorError::Io => true,
        ExecutorError::Worker(class) => matches!(
            class,
            yoink_worker_protocol::FailureClass::SourceUnavailable
                | yoink_worker_protocol::FailureClass::RateLimited
                | yoink_worker_protocol::FailureClass::ProcessingFailed
        ),
        ExecutorError::InvalidConfig
        | ExecutorError::Protocol
        | ExecutorError::Cancelled
        | ExecutorError::ResourceLimit
        | ExecutorError::UnsafeArtifact => false,
    }
}

fn request_format_defaults(request: &DownloadRequest) -> bool {
    let p = request.params();
    if request.media_mode() == DomainMediaMode::Formats {
        return p.max_height().is_none()
            && p.video_codec() == yoink_domain::VideoCodec::H264
            && p.audio_codec() == yoink_domain::AudioCodec::Best
            && p.container() == yoink_domain::Container::Mp4
            && p.send_as_file()
            && !p.gallery_zip()
            && p.subtitles().is_none()
            && p.playlist().is_none()
            && p.clips().is_empty()
            && matches!(
                p.engine(),
                yoink_domain::Engine::Auto | yoink_domain::Engine::YtDlp
            );
    }
    p.max_height().is_none()
        && p.video_codec() == yoink_domain::VideoCodec::H264
        && p.audio_codec() == yoink_domain::AudioCodec::Best
        && p.container() == yoink_domain::Container::Mp4
        && !p.send_as_file()
        && !p.gallery_zip()
        && p.subtitles().is_none_or(|s| {
            request.media_mode() == DomainMediaMode::Subtitles
                && s == &yoink_domain::SubtitleOptions::default()
        })
}

fn validate_config(config: &DownloadConfig) -> Result<(), DownloadError> {
    if !(1..=8).contains(&config.concurrency)
        || config.lease < Duration::from_secs(1)
        || config.lease > Duration::from_secs(300)
        || config.lease.subsec_nanos() != 0
        || config.heartbeat_interval.is_zero()
        || config.poll_interval < Duration::from_millis(10)
        || config.delivery_timeout.is_zero()
        || config.delivery_timeout > config.lease
        || config.cache_ttl > Duration::from_secs(31_536_000)
        || config.cache_ttl.subsec_nanos() != 0
        || config
            .heartbeat_interval
            .checked_mul(3)
            .is_none_or(|minimum| config.lease < minimum)
        || !(1..=100).contains(&config.admission.max_attempts)
        || config.admission.retry_delay.subsec_nanos() != 0
        || config.admission.retry_delay > Duration::from_secs(300)
        || !(1..=10_000).contains(&config.admission.max_pending_global)
        || !(1..=10_000).contains(&config.admission.max_pending_actor)
    {
        return Err(DownloadError::InvalidConfig);
    }
    Ok(())
}

fn selection_seed(actor_id: u64, bot_id: i64, idempotency_key: impl std::fmt::Display) -> u64 {
    let mut hash = Sha256::new();
    hash.update(b"yoink-download-network-selection-v1\0");
    hash.update(actor_id.to_be_bytes());
    hash.update(bot_id.to_be_bytes());
    hash.update(idempotency_key.to_string().as_bytes());
    u64::from_be_bytes(hash.finalize()[..8].try_into().expect("SHA-256 prefix"))
}

fn map_profile(error: ProfileError) -> DownloadError {
    match error {
        ProfileError::Database => DownloadError::Database,
        ProfileError::ProxyUnavailable => DownloadError::ProxyUnavailable,
        ProfileError::UnsupportedAuthentication => DownloadError::UnsupportedAuthentication,
        ProfileError::InvalidPreferences => DownloadError::InvalidPreferences,
        ProfileError::InvalidProxy | ProfileError::InvalidCookie => {
            DownloadError::InvalidCredentials
        }
        ProfileError::Unavailable
        | ProfileError::StaleReference
        | ProfileError::SharedAccessDenied
        | ProfileError::CandidateCapacityExceeded => DownloadError::ChangedCredentials,
    }
}

fn map_store(error: StoreError) -> DownloadError {
    match error {
        StoreError::InvalidInput | StoreError::InvalidStoredData => DownloadError::InvalidInput,
        StoreError::Unauthorized => DownloadError::Denied,
        StoreError::Conflict => DownloadError::Conflict,
        StoreError::Capacity => DownloadError::Capacity,
        StoreError::NotFoundOrStale => DownloadError::Task,
        StoreError::Database | StoreError::Schema => DownloadError::Database,
    }
}
fn is_stale(error: &StoreError) -> bool {
    *error == StoreError::NotFoundOrStale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> DownloadConfig {
        DownloadConfig::default()
    }

    #[test]
    fn network_config_is_bounded_canonical_and_redacted() {
        let config = DownloadNetworkConfig::new(
            Some("http://user:secret@proxy.example:8080".into()),
            vec!["WWW.Example.com".into(), "example.com".into()],
        )
        .unwrap();
        assert_eq!(config.no_cookie_domains, ["example.com"]);
        assert!(!format!("{config:?}").contains("secret"));
        assert!(DownloadNetworkConfig::new(None, vec!["https://example.com/path".into()]).is_err());
        assert!(DownloadNetworkConfig::new(None, vec!["not a host".into()]).is_err());
        assert!(DownloadNetworkConfig::new(None, vec!["example.com".into(); 65]).is_err());
    }

    #[test]
    fn selection_seed_is_stable_and_scoped_to_actor_bot_and_key() {
        assert_eq!(selection_seed(42, 7, "key"), selection_seed(42, 7, "key"));
        assert_ne!(selection_seed(42, 7, "key"), selection_seed(43, 7, "key"));
        assert_ne!(selection_seed(42, 7, "key"), selection_seed(42, 8, "key"));
        assert_ne!(selection_seed(42, 7, "key"), selection_seed(42, 7, "other"));
    }

    #[test]
    fn profile_errors_are_static_and_execution_stop_mapping_is_safe() {
        assert_eq!(map_profile(ProfileError::Database), DownloadError::Database);
        assert_eq!(
            map_profile(ProfileError::InvalidPreferences),
            DownloadError::InvalidPreferences
        );
        assert_eq!(
            map_profile(ProfileError::StaleReference),
            DownloadError::ChangedCredentials
        );
        assert!(
            !DownloadError::InvalidCredentials
                .to_string()
                .contains("cookie")
        );
    }

    #[test]
    fn accepts_valid_bounded_service_policy() {
        assert_eq!(validate_config(&valid_config()), Ok(()));
        assert_eq!(validate_config(&DownloadConfig::default()), Ok(()));
    }

    #[test]
    fn cache_ttl_accepts_zero_and_one_year_but_rejects_out_of_range_or_fractional() {
        let mut config = valid_config();
        config.cache_ttl = Duration::ZERO;
        assert_eq!(validate_config(&config), Ok(()));
        config.cache_ttl = Duration::from_secs(31_536_000);
        assert_eq!(validate_config(&config), Ok(()));
        config.cache_ttl = Duration::from_secs(31_536_001);
        assert_eq!(validate_config(&config), Err(DownloadError::InvalidConfig));
        config.cache_ttl = Duration::from_millis(1500);
        assert_eq!(validate_config(&config), Err(DownloadError::InvalidConfig));
    }

    #[test]
    fn rejects_invalid_concurrency_and_inadequate_lease() {
        let mut config = valid_config();
        config.concurrency = 9;
        assert_eq!(validate_config(&config), Err(DownloadError::InvalidConfig));
        config = valid_config();
        config.lease = Duration::from_secs(14);
        assert_eq!(validate_config(&config), Err(DownloadError::InvalidConfig));
    }

    #[test]
    fn rejects_invalid_poll_delivery_and_admission_limits() {
        let mut config = valid_config();
        config.poll_interval = Duration::from_millis(9);
        assert_eq!(validate_config(&config), Err(DownloadError::InvalidConfig));
        config = valid_config();
        config.delivery_timeout = config.lease + Duration::from_millis(1);
        assert_eq!(validate_config(&config), Err(DownloadError::InvalidConfig));
        config = valid_config();
        config.admission.max_attempts = 0;
        assert_eq!(validate_config(&config), Err(DownloadError::InvalidConfig));
    }

    #[test]
    fn retryability_is_limited_to_transient_executor_failures() {
        use yoink_worker_protocol::FailureClass;
        assert!(retryable_executor_error(ExecutorError::Timeout));
        assert!(retryable_executor_error(ExecutorError::Spawn));
        assert!(retryable_executor_error(ExecutorError::Io));
        for class in [
            FailureClass::SourceUnavailable,
            FailureClass::RateLimited,
            FailureClass::ProcessingFailed,
        ] {
            assert!(retryable_executor_error(ExecutorError::Worker(class)));
        }
        for error in [
            ExecutorError::Protocol,
            ExecutorError::ResourceLimit,
            ExecutorError::UnsafeArtifact,
            ExecutorError::Worker(FailureClass::UnsupportedSource),
            ExecutorError::Worker(FailureClass::InvalidMedia),
        ] {
            assert!(!retryable_executor_error(error));
        }
    }

    #[test]
    fn store_errors_map_without_exposing_storage_details() {
        assert_eq!(map_store(StoreError::Capacity), DownloadError::Capacity);
        assert_eq!(map_store(StoreError::Conflict), DownloadError::Conflict);
        assert_eq!(map_store(StoreError::NotFoundOrStale), DownloadError::Task);
        assert_eq!(map_store(StoreError::Database), DownloadError::Database);
        assert_eq!(format!("{:?}", DownloadError::Database), "Database");
    }

    #[tokio::test]
    async fn later_worker_failure_cancels_and_drains_running_sibling() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let mut workers = JoinSet::new();
        let sibling_stopped = Arc::new(AtomicBool::new(false));
        let stopped = sibling_stopped.clone();
        let shutdown = CancellationToken::new();
        let sibling_shutdown = shutdown.clone();
        let drained = Arc::new(AtomicBool::new(false));
        let drained_sibling = drained.clone();
        workers.spawn(async move {
            struct MarkStopped(Arc<AtomicBool>);
            impl Drop for MarkStopped {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let _mark = MarkStopped(stopped);
            sibling_shutdown.cancelled().await;
            tokio::task::yield_now().await;
            drained_sibling.store(true, Ordering::SeqCst);
            Ok(())
        });
        workers.spawn(async { Err(DownloadError::Database) });

        assert_eq!(
            supervise_workers(&mut workers, &shutdown).await,
            Err(DownloadError::Database)
        );
        assert!(sibling_stopped.load(Ordering::SeqCst));
        assert!(drained.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn dropping_parent_taskset_aborts_workers_and_heartbeat() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let worker_stopped = Arc::new(AtomicBool::new(false));
        let heartbeat_stopped = Arc::new(AtomicBool::new(false));
        let mut workers = JoinSet::new();
        let worker_flag = worker_stopped.clone();
        let heartbeat_flag = heartbeat_stopped.clone();
        workers.spawn(async move {
            struct MarkStopped(Arc<AtomicBool>);
            impl Drop for MarkStopped {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let _worker_guard = MarkStopped(worker_flag);
            let heartbeat_guard = MarkStopped(heartbeat_flag);
            let heartbeat = AbortOnDropHandle::new(tokio::spawn(async move {
                let _guard = heartbeat_guard;
                std::future::pending::<()>().await;
            }));
            let _heartbeat = heartbeat;
            std::future::pending::<Result<(), DownloadError>>().await
        });

        tokio::task::yield_now().await;
        drop(workers);
        tokio::task::yield_now().await;
        assert!(worker_stopped.load(Ordering::SeqCst));
        assert!(heartbeat_stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn planner_groups_only_compatible_contiguous_items_within_ten() {
        use cache::MediaCategory::{Audio, Document, Visual};
        assert_eq!(
            plan_categories(&[Visual, Visual, Audio, Audio, Document]),
            vec![vec![0, 1], vec![2, 3], vec![4]]
        );
        assert_eq!(
            plan_categories(&[Visual; 11]),
            vec![vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9], vec![10]]
        );
    }
}
