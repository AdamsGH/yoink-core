mod config;
mod runtime;

use std::{future::Future, io, process::ExitCode};

#[tokio::main]
async fn main() -> ExitCode {
    // Restrict logs to our own sanitized events. HTTP client diagnostics can
    // otherwise contain Telegram credentials embedded in request URLs.
    if tracing_subscriber::fmt()
        .with_env_filter("off,yoink_server=info,yoink_telegram=info")
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
    let command = match args.as_slice() {
        [] => "serve",
        [arg] if matches!(arg.as_str(), "serve" | "migrate" | "check-schema") => arg.as_str(),
        [arg] if matches!(arg.as_str(), "--help" | "-h") => {
            println!(
                "Usage: yoink-server [serve|migrate|check-schema]\nDatabase commands require YOINK_RUST_DATABASE_URL. Normal startup never migrates."
            );
            return Ok(());
        }
        _ => return Err("expected serve, migrate, check-schema or --help".into()),
    };
    let config = config::Config::from_env()?;
    if command != "serve" {
        let database = config
            .database
            .as_ref()
            .ok_or("YOINK_RUST_DATABASE_URL is required for database commands")?;
        let store = tokio::time::timeout(config.startup_timeout, database.connect())
            .await
            .map_err(|_| "database connection deadline exceeded")??;
        let result = tokio::time::timeout(config.startup_timeout, async {
            if command == "migrate" {
                store.migrate().await
            } else {
                store.verify_schema().await
            }
        })
        .await;
        tokio::time::timeout(config.shutdown_timeout, store.close())
            .await
            .map_err(|_| "database shutdown deadline exceeded")?;
        result.map_err(|_| "database command deadline exceeded")??;
        println!("database schema ready");
        return Ok(());
    }
    let shutdown = termination_signal()?;
    runtime::run(config, shutdown).await?;
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
