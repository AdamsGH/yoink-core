//! Isolated native Turso evaluation. Not linked into the Yoink server.
use turso::transaction::{Transaction, TransactionBehavior};
use turso::{Connection, Database, Error, params};

const SCHEMA: &str = include_str!("../sql/schema.sql");
const CONNECTION_PRAGMAS: &str = include_str!("../sql/connection_pragmas.sql");
const SET_BUSY_TIMEOUT: &str = include_str!("../sql/set_busy_timeout.sql");
const ADMIT_ACTOR: &str = include_str!("../sql/admit_actor.sql");
const ADMIT_REQUEST: &str = include_str!("../sql/admit_request.sql");
const ADMIT_JOB: &str = include_str!("../sql/admit_job.sql");
const CANDIDATE: &str = include_str!("../sql/claim_candidate.sql");
const CLAIM: &str = include_str!("../sql/claim_job.sql");
const RECLAIM: &str = include_str!("../sql/reclaim_expired.sql");
const COMPLETE: &str = include_str!("../sql/complete_lease.sql");
const MARK_SENDING: &str = include_str!("../sql/mark_sending.sql");
const RECOVER_SENDING: &str = include_str!("../sql/recover_sending.sql");
const FK: &str = include_str!("../sql/pragma_fk.sql");
const SYNC: &str = include_str!("../sql/pragma_sync.sql");
const BUSY_TIMEOUT: &str = include_str!("../sql/pragma_busy_timeout.sql");
const COUNT: &str = include_str!("../sql/count.sql");
const GET_JOB: &str = include_str!("../sql/get_job.sql");
const VACUUM_INTO: &str = include_str!("../sql/vacuum_into.sql");

pub const BUSY_TIMEOUT_MS: i64 = 250;

async fn require_database_file(path: &str) -> Result<(), Error> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| Error::Misuse(format!("expected an existing database file: {error}")))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(Error::Misuse("expected a nonempty database file".into()));
    }
    Ok(())
}

pub async fn open(path: &str) -> Result<(Database, Connection), Error> {
    let db = turso::Builder::new_local(path).build().await?;
    let conn = db.connect()?;
    apply_connection_pragmas(&conn).await?;
    conn.execute_batch(SCHEMA).await?;
    Ok((db, conn))
}

pub async fn open_existing(path: &str) -> Result<(Database, Connection), Error> {
    require_database_file(path).await?;
    let db = turso::Builder::new_local(path).build().await?;
    let conn = db.connect()?;
    apply_connection_pragmas(&conn).await?;
    Ok((db, conn))
}

/// Fixture-only backup to a controlled path. Native 0.7.2 cannot bind the path
/// or correctly decode SQL-escaped apostrophes. Do not silently create a copy
/// under a different name, or allow a restore check to create an empty database.
pub async fn backup_into(conn: &Connection, path: &str) -> Result<(), Error> {
    if path.chars().any(|ch| matches!(ch, '\'' | '"' | '\0')) {
        return Err(Error::Misuse(
            "Turso 0.7.2 backup paths cannot contain quotes or NUL".into(),
        ));
    }
    let sql = VACUUM_INTO.replace("__BACKUP_PATH__", path);
    conn.execute(sql, ()).await?;
    require_database_file(path).await
}

pub async fn apply_connection_pragmas(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(SET_BUSY_TIMEOUT).await?;
    conn.execute_batch(CONNECTION_PRAGMAS).await?;
    Ok(())
}

pub async fn admit(
    conn: &mut Connection,
    actor: i64,
    key: &str,
    payload: &str,
) -> Result<(), Error> {
    let tx = Transaction::new(conn, TransactionBehavior::Immediate).await?;
    tx.execute(ADMIT_ACTOR, params![actor]).await?;
    tx.execute(ADMIT_REQUEST, params![actor, key]).await?;
    tx.execute(ADMIT_JOB, params![actor, key, payload]).await?;
    tx.commit().await
}

