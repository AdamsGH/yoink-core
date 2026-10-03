mod auth_config;
mod config;
mod fixture_control;
mod media_config;
mod runtime;

use std::{
    future::Future,
    io::{self, Read},
    process::ExitCode,
    sync::Arc,
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
    let runtime_signal = async move {
        tokio::select! {
            result = signal => result,
            _ = controller_stopped.cancelled() => Ok(()),
        }
    };
    let runtime_result = runtime::run_with_store(config, runtime_signal, store.clone()).await;
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
    controller_result.map_err(Into::into)
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
fn termination_signal() -> io::Result<impl Future<Output = io::Result<()>>> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    Ok(async move {
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    })
}

#[cfg(not(unix))]
fn termination_signal() -> io::Result<impl Future<Output = io::Result<()>>> {
    Ok(tokio::signal::ctrl_c())
}
