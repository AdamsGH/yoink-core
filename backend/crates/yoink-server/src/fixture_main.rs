mod auth_config;
mod config;
mod fixture_control;
mod fixture_cookie_providers;
mod inbox_config;
mod inbox_runtime;
mod media_config;
mod runtime;
mod user_session_config;

use std::{
    collections::BTreeSet,
    future::Future,
    io::{self, Read},
    path::Path,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> ExitCode {
    if tracing_subscriber::fmt()
        .with_env_filter(
            "off,yoink_fixture_server=info,yoink_telegram=info,yoink_downloads=info,yoink_db=warn",
        )
        .with_target(false)
        .try_init()
        .is_err()
    {
        eprintln!("could not initialize logging");
        return ExitCode::FAILURE;
    }
    match execute().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

async fn execute() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => serve().await,
        [command] if command == "serve" => serve().await,
        [command] if command == "sql" => sql().await,
        [arg] if matches!(arg.as_str(), "--help" | "-h") => {
            println!("Usage: yoink-fixture-server [serve|sql|--help]");
            Ok(())
        }
        _ => Err("expected serve, sql or --help".into()),
    }
}

struct NotifyOnDrop(CancellationToken);

impl Drop for NotifyOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

async fn open_store()
-> Result<(config::Config, Arc<yoink_storage::Store>), Box<dyn std::error::Error>> {
    let config = config::Config::from_env()?;
    let database = config
        .database
        .as_ref()
        .ok_or("YOINK_RUST_DATABASE_PATH is required")?;
    let store = Arc::new(
        tokio::time::timeout(config.startup_timeout, database.open(false))
            .await
            .map_err(|_| "database open deadline exceeded")??,
    );
    if !fixture_control::configured_path(&database.path, &store) {
        let _ = store.close().await;
        return Err("configured database path mismatch".into());
    }
    match tokio::time::timeout(config.startup_timeout, store.verify_schema()).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            let _ = store.close().await;
            return Err("database schema check failed".into());
        }
        Err(_) => {
            let _ = store.close().await;
            return Err("database schema check deadline exceeded".into());
        }
    }
    Ok((config, store))
}

async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    let token = fixture_control::token_from_env()
        .map_err(|_| "invalid fixture controller configuration")?;
    let listener = fixture_control::bind_from_env()
        .await
        .map_err(|_| "fixture controller bind failed")?;
    let signal = termination_signal().map_err(|_| "signal initialization failed")?;
    let (config, store) = open_store().await?;
    // This opt-in belongs exclusively to the fixture entry point. Production
    // startup neither reads the switch nor imports the offline providers.
    let cookie_providers = if std::env::var("YOINK_FIXTURE_COOKIE_PROVIDERS").as_deref() == Ok("1")
    {
        match fixture_cookie_providers::install(store.clone()).await {
            Ok(providers) => Some(providers),
            Err(_) => {
                let _ = store.close().await;
                return Err("fixture cookie provider initialization failed".into());
            }
        }
    } else {
        None
    };
    let address = listener
        .local_addr()
        .map_err(|_| "fixture controller address failed")?;
    eprintln!("fixture controller listening on {address}");
    let control_shutdown = CancellationToken::new();
    let controller_stopped = CancellationToken::new();
    let task_shutdown = control_shutdown.clone();
    let stopped = controller_stopped.clone();
    let task_store = store.clone();
    let mut controller = tokio::spawn(async move {
        let _stopped = NotifyOnDrop(stopped);
        fixture_control::serve(listener, task_store, token, task_shutdown).await
    });
    let workspace = config
        .media
        .as_ref()
        .map(|media| media.executor.workspace_root.clone());
    let signal_workspace = workspace.clone();
    let signaled = Arc::new(AtomicBool::new(false));
    let signal_seen = signaled.clone();
    let runtime_signal = async move {
        tokio::select! {
            result = signal => {
                let kind = result?;
                // Snapshot at the actual signal boundary, before runtime cancellation.
                // This is fixture-only, not a public production diagnostics API.
                let (children, workspaces) = cleanup_snapshot(signal_workspace.as_deref(), Path::new("/proc/self/task"))?;
                eprintln!("fixture shutdown signal {kind}: worker_children={children}, workspaces={workspaces}");
                signal_seen.store(true, Ordering::SeqCst);
                Ok(())
            },
            _ = controller_stopped.cancelled() => Ok(()),
        }
    };
    let runtime_result = match cookie_providers {
        Some((oauth, account, refresh)) => {
            runtime::run_with_store_and_cookie_providers(
                config,
                runtime_signal,
                store.clone(),
                Some(runtime::CookieProviderOverrides {
                    oauth,
                    refresh,
                    account,
                }),
            )
            .await
        }
        None => runtime::run_with_store(config, runtime_signal, store.clone()).await,
    };
    control_shutdown.cancel();
    let controller_result =
        match tokio::time::timeout(Duration::from_secs(7), &mut controller).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(_))) => Err("fixture controller failed"),
            Ok(Err(_)) => Err("fixture controller task failed"),
            Err(_) => {
                controller.abort();
                let _ = controller.await;
                Err("fixture controller shutdown timed out")
            }
        };
    let close_result = tokio::time::timeout(Duration::from_secs(7), store.close()).await;
    if !matches!(close_result, Ok(Ok(()))) {
        return Err("fixture database shutdown failed".into());
    }
    if let Err(error) = runtime_result {
        return Err(error.into());
    }
    controller_result?;
    if signaled.load(Ordering::SeqCst) {
        // run_with_store returned successfully only after all adapter tasks drained.
        // Check the original process and workspace before exit destroys its tmpfs.
        let (children, workspaces) =
            cleanup_snapshot(workspace.as_deref(), Path::new("/proc/self/task"))?;
        if children != 0 || workspaces != 0 {
            return Err("fixture graceful shutdown left worker children or workspaces".into());
        }
        eprintln!(
            "fixture graceful shutdown verified: adapters drained, worker_children=0, workspaces=0"
        );
    }
    Ok(())
}

