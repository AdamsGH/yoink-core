use std::{future::Future, io, process::Stdio, sync::Arc, time::Duration};

use thiserror::Error;
use tokio::{net::TcpListener, process::Command, task::JoinSet, time::timeout};
use tokio_util::sync::CancellationToken;
use yoink_application::SystemService;
use yoink_telegram::TelegramAdapter;

use crate::config::Config;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ServerError {
    #[error("could not bind HTTP listener")]
    Bind,
    #[error("could not initialize signal handling")]
    Signal,
    #[error("Telegram initialization failed; verify the test token and Bot API endpoint")]
    TelegramStartup,
    #[error("database initialization failed; verify connectivity and run explicit migrations")]
    DatabaseStartup,
    #[error("database health check failed; adapters have been stopped")]
    DatabaseUnavailable,
    #[error("database shutdown failed")]
    DatabaseShutdown,
    #[error("dependency initialization exceeded the configured deadline")]
    StartupTimeout,
    #[error("HTTP adapter failed")]
    Http,
    #[error("Telegram adapter failed")]
    Telegram,
    #[error("an adapter stopped unexpectedly")]
    UnexpectedExit,
    #[error("an adapter task panicked or was cancelled unexpectedly")]
    Task,
    #[error("shutdown deadline exceeded; remaining adapter tasks were aborted")]
    ShutdownTimeout,
    #[error(
        "media execution requires the experimental container image and valid bounded configuration"
    )]
    MediaStartup,
    #[error("media execution service failed")]
    Media,
}

pub struct CookieProviderOverrides {
    pub oauth: Arc<dyn yoink_downloads::cookie_oauth::OAuthProvider>,
    pub refresh: Arc<dyn yoink_downloads::profile::oauth_refresh::OAuthRefreshProvider>,
    pub account: Arc<dyn yoink_downloads::cookie_web_live::AccountProvider>,
}

#[cfg_attr(feature = "test-fixtures", allow(dead_code))]
pub async fn run(
    config: Config,
    shutdown_signal: impl Future<Output = io::Result<()>>,
) -> Result<(), ServerError> {
    run_impl(config, shutdown_signal, None, None).await
}

#[cfg(feature = "test-fixtures")]
#[allow(
    dead_code,
    reason = "Only the isolated fixture binary calls this entry point"
)]
pub async fn run_with_store(
    config: Config,
    shutdown_signal: impl Future<Output = io::Result<()>>,
    store: Arc<yoink_storage::Store>,
) -> Result<(), ServerError> {
    run_with_store_and_cookie_providers(config, shutdown_signal, store, None).await
}

#[cfg(feature = "test-fixtures")]
#[allow(
    dead_code,
    reason = "Only the isolated fixture binary calls this entry point"
)]
pub async fn run_with_store_and_cookie_providers(
    config: Config,
    shutdown_signal: impl Future<Output = io::Result<()>>,
    store: Arc<yoink_storage::Store>,
    providers: Option<CookieProviderOverrides>,
) -> Result<(), ServerError> {
    let database = config
        .database
        .as_ref()
        .ok_or(ServerError::DatabaseStartup)?;
    let configured = database
        .path
        .parent()
        .and_then(|parent| std::fs::canonicalize(parent).ok())
        .map(|parent| parent.join(database.path.file_name().unwrap_or_default()));
    if configured.as_deref() != Some(store.database().path()) {
        return Err(ServerError::DatabaseStartup);
    }
    run_impl(config, shutdown_signal, Some(store), providers).await
}

