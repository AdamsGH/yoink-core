use crate::{
    product_config::{MusicConfig, StatsConfig},
    runtime::ServerError,
};
use std::{process::Stdio, sync::Arc};
use tokio::process::{Child, Command};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use yoink_identity::IdentityStore;
use yoink_music::{MusicService, NativeMusicAuthority, NativeMusicPersistence};
use yoink_storage::Store;
use yoink_telegram::{
    TelegramAdapter,
    music_runtime::{MusicRuntime, MusicTransportBudget},
    stats_runtime::{LookupBudget, StatsRuntime},
};

/// Retained outside the cancelable startup future, including the search factory's
/// unpublished blocking work and any launcher child awaiting preflight.
#[derive(Default)]
pub struct Products {
    pub music: Option<MusicService>,
    pub persistence: Option<Arc<NativeMusicPersistence>>,
    pub music_handlers: Option<MusicRuntime>,
    pub stats: Option<yoink_stats::Stats>,
    pub stats_handlers: Option<StatsRuntime>,
    pub stats_config: Option<StatsConfig>,
    pub imports: Option<yoink_http::stats_import::ImportJobs>,
    pub executor: Option<Arc<yoink_executor::Executor>>,
    pub preflight: Option<Child>,
    pub search_tasks: TaskTracker,
}

impl Products {
    async fn check_worker(
        &mut self,
        executor: &yoink_executor::ExecutorConfig,
    ) -> Result<(), ServerError> {
        self.preflight = Some(
            Command::new(&executor.program)
                .arg("--check")
                .current_dir(&executor.workspace_root)
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|_| ServerError::MusicStartup)?,
        );
        let status = self
            .preflight
            .as_mut()
            .expect("owned preflight")
            .wait()
            .await
            .map_err(|_| ServerError::MusicStartup)?;
        self.preflight = None;
        if !status.success() {
            return Err(ServerError::MusicStartup);
        }
        Ok(())
    }

    pub async fn music(
        &mut self,
        config: MusicConfig,
        store: Arc<Store>,
        identity: Arc<IdentityStore>,
        adapter: TelegramAdapter,
        groups: bool,
    ) -> Result<TelegramAdapter, ServerError> {
        self.check_worker(&config.executor).await?;
        let executor = Arc::new(
            yoink_executor::Executor::new(config.executor)
                .map_err(|_| ServerError::MusicStartup)?,
        );
        self.executor = Some(executor.clone());
        let persistence = Arc::new(NativeMusicPersistence {
            database: store.database().clone(),
            store,
        });
        self.persistence = Some(persistence.clone());
        let delivery = Arc::new(
            adapter
                .music_delivery(MusicTransportBudget {
                    timeout: config.settings.delivery_timeout,
                    response_bytes: config.response_bytes,
                })
                .map_err(|_| ServerError::MusicStartup)?,
        );
        let service = MusicService::new(
            Arc::new(NativeMusicAuthority(identity.clone())),
            executor,
            persistence.clone(),
            config.settings,
        )
        .map_err(|_| ServerError::MusicStartup)?;
        self.music = Some(service.clone());
        let handlers = MusicRuntime::new(service, persistence, identity, delivery, groups)
            .map_err(|_| ServerError::MusicStartup)?;
        self.music_handlers = Some(handlers.clone());
        Ok(adapter.with_music(handlers, config.observation_max_age))
    }

    pub async fn stats(
        &mut self,
        config: StatsConfig,
        store: Arc<Store>,
        identity: Arc<IdentityStore>,
        session: Option<Arc<yoink_user_session::Client>>,
        adapter: TelegramAdapter,
    ) -> Result<TelegramAdapter, ServerError> {
        let membership = Arc::new(
            adapter
                .stats_membership_provider(LookupBudget {
                    timeout: config.names.lookup_timeout,
                    response_bytes: config.response_bytes,
                })
                .map_err(|_| ServerError::StatsStartup)?,
        );
        self.open_stats(&config, &store, identity, membership, session)
            .await?;
        let stats = self.stats.as_ref().expect("owned Stats").clone();
        self.imports = Some(
            yoink_http::stats_import::ImportJobs::new(
                stats.clone(),
                yoink_http::stats_import::ImportConfig {
                    root: config.import_root.clone(),
                    max_bytes: config.import_max_bytes,
                    max_messages: config.import_max_messages,
                    max_jobs: config.import_max_jobs,
                    timeout: config.import_timeout,
                },
            )
            .map_err(|_| ServerError::StatsStartup)?,
        );
        let delivery = adapter
            .text_delivery(store, config.text_timeout)
            .map_err(|_| ServerError::StatsStartup)?;
        let handlers = StatsRuntime::new(stats, delivery);
        self.stats_handlers = Some(handlers.clone());
        self.stats_config = Some(config);
        Ok(adapter.with_stats(handlers))
    }

    async fn open_stats(
        &mut self,
        config: &StatsConfig,
        store: &Store,
        identity: Arc<IdentityStore>,
        membership: Arc<dyn yoink_stats::MembershipProvider>,
        session: Option<Arc<yoink_user_session::Client>>,
    ) -> Result<(), ServerError> {
        let stats = yoink_stats::Stats::open_with_search_tasks(
            store.database().clone(),
            identity,
            membership,
            session,
            &config.index_dir,
            self.search_tasks.clone(),
        )
        .await
        .map_err(|_| ServerError::StatsStartup)?;
        self.stats = Some(stats);
        Ok(())
    }

    pub async fn shutdown(&mut self) -> Result<(), ServerError> {
        // Call only after HTTP admission closes and adapter/job futures are joined.
        let mut result = Ok(());
        if let Some(mut child) = self.preflight.take() {
            let _ = child.start_kill();
            if child.wait().await.is_err() {
                result = Err(ServerError::MusicStartup);
            }
        }
        let music_failed = match (&self.music_handlers, &self.music) {
            (Some(handlers), _) => handlers.drain_handlers().await.is_err(),
            (None, Some(service)) => service.shutdown().await.is_err(),
            (None, None) => false,
        };
        if music_failed {
            result = result.and(Err(ServerError::Music));
        }
        if let Some(executor) = &self.executor {
            executor.shutdown_music().await;
        }
        if let Some(imports) = &self.imports {
            imports.stop_admission();
            imports.cancel_all();
            imports.drain().await;
        }
        let stats_failed = match (&self.stats_handlers, &self.stats) {
            (Some(handlers), _) => handlers.shutdown().await.is_err(),
            (None, Some(stats)) => stats.shutdown().await.is_err(),
            (None, None) => false,
        };
        if stats_failed {
            result = result.and(Err(ServerError::Stats));
        }
        self.search_tasks.close();
        self.search_tasks.wait().await;
        result
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "runtime_products_tests.rs"]
mod runtime_products_tests;

pub async fn run_stats_jobs(
    runtime: StatsRuntime,
    config: StatsConfig,
    cancel: CancellationToken,
) -> Result<(), ServerError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Ok(()),
        result = runtime.background(config.names, config.search_interval) => result.map_err(|_| ServerError::Stats),
    }
}
