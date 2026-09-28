use std::{
    collections::BTreeMap,
    fs::File,
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
};

use tokio_util::sync::CancellationToken;
use yoink_db::publication::DestinationGuard;
use yoink_storage::Store;

use crate::{Error, Options, Report, Result, archive, check_cancel};

pub(crate) async fn backup(
    store: Store,
    destination: PathBuf,
    options: Options,
    cancel: CancellationToken,
) -> Result<Report> {
    check_cancel(&cancel)?;
    store.verify_schema().await.map_err(|_| Error::Schema)?;
    let guard = tokio::task::spawn_blocking(move || DestinationGuard::reserve(&destination))
        .await
        .map_err(|_| Error::Task)??;
    check_cancel(&cancel)?;
    let snapshot = guard.staging.path().join("snapshot.db");
    // The engine does not decode escaped apostrophes in VACUUM INTO. An owned,
    // held directory descriptor gives it a fixed ASCII path without user SQL text.
    let path = guard.staging.path().to_owned();
    let directory = tokio::task::spawn_blocking(move || {
        rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map(File::from)
        .map_err(|_| Error::Storage)
    })
    .await
    .map_err(|_| Error::Task)??;
    let sql = include_str!("../sql/snapshot.sql")
        .replace("{directory_fd}", &directory.as_raw_fd().to_string());
    {
        let conn = store
            .database()
            .connection()
            .await
            .map_err(|_| Error::Database)?;
        #[cfg(test)]
        if let Some(pause) = &options.pause {
            pause.reach(crate::tests::Stage::Snapshot).await;
        }
        check_cancel(&cancel)?;
        conn.execute_batch(&sql)
            .await
            .map_err(|_| Error::Snapshot)?;
    }
    drop(directory);
    store
        .database()
        .verify_files()
        .await
        .map_err(|_| Error::Database)?;
    check_cancel(&cancel)?;
    let permission_path = snapshot.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::set_permissions(permission_path, std::fs::Permissions::from_mode(0o600))
    })
    .await
    .map_err(|_| Error::Task)?
    .map_err(|_| Error::Storage)?;
    validate(&snapshot, &options).await?;
    #[cfg(test)]
    if let Some(pause) = &options.pause {
        pause.reach(crate::tests::Stage::Publish).await;
    }
    check_cancel(&cancel)?;
    tokio::task::spawn_blocking(move || {
        let mut guard = guard;
        let report = archive::encode(&snapshot, &guard.staging_db(), options.max_bytes, &cancel)?;
        check_cancel(&cancel)?;
        guard.publish(&guard.staging_db())?;
        Ok(report)
    })
    .await
    .map_err(|_| Error::Task)?
}

pub(crate) async fn restore(
    source: PathBuf,
    destination: PathBuf,
    options: Options,
    cancel: CancellationToken,
) -> Result<Report> {
    check_cancel(&cancel)?;
    let token = cancel.clone();
    let max_bytes = options.max_bytes;
    let (guard, report) = tokio::task::spawn_blocking(move || {
        let guard = DestinationGuard::reserve(&destination)?;
        let report = archive::decode(&source, &guard.staging_db(), max_bytes, &token)?;
        Ok::<_, Error>((guard, report))
    })
    .await
    .map_err(|_| Error::Task)??;
    check_cancel(&cancel)?;
    validate(&guard.staging_db(), &options).await?;
    #[cfg(test)]
    if let Some(pause) = &options.pause {
        pause.reach(crate::tests::Stage::Publish).await;
    }
    check_cancel(&cancel)?;
    tokio::task::spawn_blocking(move || {
        let mut guard = guard;
        check_cancel(&cancel)?;
        guard.publish(&guard.staging_db())?;
        Ok(report)
    })
    .await
    .map_err(|_| Error::Task)?
}

