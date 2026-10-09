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
    #[error("Insight initialization failed")]
    InsightStartup,
    #[error("Inbox initialization failed")]
    InboxStartup,
    #[error("Inbox owned runtime failed")]
    Inbox,
    #[error("Music initialization failed")]
    MusicStartup,
    #[error("Music owned runtime failed")]
    Music,
    #[error("Stats initialization failed")]
    StatsStartup,
    #[error("Stats owned runtime failed")]
    Stats,
}

pub struct CookieProviderOverrides {
    pub oauth: Arc<dyn yoink_downloads::cookie_oauth::OAuthProvider>,
    pub refresh: Arc<dyn yoink_downloads::profile::oauth_refresh::OAuthRefreshProvider>,
    pub account: Arc<dyn yoink_downloads::cookie_web_live::AccountProvider>,
}

#[cfg(feature = "test-fixtures")]
pub type ConfirmationControl =
    Arc<std::sync::Mutex<Option<yoink_telegram::confirmation_hold::ConfirmationHold>>>;

#[derive(Default)]
pub struct DependencyOverrides {
    pub cookies: Option<CookieProviderOverrides>,
    #[cfg(feature = "test-fixtures")]
    pub insight_root_der: Option<Vec<u8>>,
    #[cfg(feature = "test-fixtures")]
    pub confirmation: Option<ConfirmationControl>,
}

#[cfg_attr(feature = "test-fixtures", allow(dead_code))]
pub async fn run(
    config: Config,
    shutdown_signal: impl Future<Output = io::Result<()>>,
) -> Result<(), ServerError> {
    run_impl(
        config,
        shutdown_signal,
        None,
        DependencyOverrides::default(),
    )
    .await
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
    run_with_store_and_overrides(
        config,
        shutdown_signal,
        store,
        DependencyOverrides {
            cookies: providers,
            ..Default::default()
        },
    )
    .await
}

#[cfg(feature = "test-fixtures")]
pub async fn run_with_store_and_overrides(
    config: Config,
    shutdown_signal: impl Future<Output = io::Result<()>>,
    store: Arc<yoink_storage::Store>,
    overrides: DependencyOverrides,
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
    run_impl(config, shutdown_signal, Some(store), overrides).await
}

