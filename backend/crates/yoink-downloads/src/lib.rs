use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use thiserror::Error;
use tokio::{sync::mpsc, task::JoinSet, time};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
use uuid::Uuid;
use yoink_access::{AccessPolicy, AccessScope, Role};
use yoink_domain::{DownloadParams, DownloadRequest, MediaMode as DomainMediaMode};
use yoink_executor::{Executor, ExecutorError, ValidatedArtifact};
use yoink_identity::IdentityStore;
use yoink_storage::{Admission, AdmissionPolicy, Claim, OperationRecord, Store, StoreError};
use yoink_worker_protocol::{HttpSourceUrl, MediaMode, PROTOCOL_VERSION, TaskKind, WorkerRequest};

pub type DeliveryFuture<'a> = Pin<Box<dyn Future<Output = Result<i32, DeliveryError>> + Send + 'a>>;

pub trait Delivery: Send + Sync {
    fn deliver(&self, actor_id: u64, artifact: ValidatedArtifact) -> DeliveryFuture<'_>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryError {
    Rejected,
    Unknown,
}

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Rejected => "delivery was rejected",
            Self::Unknown => "delivery outcome is unknown",
        })
    }
}
impl std::error::Error for DeliveryError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadConfig {
    pub admission: AdmissionPolicy,
    pub concurrency: usize,
    pub lease: Duration,
    pub heartbeat_interval: Duration,
    pub poll_interval: Duration,
    pub delivery_timeout: Duration,
}

#[derive(Clone)]
pub struct DownloadService {
    store: Arc<Store>,
    identities: Arc<IdentityStore>,
    executor: Executor,
    delivery: Arc<dyn Delivery>,
    config: DownloadConfig,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum DownloadError {
    #[error("download input is invalid")]
    InvalidInput,
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
        validate_config(&config)?;
        Ok(Self {
            store,
            identities,
            executor,
            delivery,
            config,
        })
    }

