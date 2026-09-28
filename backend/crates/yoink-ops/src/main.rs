use std::{collections::BTreeMap, ffi::OsString, path::PathBuf, process::ExitCode, time::Duration};
use tokio_util::sync::CancellationToken;
use yoink_ops::{Error, Options, Report};
use yoink_storage::Store;

const HELP: &str = "yoink-ops backup|restore --source PATH --destination NEW_PATH [--max-bytes N] [--timeout-seconds N]\n\nBackup requires exclusive process ownership of the source DB. Restore accepts a trusted Yoink backup, never raw DB/WAL files. The destination parent must be owned and private (0700), with no existing destination or sidecars. Default payload limit is 64 GiB. The default 300-second deadline requests cooperative cancellation; current native phases finish before cleanup. No deployment settings are changed. Archives contain plaintext credentials: protect them like the database.";

struct Args {
    action: String,
    source: PathBuf,
    destination: PathBuf,
    options: Options,
    deadline: Duration,
}
fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Option<Args>, &'static str> {
    let Some(action) = args.next() else {
        return Err("missing backup or restore action");
    };
    if action == "--help" || action == "-h" {
        return Ok(None);
    }
    let action = action.into_string().map_err(|_| "invalid action")?;
    if action != "backup" && action != "restore" {
        return Err("expected backup or restore");
    }
    let mut fields = BTreeMap::new();
    while let Some(flag) = args.next() {
        let flag = flag.into_string().map_err(|_| "invalid option")?;
        if !matches!(
            flag.as_str(),
            "--source" | "--destination" | "--max-bytes" | "--timeout-seconds"
        ) {
            return Err("unknown option");
        }
        let value = args.next().ok_or("missing option value")?;
        if value.is_empty() || fields.insert(flag, value).is_some() {
            return Err("empty or repeated option");
        }
    }
    let source = fields
        .remove("--source")
        .ok_or("--source is required")?
        .into();
    let destination = fields
        .remove("--destination")
        .ok_or("--destination is required")?
        .into();
    let mut options = Options::default();
    if let Some(value) = fields.remove("--max-bytes") {
        options.max_bytes = number(value)?;
    }
    let deadline = if let Some(value) = fields.remove("--timeout-seconds") {
        number(value)?
    } else {
        300
    };
    if deadline > 86400 {
        return Err("timeout must be at most 86400 seconds");
    }
    Ok(Some(Args {
        action,
        source,
        destination,
        options,
        deadline: Duration::from_secs(deadline),
    }))
}
fn number(value: OsString) -> Result<u64, &'static str> {
    value
        .to_str()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .ok_or("numeric options must be positive integers")
}
async fn operate(args: &Args, cancel: CancellationToken) -> Result<Report, Error> {
    if args.action == "backup" {
        let store = Store::open(&args.source, false, args.options.acquire_timeout)
            .await
            .map_err(|_| Error::Database)?;
        yoink_ops::backup(
            store,
            args.destination.clone(),
            args.options.clone(),
            cancel,
        )
        .await
    } else {
        yoink_ops::restore(
            args.source.clone(),
            args.destination.clone(),
            args.options.clone(),
            cancel,
        )
        .await
    }
}
#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse(std::env::args_os().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("{message}\n{HELP}");
            return ExitCode::from(2);
        }
    };
    let mut term = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(signal) => signal,
        Err(_) => {
            eprintln!("cannot register termination handler");
            return ExitCode::FAILURE;
        }
    };
    let cancel = CancellationToken::new();
    let operation = operate(&args, cancel.clone());
    tokio::pin!(operation);
    let result = tokio::select! {
        result=&mut operation=>result,
        _=tokio::signal::ctrl_c()=>{cancel.cancel();operation.await},
        _=term.recv()=>{cancel.cancel();operation.await},
        _=tokio::time::sleep(args.deadline)=>{cancel.cancel();operation.await},
    };
    match result {
        Ok(report) => {
            println!(
                "{}",
                serde_json::json!({"operation":args.action,"bytes":report.bytes,"sha256":report.sha256})
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            if matches!(error, Error::Cancelled) {
                ExitCode::from(130)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn arguments(parts: &[&str]) -> impl Iterator<Item = OsString> {
        parts.iter().map(OsString::from)
    }
    #[test]
    fn strict_arguments_allow_literal_paths_not_ambiguous_options() {
        let args = parse(arguments(&[
            "backup",
            "--source",
            "a ' ; --",
            "--destination",
            "b",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(args.source, PathBuf::from("a ' ; --"));
        for parts in [
            vec!["backup"],
            vec![
                "backup",
                "--source",
                "a",
                "--source",
                "b",
                "--destination",
                "c",
            ],
            vec![
                "restore",
                "--source",
                "a",
                "--destination",
                "b",
                "--max-bytes",
                "0",
            ],
            vec![
                "restore",
                "--source",
                "a",
                "--destination",
                "b",
                "--timeout-seconds",
                "86401",
            ],
        ] {
            assert!(parse(arguments(&parts)).is_err());
        }
    }
}