async fn validate(path: &Path, options: &Options) -> Result<()> {
    let store = Store::open(path, false, options.acquire_timeout)
        .await
        .map_err(|_| Error::Database)?;
    let result = async {
        store.verify_schema().await.map_err(|_| Error::Schema)?;
        let conn = store
            .database()
            .connection()
            .await
            .map_err(|_| Error::Database)?;
        let mut rows = conn
            .query(include_str!("../sql/integrity.sql"), ())
            .await
            .map_err(|_| Error::Integrity)?;
        let row = rows
            .next()
            .await
            .map_err(|_| Error::Integrity)?
            .ok_or(Error::Integrity)?;
        if row.get::<String>(0).map_err(|_| Error::Integrity)? != "ok"
            || rows.next().await.map_err(|_| Error::Integrity)?.is_some()
        {
            return Err(Error::Integrity);
        }
        foreign_keys(&conn).await?;
        let mut rows = conn
            .query(include_str!("../sql/checkpoint.sql"), ())
            .await
            .map_err(|_| Error::Checkpoint)?;
        let row = rows
            .next()
            .await
            .map_err(|_| Error::Checkpoint)?
            .ok_or(Error::Checkpoint)?;
        let busy = row.get::<i64>(0).map_err(|_| Error::Checkpoint)?;
        let log = row.get::<i64>(1).map_err(|_| Error::Checkpoint)?;
        let checkpointed = row.get::<i64>(2).map_err(|_| Error::Checkpoint)?;
        if busy != 0
            || log < 0
            || checkpointed != log
            || rows.next().await.map_err(|_| Error::Checkpoint)?.is_some()
        {
            return Err(Error::Checkpoint);
        }
        Ok(())
    }
    .await;
    let closed = store.close().await.map_err(|_| Error::Database);
    result?;
    closed?;
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    tokio::task::spawn_blocking(
        move || match std::fs::symlink_metadata(PathBuf::from(wal)) {
            Ok(meta) if meta.len() == 0 => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(Error::Checkpoint),
        },
    )
    .await
    .map_err(|_| Error::Task)?
}

fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

// Enumerate actual FK metadata rather than relying on an unknown/unsupported
// PRAGMA foreign_key_check silently returning no rows. MATCH SIMPLE skips a
// composite reference if any of its child columns is NULL.
async fn foreign_keys(conn: &turso::Connection) -> Result<()> {
    let mut rows = conn
        .query(include_str!("../sql/tables.sql"), ())
        .await
        .map_err(|_| Error::ForeignKey)?;
    let mut tables = Vec::new();
    while let Some(row) = rows.next().await.map_err(|_| Error::ForeignKey)? {
        tables.push(row.get::<String>(0).map_err(|_| Error::ForeignKey)?);
    }
    for table in tables {
        let mut rows = conn
            .query(&format!("PRAGMA foreign_key_list({})", ident(&table)), ())
            .await
            .map_err(|_| Error::ForeignKey)?;
        let mut keys: BTreeMap<i64, (String, Vec<(String, String)>)> = BTreeMap::new();
        while let Some(row) = rows.next().await.map_err(|_| Error::ForeignKey)? {
            let id = row.get::<i64>(0).map_err(|_| Error::ForeignKey)?;
            let parent = row.get::<String>(2).map_err(|_| Error::ForeignKey)?;
            let child = row.get::<String>(3).map_err(|_| Error::ForeignKey)?;
            let column = row.get::<String>(4).map_err(|_| Error::ForeignKey)?;
            keys.entry(id)
                .or_insert_with(|| (parent, Vec::new()))
                .1
                .push((child, column));
        }
        for (parent, columns) in keys.into_values() {
            let nonnull = columns
                .iter()
                .map(|(c, _)| format!("c.{} IS NOT NULL", ident(c)))
                .collect::<Vec<_>>()
                .join(" AND ");
            let equal = columns
                .iter()
                .map(|(c, p)| format!("p.{} = c.{}", ident(p), ident(c)))
                .collect::<Vec<_>>()
                .join(" AND ");
            let sql = format!(
                "SELECT 1 FROM {} c WHERE {nonnull} AND NOT EXISTS (SELECT 1 FROM {} p WHERE {equal}) LIMIT 1",
                ident(&table),
                ident(&parent)
            );
            let mut rows = conn.query(&sql, ()).await.map_err(|_| Error::ForeignKey)?;
            if rows.next().await.map_err(|_| Error::ForeignKey)?.is_some() {
                return Err(Error::ForeignKey);
            }
        }
    }
    Ok(())
}