    pub async fn submit(
        &self,
        actor_id: u64,
        source_url: &str,
        idempotency_key: &str,
    ) -> Result<Admission, DownloadError> {
        self.authorize(actor_id).await?;
        if source_url.len() > 4096 {
            return Err(DownloadError::InvalidInput);
        }
        let request = DownloadRequest::new(
            actor_id,
            source_url,
            DomainMediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .map_err(|_| DownloadError::InvalidInput)?;
        self.store
            .admit_bounded(&request, idempotency_key, self.config.admission)
            .await
            .map_err(map_store)
    }

    pub async fn status(
        &self,
        actor_id: u64,
        id: Uuid,
    ) -> Result<Option<OperationRecord>, DownloadError> {
        self.authorize(actor_id).await?;
        self.store.get(actor_id, id).await.map_err(map_store)
    }

    pub async fn cancel(&self, actor_id: u64, id: Uuid) -> Result<bool, DownloadError> {
        self.authorize(actor_id).await?;
        self.store
            .request_cancel(actor_id, id)
            .await
            .map_err(map_store)
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

    async fn authorize(&self, actor_id: u64) -> Result<(), DownloadError> {
        if actor_id == 0 || actor_id > i64::MAX as u64 {
            return Err(DownloadError::Denied);
        }
        let snapshot = self
            .identities
            .snapshot(actor_id as i64, None)
            .await
            .map_err(|_| DownloadError::Database)?
            .ok_or(DownloadError::Denied)?;
        let policy = AccessPolicy {
            min_role: Role::User,
            scopes: vec![AccessScope::Private],
            check_group_enabled: false,
            check_thread_policy: false,
        };
        if snapshot.authorize(&policy, None).decision.allowed {
            Ok(())
        } else {
            Err(DownloadError::Denied)
        }
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
                result = self.store.claim(&worker_id, self.config.lease) => result.map_err(map_store)?,
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
        if let Err(error) = self.authorize(claim.request.actor_id()).await {
            if error == DownloadError::Denied {
                self.store.fail(&claim, false).await.map_err(map_store)?;
                return Ok(());
            }
            return Err(error);
        }
        let worker_request = match worker_request(&claim) {
            Ok(request) => request,
            Err(()) => {
                self.store.fail(&claim, false).await.map_err(map_store)?;
                return Ok(());
            }
        };

        let (heartbeat_tx, mut heartbeat_rx) = mpsc::unbounded_channel();
        let heartbeat = self.start_heartbeat(claim.clone(), heartbeat_tx);
        let child_cancel = shutdown.child_token();
        let execution = self.executor.execute(&worker_request, child_cancel.clone());
        tokio::pin!(execution);
        let artifact = tokio::select! {
            result = &mut execution => match result {
                Ok(artifact) => artifact,
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
            _ = shutdown.cancelled() => {
                child_cancel.cancel();
                let _ = execution.await;
                drop(heartbeat);
                match self.store.fail(&claim, true).await {
                    Ok(()) => {}
                    Err(error) if is_stale(&error) => return Ok(()),
                    Err(error) => return Err(map_store(error)),
                }
                return Ok(());
            }
        };

        if shutdown.is_cancelled() {
            drop(heartbeat);
            match self.store.fail(&claim, true).await {
                Ok(()) => {}
                Err(error) if is_stale(&error) => return Ok(()),
                Err(error) => return Err(map_store(error)),
            }
            return Ok(());
        }
        if let Err(error) = self.authorize(claim.request.actor_id()).await {
            if error == DownloadError::Denied {
                drop(heartbeat);
                self.store.fail(&claim, false).await.map_err(map_store)?;
                return Ok(());
            }
            drop(heartbeat);
            return Err(error);
        }
        if let Err(error) = self.store.begin_delivery(&claim).await {
            drop(heartbeat);
            if is_stale(&error) {
                return match self.store.acknowledge_cancel(&claim).await {
                    Ok(()) => Ok(()),
                    Err(error) if is_stale(&error) => Ok(()),
                    Err(error) => Err(map_store(error)),
                };
            }
            return Err(map_store(error));
        }

        let delivery = self.delivery.deliver(claim.request.actor_id(), artifact);
        tokio::pin!(delivery);
        let deadline = time::sleep(self.config.delivery_timeout);
        tokio::pin!(deadline);
        let mut heartbeat_failed = false;
        let mut heartbeat_channel_closed = false;
        let delivery_result = loop {
            tokio::select! {
                result = &mut delivery => {
                    break match result {
                        Ok(message_id) if message_id > 0 => DeliveryOutcome::Delivered(message_id),
                        Err(DeliveryError::Rejected) => DeliveryOutcome::Rejected,
                        _ => DeliveryOutcome::Unknown,
                    };
                }
                _ = &mut deadline => break DeliveryOutcome::Unknown,
                event = heartbeat_rx.recv(), if !heartbeat_channel_closed => {
                    match event {
                        Some(HeartbeatEvent::Database) => heartbeat_failed = true,
                        Some(HeartbeatEvent::Cancelled | HeartbeatEvent::Stale) => {}
                        None => heartbeat_channel_closed = true,
                    }
                }
            }
        };
        let delivery_result = if heartbeat_failed {
            DeliveryOutcome::Unknown
        } else {
            delivery_result
        };
        match delivery_result {
            DeliveryOutcome::Delivered(message_id) => {
                if let Err(error) = self.store.complete_delivery(&claim, message_id).await {
                    drop(heartbeat);
                    return if is_stale(&error) {
                        Ok(())
                    } else {
                        Err(map_store(error))
                    };
                }
            }
            DeliveryOutcome::Rejected => {
                if let Err(error) = self.store.reject_delivery(&claim).await {
                    drop(heartbeat);
                    return if is_stale(&error) {
                        Ok(())
                    } else {
                        Err(map_store(error))
                    };
                }
            }
            DeliveryOutcome::Unknown => {
                if let Err(error) = self.store.delivery_unknown(&claim).await {
                    drop(heartbeat);
                    return if is_stale(&error) {
                        Ok(())
                    } else {
                        Err(map_store(error))
                    };
                }
            }
        }
        drop(heartbeat);
        Ok(())
    }

    fn start_heartbeat(
        &self,
        claim: Claim,
        sender: mpsc::UnboundedSender<HeartbeatEvent>,
    ) -> AbortOnDropHandle<()> {
        let store = self.store.clone();
        let interval = self.config.heartbeat_interval;
        let lease = self.config.lease;
        AbortOnDropHandle::new(tokio::spawn(async move {
            loop {
                time::sleep(interval).await;
                match store.heartbeat_state(&claim, lease).await {
                    Ok(state) if state.cancel_requested => {
                        let _ = sender.send(HeartbeatEvent::Cancelled);
                    }
                    Ok(_) => {}
                    Err(error) if is_stale(&error) => {
                        let _ = sender.send(HeartbeatEvent::Stale);
                        return;
                    }
                    Err(_) => {
                        let _ = sender.send(HeartbeatEvent::Database);
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
    Delivered(i32),
    Rejected,
    Unknown,
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

fn worker_request(claim: &Claim) -> Result<WorkerRequest, ()> {
    let request = &claim.request;
    if request.media_mode() != DomainMediaMode::Video
        || request.clip_range().is_some()
        || request.params().max_height().is_some()
    {
        return Err(());
    }
    Ok(WorkerRequest {
        version: PROTOCOL_VERSION,
        operation_id: claim.operation_id,
        attempt: claim.attempt,
        source_url: HttpSourceUrl::parse(request.source_url().as_str()).map_err(|_| ())?,
        task: TaskKind::Download {
            media_mode: MediaMode::Video,
        },
    })
}

fn retryable_executor_error(error: ExecutorError) -> bool {
    match error {
        ExecutorError::Timeout | ExecutorError::Spawn | ExecutorError::Io => true,
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

fn validate_config(config: &DownloadConfig) -> Result<(), DownloadError> {
    if !(1..=8).contains(&config.concurrency)
        || config.lease < Duration::from_secs(1)
        || config.lease > Duration::from_secs(300)
        || config.lease.subsec_nanos() != 0
        || config.heartbeat_interval.is_zero()
        || config.poll_interval < Duration::from_millis(10)
        || config.delivery_timeout.is_zero()
        || config.delivery_timeout > config.lease
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

fn map_store(error: StoreError) -> DownloadError {
    match error {
        StoreError::InvalidInput | StoreError::InvalidStoredData => DownloadError::InvalidInput,
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
        DownloadConfig {
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
        }
    }

    #[test]
    fn accepts_valid_bounded_service_policy() {
        assert_eq!(validate_config(&valid_config()), Ok(()));
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
}
