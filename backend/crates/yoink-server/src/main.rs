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
    let config = config::Config::from_env()?;
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