async fn sql() -> Result<(), Box<dyn std::error::Error>> {
    let token = fixture_control::token_from_env()
        .map_err(|_| "invalid fixture controller configuration")?;
    let (config, store) = open_store().await?;
    let mut input = Vec::new();
    std::io::Read::take(std::io::stdin(), 1024 * 1024 + 1).read_to_end(&mut input)?;
    if input.len() > 1024 * 1024 {
        let _ = store.close().await;
        return Err("request exceeded limit".into());
    }
    let response = fixture_control::offline_request(&store, &token, &input).await;
    let output = serde_json::to_vec(&response)?;
    if output.len() > 1024 * 1024 {
        store.close().await?;
        return Err("fixture response exceeded limit".into());
    }
    use std::io::Write;
    std::io::stdout().write_all(&output)?;
    std::io::stdout().write_all(b"\n")?;
    tokio::time::timeout(config.shutdown_timeout, store.close())
        .await
        .map_err(|_| "database shutdown timed out")??;
    Ok(())
}

#[cfg(unix)]
fn termination_signal() -> io::Result<impl Future<Output = io::Result<&'static str>>> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    Ok(async move {
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.map(|()| "SIGINT"),
            _ = terminate.recv() => Ok("SIGTERM"),
        }
    })
}

#[cfg(not(unix))]
fn termination_signal() -> io::Result<impl Future<Output = io::Result<&'static str>>> {
    Ok(async { tokio::signal::ctrl_c().await.map(|()| "SIGINT") })
}

// /proc children covers every executor thread and reparented children when the
// fixture is PID 1. No process is inferred from a replacement container.
fn cleanup_snapshot(workspace: Option<&Path>, tasks: &Path) -> io::Result<(usize, usize)> {
    let mut children = BTreeSet::new();
    for task in std::fs::read_dir(tasks)? {
        let path = task?.path().join("children");
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue, // exiting thread
            Err(error) => return Err(error),
        };
        for pid in text.split_whitespace() {
            children.insert(
                pid.parse::<u32>()
                    .map_err(|_| io::Error::other("invalid fixture child PID"))?,
            );
        }
    }
    let workspaces = match workspace {
        Some(path) => std::fs::read_dir(path)?
            .collect::<Result<Vec<_>, _>>()?
            .len(),
        None => 0,
    };
    Ok((children.len(), workspaces))
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    #[test]
    fn original_workspace_and_all_thread_children_are_observed_before_exit() {
        let root =
            std::env::temp_dir().join(format!("yoink-shutdown-proof-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let work = root.join("work");
        let tasks = root.join("tasks");
        std::fs::create_dir(&work).unwrap();
        std::fs::create_dir(&tasks).unwrap();
        for thread in ["1", "2"] {
            std::fs::create_dir(tasks.join(thread)).unwrap();
        }
        std::fs::write(tasks.join("1/children"), "17 18").unwrap();
        std::fs::write(tasks.join("2/children"), "18 19").unwrap();
        std::fs::create_dir(work.join("held-worker")).unwrap();
        assert_eq!(cleanup_snapshot(Some(&work), &tasks).unwrap(), (3, 1));
        std::fs::remove_dir(work.join("held-worker")).unwrap();
        assert_eq!(
            cleanup_snapshot(Some(&work), &tasks).unwrap(),
            (3, 0),
            "empty workspace alone is not process cleanup"
        );
        for thread in ["1", "2"] {
            std::fs::write(tasks.join(thread).join("children"), "").unwrap();
        }
        assert_eq!(cleanup_snapshot(Some(&work), &tasks).unwrap(), (0, 0));
        assert!(cleanup_snapshot(Some(&root.join("missing")), &tasks).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
