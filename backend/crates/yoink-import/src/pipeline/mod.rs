//! Private staging, one source snapshot, streamed conversion and atomic publication.
mod sequences;
mod table;
mod value;
mod verification;

use std::path::Path;

use futures_util::TryStreamExt;
use sha2::{Digest, Sha256};
use tokio_postgres::Client;
use turso::{
    Connection, Value,
    transaction::{Transaction, TransactionBehavior},
};
use yoink_db::raw_sql;
use yoink_storage::Store;

use crate::{
    ImportConfig, ImportError, ImportReport, Result, catalog, destination::DestinationGuard, source,
};
use table::{Column, Table, ident, insert_sql, parse_tables, projection};
use value::row_values;

struct ConnectionTask(tokio::task::JoinHandle<()>);
impl Drop for ConnectionTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub async fn execute(config: ImportConfig) -> Result<ImportReport> {
    config.validate()?;
    let mut destination = DestinationGuard::reserve(&config.destination)?;
    let staged_path = destination.staging_db();
    let (client, task) = source::connect(&config).await?;
    let mut task = ConnectionTask(task);
    let result = execute_connected(&config, &staged_path, &client).await;
    drop(client);
    task.0.abort();
    let _ = (&mut task.0).await;
    let report = result?;
    ensure_standalone(&staged_path)?;
    destination.publish(&staged_path)?;
    Ok(report)
}

async fn execute_connected(
    config: &ImportConfig,
    path: &Path,
    client: &Client,
) -> Result<ImportReport> {
    source::begin_snapshot(client).await?;
    source::configure_session(client, config).await?;
    catalog::lock_expected_tables(client).await?;
    let tables = parse_tables(&catalog::validate(client).await?)?;
    let store = Store::open(path, true, config.lock_timeout)
        .await
        .map_err(|_| ImportError::Storage)?;
    store.migrate().await.map_err(|_| ImportError::Storage)?;
    store
        .verify_schema()
        .await
        .map_err(|_| ImportError::Storage)?;
    let mut total = 0u64;
    {
        let mut connection = store
            .database()
            .connection()
            .await
            .map_err(|_| ImportError::Storage)?;
        foreign_keys(&connection, false).await?;
        let transaction = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| ImportError::Storage)?;
        let mut expected = Vec::with_capacity(tables.len());
        for table in &tables {
            let (count, digest) = copy_table(client, &transaction, table, config.max_row_bytes)
                .await
                .map_err(|error| error.at_table(&table.name, "copy"))?;
            total = total.checked_add(count).ok_or(ImportError::Conversion)?;
            expected.push((count, digest));
        }
        sequences::preserve(client, &transaction, &tables).await?;
        for (table, (count, digest)) in tables.iter().zip(&expected) {
            verification::verify_content(&transaction, table, digest, *count)
                .await
                .map_err(|error| error.at_table(&table.name, "content verification"))?;
        }
        verify_foreign_keys(&transaction, &tables).await?;
        raw_sql(include_str!("../../../yoink-inbox/sql/backfill_jobs.sql"))
            .execute(&*transaction)
            .await
            .map_err(|_| ImportError::Storage.at_table("inbox_jobs", "work admission"))?;
        transaction
            .commit()
            .await
            .map_err(|_| ImportError::Storage)?;
        foreign_keys(&connection, true).await?;
        verification::verify_integrity(&connection).await?;
    }
    // Store helpers acquire the connection mutex. Never call them while holding it.
    store
        .verify_schema()
        .await
        .map_err(|_| ImportError::Storage)?;
    {
        let connection = store
            .database()
            .connection()
            .await
            .map_err(|_| ImportError::Storage)?;
        verification::checkpoint(&connection).await?;
    }
    store.close().await.map_err(|_| ImportError::Storage)?;
    client
        .batch_execute("COMMIT")
        .await
        .map_err(|_| ImportError::Source)?;
    Ok(ImportReport {
        tables: tables.len(),
        rows: total,
    })
}

async fn copy_table(
    client: &Client,
    connection: &Connection,
    table: &Table,
    bound: usize,
) -> Result<(u64, [u8; 32])> {
    let stream = client
        .query_raw(
            &projection(table),
            std::iter::empty::<&(dyn tokio_postgres::types::ToSql + Sync)>(),
        )
        .await
        .map_err(|_| ImportError::Source)?;
    let mut stream = Box::pin(stream);
    let mut insert = connection
        .prepare(&insert_sql(table))
        .await
        .map_err(|_| ImportError::Storage)?;
    let mut count = 0u64;
    let mut hash = Sha256::new();
    while let Some(row) = stream.try_next().await.map_err(|_| ImportError::Source)? {
        let encoded: String = row.try_get(0).map_err(|_| ImportError::Conversion)?;
        let values = row_values(&encoded, table, bound)?;
        verification::digest_values(&mut hash, &values)?;
        insert
            .execute(values)
            .await
            .map_err(|_| ImportError::Storage)?;
        count = count.checked_add(1).ok_or(ImportError::Conversion)?;
    }
    Ok((count, hash.finalize().into()))
}

async fn foreign_keys(connection: &Connection, enabled: bool) -> Result<()> {
    connection
        .execute(
            if enabled {
                "PRAGMA foreign_keys=ON"
            } else {
                "PRAGMA foreign_keys=OFF"
            },
            (),
        )
        .await
        .map_err(|_| ImportError::EngineCapability)?;
    let mut rows = connection
        .query("PRAGMA foreign_keys", ())
        .await
        .map_err(|_| ImportError::EngineCapability)?;
    let row = rows
        .next()
        .await
        .map_err(|_| ImportError::EngineCapability)?
        .ok_or(ImportError::EngineCapability)?;
    if row
        .get_value(0)
        .map_err(|_| ImportError::EngineCapability)?
        != Value::Integer(i64::from(enabled))
    {
        return Err(ImportError::EngineCapability);
    }
    Ok(())
}

/// Explicit MATCH SIMPLE anti-joins also cover self references and nullable tuples.
async fn verify_foreign_keys(connection: &Connection, tables: &[Table]) -> Result<()> {
    for table in tables {
        for (children, parent, keys) in &table.foreign_keys {
            let equality = children
                .iter()
                .zip(keys)
                .map(|(child, key)| format!("p.{} = c.{}", ident(key), ident(child)))
                .collect::<Vec<_>>()
                .join(" AND ");
            let present = children
                .iter()
                .map(|child| format!("c.{} IS NOT NULL", ident(child)))
                .collect::<Vec<_>>()
                .join(" AND ");
            let sql = format!(
                "SELECT 1 FROM {} c WHERE {present} AND NOT EXISTS (SELECT 1 FROM {} p WHERE {equality}) LIMIT 1",
                ident(&table.name),
                ident(parent)
            );
            let mut rows = connection
                .query(&sql, ())
                .await
                .map_err(|_| ImportError::EngineCapability)?;
            if rows
                .next()
                .await
                .map_err(|_| ImportError::Storage)?
                .is_some()
            {
                return Err(
                    ImportError::Conversion.at_table(&table.name, "foreign-key verification")
                );
            }
        }
    }
    Ok(())
}

fn ensure_standalone(path: &Path) -> Result<()> {
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    match std::fs::metadata(wal) {
        Ok(metadata) if metadata.len() <= 32 => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(ImportError::Storage),
    }
}