async fn run_impl(
    config: Config,
    shutdown_signal: impl Future<Output = io::Result<()>>,
    provided_store: Option<Arc<yoink_storage::Store>>,
    cookie_providers: Option<CookieProviderOverrides>,
) -> Result<(), ServerError> {
    let system = SystemService::new(env!("CARGO_PKG_VERSION"));
    let cancel = CancellationToken::new();
    let listener = TcpListener::bind(config.http_bind)
        .await
        .map_err(|_| ServerError::Bind)?;
    let address = listener.local_addr().map_err(|_| ServerError::Bind)?;
    tokio::pin!(shutdown_signal);
    let mut tasks = JoinSet::new();
    let mut cookie_oauth_lifecycle = None;
    let auth_state =
        yoink_http::AuthState::new(config.auth.as_ref().is_some_and(|auth| auth.dev_enabled));
    let http_auth = config.auth.as_ref().map(|_| auth_state.clone());
    let http_identities = auth_state.clone();
    let http_system = system.clone();
    let http_cancel = cancel.clone();
    let user_session = config
        .user_session
        .clone()
        .map(yoink_user_session::Client::new)
        .transpose()
        .map_err(|_| ServerError::Http)?
        .map(Arc::new);
    tasks.spawn(async move {
        yoink_http::serve_with_user_session(
            listener,
            http_system,
            http_auth,
            http_identities,
            user_session,
            http_cancel,
        )
        .await
        .map_err(|_| ServerError::Http)
    });
    tracing::info!(%address, "HTTP diagnostics listening");
    let owner_id = config.telegram.as_ref().map(|telegram| telegram.owner_id);
    let groups_enabled = config
        .telegram
        .as_ref()
        .is_some_and(|telegram| telegram.groups_enabled);
    let database_timings = config
        .database
        .as_ref()
        .map(|db| (db.health_interval, db.acquire_timeout));
    let mut database: Option<Arc<yoink_storage::Store>> = None;
    let mut identities: Option<Arc<yoink_identity::IdentityStore>> = None;
    let mut downloads = None;
    let prepare = async {
        if let Some(database_config) = &config.database {
            let store = match provided_store.clone() {
                Some(store) => store,
                None => Arc::new(
                    database_config
                        .open(false)
                        .await
                        .map_err(|_| ServerError::DatabaseStartup)?,
                ),
            };
            database = Some(store.clone());
            store
                .verify_schema()
                .await
                .map_err(|_| ServerError::DatabaseStartup)?;
            let identity = Arc::new(yoink_identity::IdentityStore::new(
                store.database().clone(),
                owner_id
                    .map(|id| id as i64)
                    .or_else(|| config.auth.as_ref().map(|auth| auth.owner_id)),
                false,
            ));
            auth_state
                .install_identities(identity.clone())
                .map_err(|_| ServerError::DatabaseStartup)?;
            auth_state
                .install_downloader_settings(yoink_downloads::UserSettingsService::new(
                    store.database().clone(),
                    identity.clone(),
                ))
                .map_err(|_| ServerError::DatabaseStartup)?;
            identities = Some(identity);
        }
        if let Some(auth) = config.auth {
            let service = yoink_http::AuthService {
                identities: identities.clone().ok_or(ServerError::DatabaseStartup)?,
                crypto: auth.crypto,
            };
            auth_state
                .install(service)
                .map_err(|_| ServerError::DatabaseStartup)?;
        }
        let mut telegram = match config.telegram {
            Some(telegram) => Some(
                TelegramAdapter::prepare(
                    telegram.token.into_inner(),
                    telegram.api_url,
                    telegram.owner_id,
                    system.clone(),
                    telegram.poll_timeout,
                )
                .await
                .map_err(|_| ServerError::TelegramStartup)?
                .with_inline_search(telegram.inline_search)
                .map_err(|_| ServerError::TelegramStartup)?
                .with_progress(telegram.progress)
                .with_cookie_upload(telegram.cookie_upload)
                .with_cookie_url(telegram.cookie_url)
                .with_group_access(telegram.groups_enabled),
            ),
            None => None,
        };
        if let Some(identities) = identities.clone() {
            telegram = telegram.map(|adapter| adapter.with_identity_store(identities));
        }
        if let Some(media) = config.media {
            let store = database.as_ref().ok_or(ServerError::MediaStartup)?;
            let adapter = telegram.take().ok_or(ServerError::MediaStartup)?;
            let identities = identities.clone().ok_or(ServerError::MediaStartup)?;
            let worker_workspace = media.executor.workspace_root.clone();
            let executor = yoink_executor::Executor::new(media.executor)
                .map_err(|_| ServerError::MediaStartup)?;
            let executor = if media.youtube_auth_adapter {
                executor.with_youtube_auth_adapter()
            } else {
                executor
            };
            // The enclosing startup deadline also bounds this check. The configured
            // workspace must already exist, as required by Executor::new.
            let status = Command::new("/usr/local/bin/yoink-worker-launcher")
                .arg("--check")
                .current_dir(worker_workspace)
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status()
                .await
                .map_err(|_| ServerError::MediaStartup)?;
            if !status.success() {
                return Err(ServerError::MediaStartup);
            }
            let service = yoink_downloads::DownloadService::new(
                store.clone(),
                identities,
                executor,
                adapter.delivery(),
                media.downloads,
            )
            .map_err(|_| ServerError::MediaStartup)?
            .with_network_config(media.network)
            .with_group_access(groups_enabled);
            service
                .check_profile_schema()
                .await
                .map_err(|_| ServerError::MediaStartup)?;
            let service = if let Some(origin) = media.cookie_web.origin {
                service
                    .with_cookie_web_origin(origin.as_str())
                    .map_err(|_| ServerError::MediaStartup)?
            } else {
                service
            };
            let providers = match cookie_providers {
                Some(providers) => providers,
                None => {
                    let oauth_config = yoink_downloads::cookie_oauth::OAuthProviderConfig::new(
                        media.cookie_web.oauth_timeout,
                    )
                    .map_err(|_| ServerError::MediaStartup)?;
                    let oauth = Arc::new(
                        yoink_downloads::cookie_oauth::YouTubeOAuthProvider::with_config(
                            oauth_config,
                        )
                        .map_err(|_| ServerError::MediaStartup)?,
                    );
                    let account_config =
                        yoink_downloads::cookie_web_live::AccountProviderConfig::new(
                            media.cookie_web.account_timeout,
                        )
                        .map_err(|_| ServerError::MediaStartup)?;
                    let account = Arc::new(
                        yoink_downloads::cookie_web_live::LiveAccountProvider::with_config(
                            account_config,
                        )
                        .map_err(|_| ServerError::MediaStartup)?,
                    );
                    CookieProviderOverrides {
                        refresh: oauth.clone(),
                        oauth,
                        account,
                    }
                }
            };
            let service = service
                .with_oauth_refresh(providers.refresh)
                .with_oauth_refresh_timeout(media.cookie_web.oauth_timeout)
                .map_err(|_| ServerError::MediaStartup)?
                .with_cookie_oauth_provider(providers.oauth)
                .with_cookie_oauth_poll_config(media.cookie_web.oauth_poll)
                .map_err(|_| ServerError::MediaStartup)?
                .with_cookie_account_provider(providers.account);

            cookie_oauth_lifecycle =
                Some((service.clone(), media.cookie_web.oauth_cleanup_interval));
            auth_state
                .install_cookies(service.clone())
                .map_err(|_| ServerError::MediaStartup)?;
            telegram = Some(adapter.with_downloads(service.clone()));
            downloads = Some(service);
        }
        if let Some(adapter) = &mut telegram {
            adapter.initialize_menus().await;
        }
        Ok(telegram)
    };
    enum Startup {
        Prepared(Result<Option<Box<TelegramAdapter>>, ServerError>),
        Stopped(Result<(), ServerError>),
    }
    let startup = tokio::select! {
        result = timeout(config.startup_timeout, prepare) => Startup::Prepared(result.unwrap_or(Err(ServerError::StartupTimeout)).map(|adapter| adapter.map(Box::new))),
        signal = &mut shutdown_signal => Startup::Stopped(signal.map_err(|_| ServerError::Signal)),
        completed = tasks.join_next() => Startup::Stopped(Err(adapter_exit(completed))),
    };
    if let Some((service, interval)) = cookie_oauth_lifecycle {
        tasks.spawn(run_cookie_oauth_lifecycle(
            service,
            cancel.clone(),
            interval,
        ));
    }
    if let Some(database) = database.clone() {
        let database_cancel = cancel.clone();
        let (health_interval, health_timeout) =
            database_timings.expect("configured database timings");
        tasks.spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = database_cancel.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(health_interval) => {
                        let check = tokio::select! {
                            biased;
                            _ = database_cancel.cancelled() => return Ok(()),
                            check = timeout(health_timeout, database.verify_schema()) => check,
                        };
                        if !matches!(check, Ok(Ok(()))) {
                            return Err(ServerError::DatabaseUnavailable);
                        }
                    }
                }
            }
        });
    }
    let telegram = match startup {
        Startup::Prepared(Ok(telegram)) => telegram,
        Startup::Prepared(Err(error)) => {
            let result =
                stop_and_drain(tasks, system, cancel, config.shutdown_timeout, Err(error)).await;
            return finish_database(result, database, config.shutdown_timeout).await;
        }
        Startup::Stopped(result) => {
            let result =
                stop_and_drain(tasks, system, cancel, config.shutdown_timeout, result).await;
            return finish_database(result, database, config.shutdown_timeout).await;
        }
    };
    let telegram_enabled = telegram.is_some();
    let bot_username = telegram
        .as_ref()
        .map(|bot| format!("@{}", bot.bot_username()));
    let media_enabled = downloads.is_some();
    if let Some(downloads) = downloads {
        let media_cancel = cancel.clone();
        tasks.spawn(async move {
            downloads.run(media_cancel).await.map_err(|error| {
                tracing::error!(error = %error, "media execution service stopped");
                ServerError::Media
            })
        });
        system.set_downloads_enabled(true);
    }
    if let Some(telegram) = telegram {
        let telegram_cancel = cancel.clone();
        tasks.spawn(async move {
            telegram
                .run(telegram_cancel)
                .await
                .map_err(|_| ServerError::Telegram)
        });
    }
    system.set_bot_status(telegram_enabled, bot_username);
    system.set_ready(true);
    tracing::info!(%address, telegram_enabled, media_enabled, "diagnostic adapters ready");
    let result = supervise(
        tasks,
        system,
        cancel,
        config.shutdown_timeout,
        shutdown_signal,
    )
    .await;
    finish_database(result, database, config.shutdown_timeout).await
}

