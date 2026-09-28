mod endpoint;
mod tls;

use crate::{ImportConfig, ImportError, Result};
use std::{future::Future, str::FromStr};
use tokio_postgres::{Client, Config, NoTls, config::SslMode};

pub async fn connect(config: &ImportConfig) -> Result<(Client, tokio::task::JoinHandle<()>)> {
    let mut pg = Config::from_str(&config.source_url).map_err(|_| ImportError::Configuration)?;
    if (pg.get_hosts().is_empty() && pg.get_hostaddrs().is_empty())
        || pg.get_user().is_none()
        || pg.get_dbname().is_none()
    {
        return Err(ImportError::Configuration);
    }
    pg.application_name("yoink-native-import");
    pg.connect_timeout(config.connect_timeout);
    let operation = async {
        if config.allow_loopback_plaintext {
            if config.ca_cert.is_some() || !endpoint::is_local(&pg) {
                return Err(ImportError::PlaintextEndpoint);
            }
            pg.ssl_mode(SslMode::Disable);
            let (client, connection) = pg.connect(NoTls).await.map_err(|_| ImportError::Source)?;
            Ok((client, drive(connection)))
        } else {
            pg.ssl_mode(SslMode::Require);
            let ca = config.ca_cert.clone();
            let connector = tokio::task::spawn_blocking(move || tls::connector(ca.as_deref()))
                .await
                .map_err(|_| ImportError::Tls)??;
            let (client, connection) = pg.connect(connector).await.map_err(|_| ImportError::Tls)?;
            Ok((client, drive(connection)))
        }
    };
    tokio::time::timeout(config.connect_timeout, operation)
        .await
        .map_err(|_| ImportError::Deadline)?
}

fn drive(
    connection: impl Future<Output = std::result::Result<(), tokio_postgres::Error>> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let _ = connection.await;
    })
}

/// Establish the snapshot before waiting for any application-table lock.
pub async fn begin_snapshot(client: &Client) -> Result<String> {
    client
        .batch_execute(include_str!("../sql/source-begin.sql"))
        .await
        .map_err(|_| ImportError::Source)?;
    client
        .query_one(include_str!("../sql/source-snapshot.sql"), &[])
        .await
        .map_err(|_| ImportError::Source)?
        .try_get(0)
        .map_err(|_| ImportError::Source)
}

pub async fn configure_session(client: &Client, config: &ImportConfig) -> Result<()> {
    let lock = format!("{}ms", config.lock_timeout.as_millis());
    let statement = format!("{}ms", config.statement_timeout.as_millis());
    client
        .query_one(
            include_str!("../sql/source-session.sql"),
            &[&lock, &statement],
        )
        .await
        .map_err(|_| ImportError::Source)?;
    Ok(())
}
