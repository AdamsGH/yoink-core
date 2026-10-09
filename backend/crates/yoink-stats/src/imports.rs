//! Bounded, restart-ephemeral Desktop import jobs. No core identities are created.
mod desktop;
mod parser;
use crate::{Error, Stats};
pub use desktop::ImportCounts;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use yoink_access::Role;

#[derive(Clone, Debug)]
pub struct ImportConfig {
    pub root: PathBuf,
    pub max_bytes: usize,
    pub max_messages: usize,
    pub max_jobs: usize,
    pub timeout: Duration,
}
impl ImportConfig {
    pub fn validate(&self) -> Result<(), Error> {
        if !self.root.is_absolute()
            || self.root.parent().is_none()
            || self.root.components().any(|c| {
                !matches!(
                    c,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
            || self.max_bytes == 0
            || self.max_bytes > 16 * 1024 * 1024
            || self.max_messages == 0
            || self.max_messages > 100_000
            || self.max_jobs == 0
            || self.max_jobs > 128
            || self.timeout.is_zero()
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct ImportStatus {
    pub job_id: String,
    pub status: &'static str,
    pub inserted: u64,
    pub skipped: u64,
    pub events: u64,
    pub processed: u64,
    pub total: u64,
    pub error: Option<&'static str>,
}
struct Job {
    actor: i64,
    status: ImportStatus,
}
#[derive(Default)]
struct State {
    closed: bool,
    sequence: u64,
    jobs: BTreeMap<u64, Job>,
}
struct Inner {
    stats: Stats,
    config: ImportConfig,
    state: Mutex<State>,
    tasks: TaskTracker,
    cancellation: CancellationToken,
    intake: Arc<tokio::sync::Semaphore>,
    #[cfg(test)]
    desktop_pause: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}
#[derive(Clone)]
pub struct ImportJobs(Arc<Inner>);
/// Capacity remains owned through requests, jobs and canceled blocking work.
pub struct ImportInput {
    owner: Arc<Inner>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}
impl ImportJobs {
    pub fn new(stats: Stats, config: ImportConfig) -> Result<Self, Error> {
        config.validate()?;
        let intake = Arc::new(tokio::sync::Semaphore::new(config.max_jobs));
        Ok(Self(Arc::new(Inner {
            stats,
            config,
            state: Mutex::new(State::default()),
            intake,
            tasks: TaskTracker::new(),
            cancellation: CancellationToken::new(),
            #[cfg(test)]
            desktop_pause: Mutex::new(None),
        })))
    }
    pub fn config(&self) -> &ImportConfig {
        &self.0.config
    }
    pub fn reserve_input(&self) -> Result<Arc<ImportInput>, Error> {
        let state = self.0.state.lock().map_err(|_| Error::Unavailable)?;
        if state.closed {
            return Err(Error::Unavailable);
        }
        let permit = self
            .0
            .intake
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Unavailable)?;
        Ok(Arc::new(ImportInput {
            owner: self.0.clone(),
            _permit: permit,
        }))
    }
    pub async fn blocking<T: Send + 'static>(
        &self,
        input: Arc<ImportInput>,
        action: impl FnOnce() -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        if !Arc::ptr_eq(&input.owner, &self.0) {
            return Err(Error::Invalid);
        }
        // Tuple fields drop in order: dispose undelivered results before releasing capacity.
        let (result, _input) =
            yoink_stats_search::tracked_blocking(&self.0.tasks, move || Ok((action(), input)))
                .await
                .map_err(|_| Error::Unavailable)?;
        result
    }
    pub async fn authorize(&self, actor: i64) -> Result<(), Error> {
        if self.0.stats.role(actor).await? == Role::Owner {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }
    pub async fn start(
        &self,
        actor: i64,
        chat: i64,
        bytes: Vec<u8>,
    ) -> Result<ImportStatus, Error> {
        self.authorize(actor).await?;
        let input = self.reserve_input()?;
        self.start_reserved(actor, chat, bytes, input).await
    }
    pub async fn start_reserved(
        &self,
        actor: i64,
        chat: i64,
        bytes: Vec<u8>,
        input: Arc<ImportInput>,
    ) -> Result<ImportStatus, Error> {
        if !Arc::ptr_eq(&input.owner, &self.0) {
            return Err(Error::Invalid);
        }
        self.authorize(actor).await?;
        if chat >= 0 || bytes.is_empty() || bytes.len() > self.0.config.max_bytes {
            return Err(Error::Invalid);
        }
        let mut state = self.0.state.lock().map_err(|_| Error::Unavailable)?;
        if state.closed {
            return Err(Error::Unavailable);
        }
        if state.jobs.len() == self.0.config.max_jobs {
            let completed = state
                .jobs
                .iter()
                .find(|(_, j)| j.status.status != "running")
                .map(|(id, _)| *id);
            if let Some(id) = completed {
                state.jobs.remove(&id);
            } else {
                return Err(Error::Unavailable);
            }
        }
        state.sequence = state.sequence.checked_add(1).ok_or(Error::Unavailable)?;
        let id = state.sequence;
        let status = ImportStatus {
            job_id: id.to_string(),
            status: "running",
            inserted: 0,
            skipped: 0,
            events: 0,
            processed: 0,
            total: 0,
            error: None,
        };
        state.jobs.insert(
            id,
            Job {
                actor,
                status: status.clone(),
            },
        );
        let jobs = self.clone();
        // Spawn registration and admission closure share the same lock.
        self.0.tasks.spawn(async move {
            let ownership = (&jobs.0.tasks, Some(input.clone()));
            #[cfg(test)]
            let pause = jobs.0.desktop_pause.lock().unwrap().clone();
            #[cfg(test)]
            let import = jobs.0.stats.desktop_inner(
                actor,
                chat,
                bytes,
                jobs.0.config.max_messages,
                ownership,
                pause
                    .as_ref()
                    .map(|(entered, release)| (&**entered, &**release)),
            );
            #[cfg(not(test))]
            let import = jobs.0.stats.desktop_inner(
                actor,
                chat,
                bytes,
                jobs.0.config.max_messages,
                ownership,
            );
            let result = tokio::select! {
                biased;
                _ = jobs.0.cancellation.cancelled() => Err(Error::Unavailable),
                result = tokio::time::timeout(jobs.0.config.timeout,
                    import) =>
                    result.unwrap_or(Err(Error::Unavailable)),
            };
            if let Ok(mut state) = jobs.0.state.lock()
                && let Some(job) = state.jobs.get_mut(&id)
            {
                match result {
                    Ok(c) => {
                        job.status.status = "done";
                        job.status.inserted = c.inserted;
                        job.status.skipped = c.skipped;
                        job.status.events = c.events;
                        job.status.processed = c.total;
                        job.status.total = c.total;
                    }
                    Err(e) => {
                        job.status.status = "error";
                        job.status.error = Some(match e {
                            Error::Forbidden => "access_denied",
                            Error::Invalid => "invalid_export",
                            Error::Unavailable => "import_stopped",
                            _ => "import_failed",
                        });
                    }
                }
            }
        });
        Ok(status)
    }
    pub async fn status(&self, actor: i64, id: &str) -> Result<Option<ImportStatus>, Error> {
        self.authorize(actor).await?;
        let id: u64 = id.parse().map_err(|_| Error::Invalid)?;
        let state = self.0.state.lock().map_err(|_| Error::Unavailable)?;
        Ok(state
            .jobs
            .get(&id)
            .filter(|j| j.actor == actor)
            .map(|j| j.status.clone()))
    }
    pub fn stop_admission(&self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.closed = true;
            self.0.tasks.close();
        }
    }
    pub fn cancel_all(&self) {
        self.stop_admission();
        self.0.cancellation.cancel();
    }
    pub async fn drain(&self) {
        self.stop_admission();
        self.0.tasks.wait().await;
    }
}

#[cfg(test)]
mod tests;