async fn run_cookie_oauth_lifecycle(
    service: yoink_downloads::DownloadService,
    cancel: CancellationToken,
    interval: Duration,
) -> Result<(), ServerError> {
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(interval) => {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => break,
                    result = service.cleanup_cookie_oauth(epoch_seconds()) => {
                        if result.is_err() {
                            tracing::warn!("cookie OAuth expiry cleanup failed");
                        }
                    }
                }
            }
        }
    }
    service.drain_cookie_oauth().await;
    Ok(())
}

fn epoch_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs().min(i64::MAX as u64) as i64)
}

async fn supervise(
    mut tasks: JoinSet<Result<(), ServerError>>,
    system: SystemService,
    cancel: CancellationToken,
    shutdown_timeout: Duration,
    shutdown_signal: impl Future<Output = io::Result<()>>,
) -> Result<(), ServerError> {
    let initial = tokio::select! {
        signal = shutdown_signal => signal.map_err(|_| ServerError::Signal),
        completed = tasks.join_next() => Err(adapter_exit(completed)),
    };
    stop_and_drain(tasks, system, cancel, shutdown_timeout, initial).await
}

fn adapter_exit(
    completed: Option<Result<Result<(), ServerError>, tokio::task::JoinError>>,
) -> ServerError {
    match completed {
        Some(Ok(Err(error))) => error,
        Some(Err(_)) => ServerError::Task,
        _ => ServerError::UnexpectedExit,
    }
}

