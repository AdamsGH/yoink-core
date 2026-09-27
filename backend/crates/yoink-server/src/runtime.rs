use std::{future::Future, io, time::Duration};

use thiserror::Error;
use tokio::{net::TcpListener, task::JoinSet, time::timeout};
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
}

pub async fn run(
    config: Config,
    shutdown_signal: impl Future<Output = io::Result<()>>,
) -> Result<(), ServerError> {
    let system = SystemService::new(env!("CARGO_PKG_VERSION"));
    let cancel = CancellationToken::new();
    let listener = TcpListener::bind(config.http_bind)
        .await
        .map_err(|_| ServerError::Bind)?;
    let address = listener.local_addr().map_err(|_| ServerError::Bind)?;
    tokio::pin!(shutdown_signal);

    // Serve diagnostics immediately with readiness false, including while an
    // external dependency is slow. The same drain path owns startup failures.
    let mut tasks = JoinSet::new();
    let http_system = system.clone();
    let http_cancel = cancel.clone();
    tasks.spawn(async move {
        yoink_http::serve(listener, http_system, http_cancel)
            .await
            .map_err(|_| ServerError::Http)
    });
    tracing::info!(%address, "HTTP diagnostics listening");
    let database_timings = config
        .database
        .as_ref()
        .map(|db| (db.health_interval, db.acquire_timeout));
    let mut database = None;
    let prepare = async {
        if let Some(database_config) = &config.database {
            database = Some(
                database_config
                    .connect()
                    .await
                    .map_err(|_| ServerError::DatabaseStartup)?,
            );
            database
                .as_ref()
                .expect("database initialized")
                .verify_schema()
                .await
                .map_err(|_| ServerError::DatabaseStartup)?;
        }
        match config.telegram {
            Some(telegram) => TelegramAdapter::prepare(
                telegram.token.into_inner(),
                telegram.api_url,
                telegram.owner_id,
                system.clone(),
                telegram.poll_timeout,
            )
            .await
            .map(Some)
            .map_err(|_| ServerError::TelegramStartup),
            None => Ok(None),
        }
    };
    enum Startup {
        Prepared(Result<Option<TelegramAdapter>, ServerError>),
        Stopped(Result<(), ServerError>),
    }
    let startup = tokio::select! {
        result = timeout(config.startup_timeout, prepare) => Startup::Prepared(result.unwrap_or(Err(ServerError::StartupTimeout))),
        signal = &mut shutdown_signal => Startup::Stopped(signal.map_err(|_| ServerError::Signal)),
        completed = tasks.join_next() => Startup::Stopped(Err(adapter_exit(completed))),
    };
    if let Some(database) = database {
        let database_cancel = cancel.clone();
        let (health_interval, health_timeout) =
            database_timings.expect("configured database timings");
        tasks.spawn(async move {
            let result = loop {
                tokio::select! {
                    biased;
                    _ = database_cancel.cancelled() => break Ok(()),
                    _ = tokio::time::sleep(health_interval) => {
                        let check = tokio::select! {
                            biased;
                            _ = database_cancel.cancelled() => break Ok(()),
                            check = timeout(health_timeout, database.verify_schema()) => check,
                        };
                        if !matches!(check, Ok(Ok(()))) {
                            break Err(ServerError::DatabaseUnavailable);
                        }
                    }
                }
            };
            database.close().await;
            result
        });
    }
    let telegram = match startup {
        Startup::Prepared(Ok(telegram)) => telegram,
        Startup::Prepared(Err(error)) => {
            return stop_and_drain(tasks, system, cancel, config.shutdown_timeout, Err(error))
                .await;
        }
        Startup::Stopped(result) => {
            return stop_and_drain(tasks, system, cancel, config.shutdown_timeout, result).await;
        }
    };
    let telegram_enabled = telegram.is_some();
    if let Some(telegram) = telegram {
        let telegram_cancel = cancel.clone();
        tasks.spawn(async move {
            telegram
                .run(telegram_cancel)
                .await
                .map_err(|_| ServerError::Telegram)
        });
    }
    system.set_ready(true);
    tracing::info!(%address, telegram_enabled, "diagnostic adapters ready; business modules remain planned");
    supervise(
        tasks,
        system,
        cancel,
        config.shutdown_timeout,
        shutdown_signal,
    )
    .await
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