pub async fn claim(
    conn: &mut Connection,
    token: &str,
    now: i64,
    lease_ms: i64,
) -> Result<Option<i64>, Error> {
    let tx = Transaction::new(conn, TransactionBehavior::Immediate).await?;
    let mut rows = tx.query(CANDIDATE, ()).await?;
    let candidate = if let Some(row) = rows.next().await? {
        Some((row.get::<i64>(0)?, row.get::<i64>(1)?))
    } else {
        None
    };
    while rows.next().await?.is_some() {}
    if let Some((id, generation)) = candidate {
        tx.execute(CLAIM, params![id, generation + 1, token, now + lease_ms])
            .await?;
        tx.commit().await?;
        Ok(Some(id))
    } else {
        tx.commit().await?;
        Ok(None)
    }
}

pub async fn reclaim_expired(conn: &mut Connection, now: i64) -> Result<u64, Error> {
    let tx = Transaction::new(conn, TransactionBehavior::Immediate).await?;
    let changed = tx.execute(RECLAIM, params![now]).await?;
    tx.commit().await?;
    Ok(changed)
}

pub async fn complete(
    conn: &mut Connection,
    id: i64,
    generation: i64,
    token: &str,
    now: i64,
) -> Result<bool, Error> {
    let tx = Transaction::new(conn, TransactionBehavior::Immediate).await?;
    let changed = tx
        .execute(COMPLETE, params![id, generation, token, now])
        .await?;
    tx.commit().await?;
    Ok(changed == 1)
}

pub async fn mark_sending(
    conn: &mut Connection,
    id: i64,
    generation: i64,
    token: &str,
    now: i64,
) -> Result<bool, Error> {
    let tx = Transaction::new(conn, TransactionBehavior::Immediate).await?;
    let changed = tx
        .execute(MARK_SENDING, params![id, generation, token, now])
        .await?;
    tx.commit().await?;
    Ok(changed == 1)
}

pub async fn recover_sending(conn: &mut Connection) -> Result<u64, Error> {
    let tx = Transaction::new(conn, TransactionBehavior::Immediate).await?;
    let changed = tx.execute(RECOVER_SENDING, ()).await?;
    tx.commit().await?;
    Ok(changed)
}

pub async fn scalar(conn: &Connection, sql: &str) -> Result<i64, Error> {
    let mut rows = conn.query(sql, ()).await?;
    let row = rows.next().await?.expect("scalar query returned no row");
    let value = row.get::<i64>(0)?;
    while rows.next().await?.is_some() {}
    Ok(value)
}

#[derive(Debug, PartialEq, Eq)]
pub struct JobSnapshot {
    pub state: String,
    pub generation: i64,
    pub token: Option<String>,
    pub expires_at: Option<i64>,
}

pub async fn job(conn: &Connection, id: i64) -> Result<JobSnapshot, Error> {
    let mut rows = conn.query(GET_JOB, params![id]).await?;
    let row = rows.next().await?.expect("job missing");
    let state = row.get::<String>(0)?;
    let generation = row.get::<i64>(1)?;
    let token = row.get::<Option<String>>(2)?;
    let expires = row.get::<Option<i64>>(3)?;
    while rows.next().await?.is_some() {}
    Ok(JobSnapshot {
        state,
        generation,
        token,
        expires_at: expires,
    })
}

pub fn is_busy(error: &Error) -> bool {
    matches!(error, Error::Busy(_) | Error::BusySnapshot(_))
}

pub async fn queue_count(conn: &Connection, state: &str) -> Result<i64, Error> {
    let mut rows = conn.query(COUNT, params![state]).await?;
    let n = rows.next().await?.expect("count missing").get::<i64>(0)?;
    while rows.next().await?.is_some() {}
    Ok(n)
}

pub async fn pragmas(conn: &Connection) -> Result<(i64, i64, i64), Error> {
    Ok((
        scalar(conn, FK).await?,
        scalar(conn, SYNC).await?,
        scalar(conn, BUSY_TIMEOUT).await?,
    ))
}