async fn stop_and_drain(
    mut tasks: JoinSet<Result<(), ServerError>>,
    system: SystemService,
    cancel: CancellationToken,
    shutdown_timeout: Duration,
    initial: Result<(), ServerError>,
) -> Result<(), ServerError> {
    system.set_ready(false);
    system.set_bot_status(false, None);
    cancel.cancel();
    let drain = timeout(shutdown_timeout, async {
        let mut error = None;
        while let Some(completed) = tasks.join_next().await {
            match completed {
                Ok(Ok(())) => {}
                Ok(Err(failure)) => {
                    error.get_or_insert(failure);
                }
                Err(_) => {
                    error.get_or_insert(ServerError::Task);
                }
            }
        }
        error.map_or(Ok(()), Err)
    })
    .await;
    let drained = match drain {
        Ok(result) => result,
        Err(_) => {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            Err(ServerError::ShutdownTimeout)
        }
    };
    initial.and(drained)
}

async fn finish_database(
    result: Result<(), ServerError>,
    database: Option<Arc<yoink_storage::Store>>,
    deadline: Duration,
) -> Result<(), ServerError> {
    let closed = if let Some(database) = database {
        timeout(deadline, database.close())
            .await
            .map_err(|_| ServerError::ShutdownTimeout)?
            .map_err(|_| ServerError::DatabaseShutdown)
    } else {
        Ok(())
    };
    closed.and(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn clean_shutdown_cancels_owned_tasks_and_clears_readiness() {
        let system = SystemService::new("test");
        system.set_ready(true);
        let cancel = CancellationToken::new();
        let child = cancel.clone();
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
            child.cancelled().await;
            Ok(())
        });
        assert_eq!(
            supervise(
                tasks,
                system.clone(),
                cancel.clone(),
                Duration::from_secs(1),
                async { Ok(()) }
            )
            .await,
            Ok(())
        );
        assert!(!system.snapshot().ready);
        assert!(cancel.is_cancelled());
    }

    #[tokio::test]
    async fn unexpected_exit_cancels_the_sibling_and_fails() {
        let system = SystemService::new("test");
        system.set_ready(true);
        let cancel = CancellationToken::new();
        let child = cancel.clone();
        let mut tasks = JoinSet::new();
        tasks.spawn(async { Ok(()) });
        tasks.spawn(async move {
            child.cancelled().await;
            Ok(())
        });
        assert_eq!(
            supervise(
                tasks,
                system.clone(),
                cancel.clone(),
                Duration::from_secs(1),
                std::future::pending()
            )
            .await,
            Err(ServerError::UnexpectedExit)
        );
        assert!(!system.snapshot().ready);
        assert!(cancel.is_cancelled());
    }

    #[tokio::test]
    async fn uncooperative_task_is_aborted_after_deadline() {
        let mut tasks = JoinSet::new();
        tasks.spawn(std::future::pending());
        assert_eq!(
            supervise(
                tasks,
                SystemService::new("test"),
                CancellationToken::new(),
                Duration::from_millis(10),
                async { Ok(()) }
            )
            .await,
            Err(ServerError::ShutdownTimeout)
        );
    }

    #[tokio::test]
    async fn task_failure_is_not_a_successful_shutdown() {
        let mut tasks = JoinSet::new();
        tasks.spawn(async { Err(ServerError::Http) });
        assert_eq!(
            supervise(
                tasks,
                SystemService::new("test"),
                CancellationToken::new(),
                Duration::from_secs(1),
                std::future::pending()
            )
            .await,
            Err(ServerError::Http)
        );
    }
}

#[cfg(test)]
#[path = "runtime_cookie_tests.rs"]
mod cookie_oauth_tests;
