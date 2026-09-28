use yoink_import::{ImportError, config::ImportConfig, import};

const HELP: &str = "yoink-import --destination NEW_DATABASE [options]

Source: explicitly supply YOINK_IMPORT_SOURCE_URL. Never pass its password in argv.
Default: verified TLS, a read-only repeatable-read source and no-replace publication.

Options:
  --ca-cert PATH                     PEM trust store instead of system roots
  --allow-loopback-plaintext          Numeric loopback or absolute Unix sockets only
  --connect-timeout-seconds N         Default 10, maximum 300
  --lock-timeout-seconds N            Default 15, maximum 300
  --statement-timeout-seconds N       Default 60, maximum 3600
  --total-timeout-seconds N           Default 3600, maximum 86400
  --max-row-bytes N                   Default 16777216, maximum 268435456

The destination must be absent, in an existing owner-only writable directory.
The importer does not read .env or change the running deployment.";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        println!("{HELP}");
        return;
    }
    let result = async {
        let config = ImportConfig::from_cli_args(args)?;
        #[cfg(unix)]
        {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .map_err(|_| ImportError::Configuration)?;
            tokio::select! {
                result = import(config) => result,
                _ = tokio::signal::ctrl_c() => Err(ImportError::Interrupted),
                _ = terminate.recv() => Err(ImportError::Interrupted),
            }
        }
        #[cfg(not(unix))]
        {
            import(config).await
        }
    }
    .await;
    match result {
        Ok(report) => println!(
            "import complete: {} tables, {} rows",
            report.tables, report.rows
        ),
        Err(error) => {
            eprintln!("import failed: {error}");
            std::process::exit(1);
        }
    }
}