async fn run_impl(
    config: Config,
    shutdown_signal: impl Future<Output = io::Result<()>>,
    provided_store: Option<Arc<yoink_storage::Store>>,
    overrides: DependencyOverrides,
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
    let late_insight = yoink_http::LateInsight::default();
    let http_insight = late_insight.clone();
    let mut insight_runtime: Option<crate::insight_runtime::InsightRuntime> = None;
    let mut insight_preflight: Option<tokio::process::Child> = None;
    let mut insight_handlers: Option<yoink_telegram::InsightRuntime> = None;
    let mut telegram_handlers: Option<yoink_telegram::TelegramHandlerLifecycle> = None;
    let late_inbox = yoink_http::LateInbox::default();
    let http_inbox = late_inbox.clone();
    let late_music_stats = yoink_http::LateMusicStats::default();
    let http_music_stats = late_music_stats.clone();
    let activity = yoink_http::activity::Activity::default();
    let mut products = crate::product_runtime::Products::default();
    let mut inbox_runtime = None;
    let mut inbox_telegram = None;
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
    let stats_session = user_session.clone();
    tasks.spawn(async move {
        yoink_http::serve_with_late_music_stats(
            listener,
            http_system,
            http_auth,
            http_identities,
            user_session,
            http_inbox,
            http_insight,
            http_music_stats,
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
    let auth_enabled = config.auth.is_some();
    let inbox_owner = owner_id
        .map(|id| id as i64)
        .or_else(|| config.auth.as_ref().map(|auth| auth.owner_id));
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
                config.insight.is_some(),
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
            Some(telegram) => {
                let adapter = TelegramAdapter::prepare(
                    telegram.token.into_inner(),
                    telegram.api_url,
                    telegram.owner_id,
                    system.clone(),
                    telegram.poll_timeout,
                    telegram.request_timeout,
                )
                .await
                .map_err(|error| {
                    tracing::error!(?error, "Telegram preparation failed");
                    ServerError::TelegramStartup
                })?;
                telegram_handlers = Some(adapter.handler_lifecycle());
                Some(
                    adapter
                        .with_inline_search(telegram.inline_search)
                        .map_err(|_| ServerError::TelegramStartup)?
                        .with_progress(telegram.progress)
                        .with_cookie_upload(telegram.cookie_upload)
                        .with_cookie_url(telegram.cookie_url)
                        .with_group_access(telegram.groups_enabled),
                )
            }
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
            .with_metrics(system.metrics())
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
            let providers = match overrides.cookies {
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
        if let Some(insight) = config.insight {
            let text_timeout = insight.text_timeout;
            let store = database.as_ref().ok_or(ServerError::InsightStartup)?;
            let identity = identities.clone().ok_or(ServerError::InsightStartup)?;
            // Verify the immutable launcher in the configured workspace before readiness.
            insight_preflight = Some(
                Command::new("/usr/local/bin/yoink-worker-launcher")
                    .arg("--check")
                    .current_dir(&insight.executor.workspace_root)
                    .env_clear()
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(true)
                    .spawn()
                    .map_err(|_| ServerError::InsightStartup)?,
            );
            let status = insight_preflight
                .as_mut()
                .expect("preflight spawned")
                .wait()
                .await
                .map_err(|_| ServerError::InsightStartup)?;
            insight_preflight = None;
            if !status.success() {
                return Err(ServerError::InsightStartup);
            }
            #[cfg(feature = "test-fixtures")]
            let composed = match overrides.insight_root_der.as_deref() {
                Some(root) => crate::insight_runtime::InsightRuntime::with_fixture_root(
                    insight,
                    store,
                    identity.clone(),
                    inbox_owner,
                    cancel.child_token(),
                    root,
                ),
                None => crate::insight_runtime::InsightRuntime::new(
                    insight,
                    store,
                    identity.clone(),
                    inbox_owner,
                    cancel.child_token(),
                ),
            };
            #[cfg(not(feature = "test-fixtures"))]
            let composed = crate::insight_runtime::InsightRuntime::new(
                insight,
                store,
                identity.clone(),
                inbox_owner,
                cancel.child_token(),
            );
            insight_runtime = Some(composed?);
            if let Some(adapter) = telegram.take() {
                let text = adapter
                    .text_delivery(store.clone(), text_timeout)
                    .map_err(|_| ServerError::InsightStartup)?;
                #[cfg(feature = "test-fixtures")]
                if let Some(control) = &overrides.confirmation {
                    *control.lock().expect("fixture confirmation control") =
                        Some(text.confirmation_hold());
                }
                let handler = yoink_telegram::InsightRuntime::new(
                    insight_runtime
                        .as_ref()
                        .expect("installed runtime")
                        .service
                        .clone(),
                    identity,
                    text,
                    groups_enabled,
                );
                insight_handlers = Some(handler.clone());
                telegram = Some(adapter.with_insight(handler));
            }
            if auth_enabled {
                activity
                    .install_insight(
                        insight_runtime
                            .as_ref()
                            .expect("installed runtime")
                            .service
                            .clone(),
                    )
                    .map_err(|_| ServerError::InsightStartup)?;
                late_insight
                    .install_with_activity(
                        system.clone(),
                        auth_state.clone(),
                        insight_runtime
                            .as_ref()
                            .expect("installed runtime")
                            .http(cancel.child_token()),
                        activity.clone(),
                    )
                    .map_err(|_| ServerError::InsightStartup)?;
            }
        }
        if let Some(inbox) = config.inbox {
            let store = database.as_ref().ok_or(ServerError::InboxStartup)?;
            let identity = identities.clone().ok_or(ServerError::InboxStartup)?;
            let status = Command::new("/usr/local/bin/yoink-worker-launcher")
                .arg("--check")
                .current_dir(&inbox.executor.workspace_root)
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status()
                .await
                .map_err(|_| ServerError::InboxStartup)?;
            if !status.success() {
                return Err(ServerError::InboxStartup);
            }
            let runtime = crate::inbox_runtime::InboxRuntime::new_with_insight(
                inbox,
                store,
                identity.clone(),
                inbox_owner,
                insight_runtime.as_ref().map(|r| r.service.clone()),
            )?;
            if let Some(adapter) = telegram.take() {
                let text = adapter
                    .text_delivery(store.clone(), runtime.config.text_timeout)
                    .map_err(|_| ServerError::InboxStartup)?;
                let notification = yoink_telegram::InboxRuntime::new(
                    runtime.store.clone(),
                    identity,
                    text,
                    groups_enabled,
                )
                .with_config(runtime.config.notifications)
                .map_err(|_| ServerError::InboxStartup)?;
                telegram = Some(adapter.with_inbox(notification.clone()));
                inbox_telegram = Some((notification, runtime.config.interval));
            }
            if auth_enabled {
                late_inbox
                    .install(
                        system.clone(),
                        auth_state.clone(),
                        runtime.http(cancel.child_token()),
                    )
                    .map_err(|_| ServerError::InboxStartup)?;
            }
            inbox_runtime = Some(runtime);
        }
        if let Some(music) = config.music {
            let adapter = telegram.take().ok_or(ServerError::MusicStartup)?;
            telegram = Some(
                products
                    .music(
                        music,
                        database.clone().ok_or(ServerError::MusicStartup)?,
                        identities.clone().ok_or(ServerError::MusicStartup)?,
                        adapter,
                        groups_enabled,
                    )
                    .await?,
            );
            if let Some(persistence) = &products.persistence {
                activity
                    .install("music", persistence.clone())
                    .map_err(|_| ServerError::MusicStartup)?;
            }
        }
        if let Some(stats) = config.stats {
            let adapter = telegram.take().ok_or(ServerError::StatsStartup)?;
            telegram = Some(
                products
                    .stats(
                        stats,
                        database.clone().ok_or(ServerError::StatsStartup)?,
                        identities.clone().ok_or(ServerError::StatsStartup)?,
                        stats_session,
                        adapter,
                    )
                    .await?,
            );
        }
        if let Some(adapter) = &mut telegram {
            adapter.initialize_menus().await;
        }
        if auth_enabled
            && (insight_runtime.is_some() || products.music.is_some() || products.stats.is_some())
        {
            late_music_stats
                .install_with_stats_imports(
                    system.clone(),
                    auth_state.clone(),
                    products.persistence.clone(),
                    products.stats.clone(),
                    activity.clone(),
                    products.imports.clone(),
                )
                .map_err(|_| ServerError::Http)?;
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
            let result = stop_and_drain(
                tasks,
                system,
                cancel,
                config.shutdown_timeout,
                Err(error),
                &late_music_stats,
                telegram_handlers.as_ref(),
            )
            .await;
            drain_insight_preflight(&mut insight_preflight).await;
            if let Some(handlers) = &insight_handlers {
                handlers.drain_handlers().await;
            }
            if let Some(runtime) = &insight_runtime {
                runtime.shutdown().await;
            }
            let result = result.and(products.shutdown().await);
            return finish_database(result, database, config.shutdown_timeout).await;
        }
        Startup::Stopped(result) => {
            let result = stop_and_drain(
                tasks,
                system,
                cancel,
                config.shutdown_timeout,
                result,
                &late_music_stats,
                telegram_handlers.as_ref(),
            )
            .await;
            drain_insight_preflight(&mut insight_preflight).await;
            if let Some(handlers) = &insight_handlers {
                handlers.drain_handlers().await;
            }
            if let Some(runtime) = &insight_runtime {
                runtime.shutdown().await;
            }
            let result = result.and(products.shutdown().await);
            return finish_database(result, database, config.shutdown_timeout).await;
        }
    };
    system.set_inbox_enabled(inbox_runtime.is_some());
    if let Some(runtime) = inbox_runtime {
        tasks.spawn(runtime.run(cancel.child_token()));
    }
    if let (Some(runtime), Some(settings)) =
        (&products.stats_handlers, products.stats_config.take())
    {
        tasks.spawn(crate::product_runtime::run_stats_jobs(
            runtime.clone(),
            settings,
            cancel.child_token(),
        ));
    }
    if let Some((runtime, interval)) = inbox_telegram {
        let token = cancel.child_token();
        tasks.spawn(async move {
            runtime
                .run(token, interval)
                .await
                .map_err(|_| ServerError::Inbox)
        });
    }
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
    system.set_insight_enabled(insight_runtime.is_some());
    system.set_music_enabled(products.music.is_some());
    system.set_stats_enabled(products.stats.is_some());
    if let Some(telegram) = telegram {
        let telegram_cancel = cancel.clone();
        let _telegram_task = tasks.spawn(async move {
            telegram
                .run(telegram_cancel)
                .await
                .map_err(|_| ServerError::Telegram)
        });
        #[cfg(feature = "test-fixtures")]
        eprintln!(
            "fixture Telegram adapter task: task_id={}",
            _telegram_task.id()
        );
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
        &late_music_stats,
        telegram_handlers.as_ref(),
    )
    .await;
    drain_insight_preflight(&mut insight_preflight).await;
    if let Some(handlers) = &insight_handlers {
        handlers.drain_handlers().await;
        #[cfg(feature = "test-fixtures")]
        eprintln!("fixture Insight handlers drained");
    }
    if let Some(runtime) = &insight_runtime {
        runtime.shutdown().await;
    }
    #[cfg(feature = "test-fixtures")]
    if insight_runtime.is_some() {
        eprintln!("fixture concrete Insight shutdown completed");
    }
    let result = result.and(products.shutdown().await);
    let result = finish_database(result, database, config.shutdown_timeout).await;
    #[cfg(feature = "test-fixtures")]
    eprintln!("fixture database shutdown completed: result={result:?}");
    result
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
    http_products: &yoink_http::LateMusicStats,
    telegram_handlers: Option<&yoink_telegram::TelegramHandlerLifecycle>,
) -> Result<(), ServerError> {
    let initial = tokio::select! {
        signal = shutdown_signal => signal.map_err(|_| ServerError::Signal),
        completed = tasks.join_next() => Err(adapter_exit(completed)),
    };
    stop_and_drain(
        tasks,
        system,
        cancel,
        shutdown_timeout,
        initial,
        http_products,
        telegram_handlers,
    )
    .await
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
    http_products: &yoink_http::LateMusicStats,
    telegram_handlers: Option<&yoink_telegram::TelegramHandlerLifecycle>,
) -> Result<(), ServerError> {
    system.set_ready(false);
    system.set_bot_status(false, None);
    http_products.cancel_all();
    if let Some(handlers) = telegram_handlers {
        handlers.stop_admission();
    }
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
        if let Some(handlers) = telegram_handlers {
            handlers.cancel_all();
            handlers.drain_handlers().await;
        }
        http_products.drain().await;
        error.map_or(Ok(()), Err)
    })
    .await;
    let drained = match drain {
        Ok(result) => result,
        Err(_) => {
            http_products.cancel_all();
            if let Some(handlers) = telegram_handlers {
                handlers.cancel_all();
            }
            tasks.abort_all();
            while let Some(completed) = tasks.join_next_with_id().await {
                #[cfg(feature = "test-fixtures")]
                if let Err(error) = &completed
                    && error.is_cancelled()
                {
                    eprintln!("fixture adapter aborted and joined: task_id={}", error.id());
                }
                #[cfg(not(feature = "test-fixtures"))]
                let _ = completed;
            }
            if let Some(handlers) = telegram_handlers {
                handlers.drain_handlers().await;
            }
            http_products.drain().await;
            Err(ServerError::ShutdownTimeout)
        }
    };
    initial.and(drained)
}

async fn drain_insight_preflight(child: &mut Option<tokio::process::Child>) {
    if let Some(mut child) = child.take() {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
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
                async { Ok(()) },
                &yoink_http::LateMusicStats::default(),
                None,
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
                std::future::pending(),
                &yoink_http::LateMusicStats::default(),
                None,
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
                async { Ok(()) },
                &yoink_http::LateMusicStats::default(),
                None,
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
                std::future::pending(),
                &yoink_http::LateMusicStats::default(),
                None,
            )
            .await,
            Err(ServerError::Http)
        );
    }
}

#[cfg(test)]
#[path = "runtime_cookie_tests.rs"]
mod cookie_oauth_tests;

#[cfg(test)]
#[path = "runtime_http_tests.rs"]
mod http_lifecycle_tests;

#[cfg(test)]
mod insight_preflight_tests {
    use super::*;
    #[tokio::test]
    async fn interrupted_preflight_is_killed_and_reaped_before_teardown() {
        let mut child = Some(
            Command::new("/bin/sleep")
                .arg("30")
                .kill_on_drop(true)
                .spawn()
                .unwrap(),
        );
        let pid = child.as_ref().unwrap().id().unwrap();
        timeout(Duration::from_secs(1), drain_insight_preflight(&mut child))
            .await
            .unwrap();
        assert!(child.is_none());
        #[cfg(target_os = "linux")]
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        #[cfg(not(target_os = "linux"))]
        let _ = pid;
    }
}
