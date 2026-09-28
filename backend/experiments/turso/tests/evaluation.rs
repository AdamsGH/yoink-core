use std::collections::HashSet;
use std::os::unix::process::ExitStatusExt;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::process::Command;
use tokio::sync::{Barrier, oneshot};
use tokio::time::{Instant, sleep, timeout};
use turso::transaction::{Transaction, TransactionBehavior};
use turso::{Error, params};
use yoink_turso_probe::{
    BUSY_TIMEOUT_MS, admit, apply_connection_pragmas, backup_into, claim, complete, is_busy, job,
    mark_sending, open, open_existing, pragmas, queue_count, reclaim_expired, recover_sending,
};

const ROLLBACK_PROBE: &str = include_str!("../sql/rollback_probe.sql");
const CANCELLED_ROLLBACK_PROBE: &str = include_str!("../sql/rollback_probe_two.sql");
const INSERT_ACTOR_STRICT: &str = include_str!("../sql/insert_actor_strict.sql");
const INSERT_DUPLICATE_REQUEST: &str = include_str!("../sql/insert_duplicate_request.sql");
const JOB_PAYLOAD: &str = include_str!("../sql/job_payload.sql");
const JOB_SCHEMA: &str = include_str!("../sql/job_schema.sql");
const REQUEST_COUNT: &str = include_str!("../sql/request_count.sql");
const COUNT_PAYLOAD: &str = include_str!("../sql/count_snapshot_payload.sql");
const COUNT_KEPT_PAYLOAD: &str = include_str!("../sql/count_kept_payload.sql");

fn path(dir: &TempDir) -> String {
    dir.path().join("queue.db").to_string_lossy().into_owned()
}

async fn prepared() -> (TempDir, turso::Database, turso::Connection) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let (db, conn) = open(&path(&dir)).await.expect("open fixture database");
    (dir, db, conn)
}

async fn scalar(conn: &turso::Connection, sql: &str) -> Result<i64, Error> {
    let mut rows = conn.query(sql, ()).await?;
    let value = rows.next().await?.expect("scalar row").get::<i64>(0)?;
    assert!(rows.next().await?.is_none());
    Ok(value)
}

async fn fk_delete_probe(
    action: &str,
    inline: bool,
    timing: &str,
    layout: &str,
) -> Result<(), Error> {
    let dir = tempfile::tempdir().unwrap();
    let db_path = path(&dir);
    let (_db, mut conn) = open(&db_path).await.unwrap();
    conn.execute("CREATE TABLE parent (id INTEGER PRIMARY KEY)", ())
        .await
        .unwrap();
    conn.execute("INSERT INTO parent VALUES (1)", ())
        .await
        .unwrap();

    let column = match layout {
        "int-key" => "id INT PRIMARY KEY NOT NULL",
        "ordinary" => "id INTEGER NOT NULL",
        _ => "id INTEGER PRIMARY KEY",
    };
    let child = if inline {
        format!("CREATE TABLE pin ({column} REFERENCES parent(id) ON DELETE {action})")
    } else {
        format!(
            "CREATE TABLE pin ({column}, FOREIGN KEY(id) REFERENCES parent(id) ON DELETE {action})"
        )
    };
    if timing != "post-query" {
        conn.execute(&child, ()).await.unwrap();
        conn.execute("INSERT INTO pin VALUES (1)", ())
            .await
            .unwrap();
    }
    if timing != "fresh" {
        assert_eq!(
            scalar(&conn, "SELECT count(*) FROM parent").await.unwrap(),
            1
        );
    }
    if timing == "post-query" {
        conn.execute(&child, ()).await.unwrap();
        conn.execute("INSERT INTO pin VALUES (1)", ())
            .await
            .unwrap();
    }
    if timing == "reopened" {
        drop(conn);
        let (_db, reopened) = open_existing(&db_path).await.unwrap();
        conn = reopened;
    }

    if layout == "indexed-rowid" {
        conn.execute("CREATE INDEX pin_id_fk ON pin(id)", ())
            .await
            .unwrap();
    }
    assert_eq!(scalar(&conn, "PRAGMA foreign_keys").await.unwrap(), 1);
    let mut foreign_keys = conn.query("PRAGMA foreign_key_list('pin')", ()).await?;
    assert!(foreign_keys.next().await?.is_some(), "expected declared FK");
    assert!(
        foreign_keys.next().await?.is_none(),
        "expected exactly one FK"
    );
    let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate).await?;
    let result = tx.execute("DELETE FROM parent WHERE id = 1", ()).await;
    tx.rollback().await?;
    assert_eq!(scalar(&conn, "SELECT count(*) FROM parent").await?, 1);
    assert_eq!(scalar(&conn, "SELECT count(*) FROM pin").await?, 1);
    result.map(|_| ())
}

#[tokio::test]
async fn rowid_reference_delete_requires_explicit_child_index() {
    let mut unexpected = Vec::new();
    for action in ["RESTRICT", "NO ACTION"] {
        for inline in [false, true] {
            for timing in ["fresh", "post-query", "reopened"] {
                for layout in ["rowid", "indexed-rowid", "int-key", "ordinary"] {
                    let result = fk_delete_probe(action, inline, timing, layout).await;
                    println!(
                        "ON DELETE {action}, inline={inline}, timing={timing}, layout={layout}: {result:?}"
                    );
                    let expected = if layout == "rowid" {
                        result.is_ok()
                    } else {
                        matches!(&result, Err(Error::Constraint(_)))
                    };
                    if !expected {
                        unexpected.push(format!("ON DELETE {action}, inline={inline}, timing={timing}, layout={layout}: {result:?}"));
                    }
                }
            }
        }
    }
    assert!(
        unexpected.is_empty(),
        "unexpected rowid FK matrix outcomes: {unexpected:#?}"
    );
}

async fn admit_bounded(conn: &mut turso::Connection, payload: &str) {
    let mut delay = 2;
    for attempt in 0..40 {
        match admit(conn, 20, "concurrent-key", payload).await {
            Ok(()) => return,
            Err(error) if is_busy(&error) => {
                if attempt == 39 {
                    panic!("typed busy retry budget exhausted during admission: {error:?}");
                }
                sleep(Duration::from_millis(delay)).await;
                delay = (delay * 2).min(20);
            }
            Err(error) => panic!("non-retryable admission error: {error:?}"),
        }
    }
    unreachable!()
}

async fn claim_bounded(conn: &mut turso::Connection, token: &str, now: i64) -> Option<i64> {
    let mut delay = 2;
    for attempt in 0..40 {
        match claim(conn, token, now, 60_000).await {
            Ok(id) => return id,
            Err(error) if is_busy(&error) => {
                if attempt == 39 {
                    panic!("typed busy retry budget exhausted: {error:?}");
                }
                sleep(Duration::from_millis(delay)).await;
                delay = (delay * 2).min(20);
            }
            Err(error) => panic!("non-retryable claim error: {error:?}"),
        }
    }
    unreachable!()
}

#[tokio::test]
async fn admission_is_unique_by_actor_and_request_key() {
    let (_dir, _db, mut conn) = prepared().await;
    admit(&mut conn, 10, "same", "first").await.unwrap();
    admit(&mut conn, 10, "same", "duplicate payload")
        .await
        .unwrap();
    admit(&mut conn, 11, "same", "different actor")
        .await
        .unwrap();
    assert_eq!(queue_count(&conn, "queued").await.unwrap(), 2);
    let mut rows = conn.query(JOB_PAYLOAD, params![1_i64]).await.unwrap();
    assert_eq!(
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>(0)
            .unwrap(),
        "first"
    );
    while rows.next().await.unwrap().is_some() {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_same_key_admission_is_idempotent() {
    let (_dir, db, _setup) = prepared().await;
    let mut connections = Vec::new();
    for _ in 0..4 {
        let conn = db.connect().unwrap();
        apply_connection_pragmas(&conn).await.unwrap();
        connections.push(conn);
    }
    let start = Arc::new(Barrier::new(5));
    let mut workers = Vec::new();
    for (index, mut conn) in connections.into_iter().enumerate() {
        let start = Arc::clone(&start);
        workers.push(tokio::spawn(async move {
            start.wait().await;
            admit_bounded(&mut conn, &format!("payload-{index}")).await;
        }));
    }
    start.wait().await;
    for worker in workers {
        worker.await.unwrap();
    }
    let check = db.connect().unwrap();
    assert_eq!(queue_count(&check, "queued").await.unwrap(), 1);
    let mut request = check
        .query(REQUEST_COUNT, params![20_i64, "concurrent-key"])
        .await
        .unwrap();
    assert_eq!(
        request
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<i64>(0)
            .unwrap(),
        1
    );
    while request.next().await.unwrap().is_some() {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_connections_claim_more_than_one_hundred_jobs_once() {
    const JOBS: i64 = 120;
    let (_dir, db, mut setup) = prepared().await;
    for id in 1..=JOBS {
        admit(&mut setup, id, &format!("request-{id}"), "payload")
            .await
            .unwrap();
    }
    drop(setup);

    let db = Arc::new(db);
    let mut connections = Vec::new();
    for _ in 0..8 {
        let conn = db.connect().expect("independent connection");
        apply_connection_pragmas(&conn)
            .await
            .expect("connection pragmas");
        connections.push(conn);
    }
    let start = Arc::new(Barrier::new(9));
    let mut workers = Vec::new();
    for (worker, mut conn) in connections.into_iter().enumerate() {
        let start = Arc::clone(&start);
        workers.push(tokio::spawn(async move {
            start.wait().await;
            let mut ids = Vec::new();
            while let Some(id) = claim_bounded(&mut conn, &format!("worker-{worker}"), 1_000).await
            {
                ids.push(id);
            }
            ids
        }));
    }
    start.wait().await;
    let mut all = Vec::new();
    for worker in workers {
        all.extend(worker.await.unwrap());
    }
    let unique: HashSet<_> = all.iter().copied().collect();
    assert_eq!(all.len(), JOBS as usize);
    assert_eq!(unique.len(), all.len(), "no job may be owned twice");
    let check = db.connect().unwrap();
    assert_eq!(queue_count(&check, "leased").await.unwrap(), JOBS);
}

#[tokio::test]
async fn lease_generation_token_and_expiry_fence_stale_completion() {
    let (_dir, _db, mut conn) = prepared().await;
    admit(&mut conn, 1, "one", "payload").await.unwrap();
    let id = claim(&mut conn, "old-token", 100, 10)
        .await
        .unwrap()
        .unwrap();
    let snapshot = job(&conn, id).await.unwrap();
    let generation = snapshot.generation;
    assert_eq!(generation, 1);
    assert_eq!(snapshot.expires_at, Some(110));
    assert!(
        !mark_sending(&mut conn, id, generation, "old-token", 110)
            .await
            .unwrap()
    );
    assert_eq!(reclaim_expired(&mut conn, 110).await.unwrap(), 1);
    assert!(
        !complete(&mut conn, id, generation, "old-token", 111)
            .await
            .unwrap()
    );
    assert_eq!(
        claim(&mut conn, "new-token", 120, 50).await.unwrap(),
        Some(id)
    );
    let snapshot = job(&conn, id).await.unwrap();
    assert_eq!(snapshot.state, "leased");
    assert_eq!(snapshot.generation, generation + 1);
    assert_eq!(snapshot.token.as_deref(), Some("new-token"));
    assert_eq!(snapshot.expires_at, Some(170));
    assert!(
        !complete(&mut conn, id, generation, "new-token", 121)
            .await
            .unwrap()
    );
    assert!(
        !complete(&mut conn, id, snapshot.generation, "old-token", 121)
            .await
            .unwrap()
    );
    assert!(
        !complete(&mut conn, id, snapshot.generation, "new-token", 170)
            .await
            .unwrap()
    );
    assert!(
        complete(&mut conn, id, snapshot.generation, "new-token", 169)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn persisted_sending_recovers_to_delivery_unknown_not_queued() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = path(&dir);
    let id;
    {
        let (_db, mut conn) = open(&db_path).await.unwrap();
        admit(&mut conn, 2, "send", "external delivery not performed")
            .await
            .unwrap();
        id = claim(&mut conn, "lease", 10, 100).await.unwrap().unwrap();
        let generation = job(&conn, id).await.unwrap().generation;
        assert!(
            mark_sending(&mut conn, id, generation, "lease", 20)
                .await
                .unwrap()
        );
    }
    let (_db, mut conn) = open_existing(&db_path).await.unwrap();
    assert_eq!(job(&conn, id).await.unwrap().state, "sending");
    assert_eq!(recover_sending(&mut conn).await.unwrap(), 1);
    assert_eq!(job(&conn, id).await.unwrap().state, "delivery_unknown");
    assert_eq!(queue_count(&conn, "queued").await.unwrap(), 0);
    assert_eq!(claim(&mut conn, "other", 20, 100).await.unwrap(), None);
}

#[tokio::test]
async fn failed_multistatement_admission_leaves_no_orphan_request() {
    let (_dir, _db, mut conn) = prepared().await;
    let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
        .await
        .unwrap();
    tx.execute(include_str!("../sql/admit_actor.sql"), params![12_i64])
        .await
        .unwrap();
    tx.execute(
        include_str!("../sql/admit_request.sql"),
        params![12_i64, "will-fail"],
    )
    .await
    .unwrap();
    let error = tx
        .execute(
            include_str!("../sql/insert_bad_state.sql"),
            params![900_001_i64, 12_i64, "will-fail", "payload", "invalid"],
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Constraint(_)));
    tx.rollback().await.unwrap();
    let mut rows = conn
        .query(REQUEST_COUNT, params![12_i64, "will-fail"])
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
    while rows.next().await.unwrap().is_some() {}
    conn.execute(INSERT_ACTOR_STRICT, params![12_i64])
        .await
        .unwrap();
}

#[tokio::test]
async fn sqlite_constraints_and_connection_pragmas_are_enforced() {
    let (_dir, _db, mut conn) = prepared().await;
    assert_eq!(pragmas(&conn).await.unwrap(), (1, 2, BUSY_TIMEOUT_MS));
    admit(&mut conn, 3, "valid", "payload").await.unwrap();

    let primary_key = conn
        .execute(INSERT_ACTOR_STRICT, params![3_i64])
        .await
        .unwrap_err();
    assert!(matches!(primary_key, Error::Constraint(_)));

    let unique_key = conn
        .execute(INSERT_DUPLICATE_REQUEST, params![3_i64, "valid"])
        .await
        .unwrap_err();
    assert!(matches!(unique_key, Error::Constraint(_)));

    let unique_job = conn
        .execute(
            include_str!("../sql/insert_job_strict.sql"),
            params![3_i64, "valid", "duplicate"],
        )
        .await
        .unwrap_err();
    assert!(matches!(unique_job, Error::Constraint(_)));
    conn.execute(
        include_str!("../sql/admit_request.sql"),
        params![3_i64, "invalid-state-test"],
    )
    .await
    .unwrap();
    let check = conn
        .execute(
            include_str!("../sql/insert_bad_state.sql"),
            params![
                700_002_i64,
                3_i64,
                "invalid-state-test",
                "bad",
                "impossible"
            ],
        )
        .await
        .unwrap_err();
    assert!(matches!(check, Error::Constraint(_)));

    let foreign_key = conn
        .execute(
            include_str!("../sql/insert_audit.sql"),
            params![8_888_888_i64],
        )
        .await
        .unwrap_err();
    assert!(matches!(foreign_key, Error::Constraint(_)));
}

#[tokio::test]
async fn explicit_rollback_and_cancelled_transaction_restore_connection_reuse() {
    let (_dir, _db, mut conn) = prepared().await;
    {
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .unwrap();
        tx.execute(ROLLBACK_PROBE, ()).await.unwrap();
        tx.rollback().await.unwrap();
    }
    conn.execute(INSERT_ACTOR_STRICT, params![700_001_i64])
        .await
        .unwrap();

    let (written_tx, written_rx) = oneshot::channel();
    {
        let cancelled = async {
            let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
                .await
                .unwrap();
            tx.execute(CANCELLED_ROLLBACK_PROBE, ()).await.unwrap();
            let _ = written_tx.send(());
            std::future::pending::<()>().await;
            tx.commit().await.unwrap();
        };
        tokio::select! {
            _ = cancelled => panic!("transaction completed before cancellation"),
            written = timeout(Duration::from_secs(2), written_rx) => {
                written.expect("write handshake timeout").expect("write handshake dropped");
            }
        }
    }
    // A read must trigger deferred rollback too, not only the next write.
    let mut actor = conn
        .query(include_str!("../sql/count_actor.sql"), params![700_002_i64])
        .await
        .unwrap();
    assert_eq!(
        actor.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
    while actor.next().await.unwrap().is_some() {}
    // Strict INSERT of the cancelled key would fail if the transaction committed.
    conn.execute(INSERT_ACTOR_STRICT, params![700_002_i64])
        .await
        .unwrap();
    admit(&mut conn, 42, "reused", "ok").await.unwrap();
    assert_eq!(queue_count(&conn, "queued").await.unwrap(), 1);
}

#[tokio::test]
async fn committed_data_survives_close_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = path(&dir);
    {
        let (_db, mut conn) = open(&db_path).await.unwrap();
        admit(&mut conn, 4, "durable", "value").await.unwrap();
    }
    let (_db, conn) = open_existing(&db_path).await.unwrap();
    assert_eq!(queue_count(&conn, "queued").await.unwrap(), 1);
}

#[tokio::test]
async fn vacuum_into_creates_consistent_restorable_backup() {
    let dir = tempfile::tempdir().unwrap();
    let source_path = path(&dir);
    let backup_path = dir
        .path()
        .join("backup snapshot.db")
        .to_string_lossy()
        .into_owned();
    let (_db, mut source) = open(&source_path).await.unwrap();
    admit(&mut source, 5, "backup", "snapshot").await.unwrap();
    backup_into(&source, &backup_path).await.unwrap();
    admit(&mut source, 6, "after-snapshot", "not in backup")
        .await
        .unwrap();
    drop(source);
    let (_backup_db, backup) = open_existing(&backup_path).await.unwrap();
    assert_eq!(queue_count(&backup, "queued").await.unwrap(), 1);
    let mut payload = backup.query(JOB_PAYLOAD, params![1_i64]).await.unwrap();
    assert_eq!(
        payload
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>(0)
            .unwrap(),
        "snapshot"
    );
    while payload.next().await.unwrap().is_some() {}
    let mut schema = backup.query(JOB_SCHEMA, ()).await.unwrap();
    let schema_sql = schema
        .next()
        .await
        .unwrap()
        .unwrap()
        .get::<String>(0)
        .unwrap();
    assert!(schema_sql.contains("CHECK"));
    assert!(schema_sql.contains("UNIQUE (actor_id, request_key)"));
    assert!(schema_sql.contains("FOREIGN KEY"));
    while schema.next().await.unwrap().is_some() {}
    let mut missing = backup
        .query(COUNT_PAYLOAD, params!["not in backup"])
        .await
        .unwrap();
    assert_eq!(
        missing
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<i64>(0)
            .unwrap(),
        0
    );
    while missing.next().await.unwrap().is_some() {}
    let duplicate = backup
        .execute(
            include_str!("../sql/insert_job_strict.sql"),
            params![5_i64, "backup", "duplicate"],
        )
        .await
        .unwrap_err();
    assert!(matches!(duplicate, Error::Constraint(_)));
    let foreign_key = backup
        .execute(
            include_str!("../sql/insert_audit.sql"),
            params![8_888_888_i64],
        )
        .await
        .unwrap_err();
    assert!(matches!(foreign_key, Error::Constraint(_)));
    backup
        .execute(
            include_str!("../sql/admit_request.sql"),
            params![5_i64, "invalid-state-test"],
        )
        .await
        .unwrap();
    let invalid_state = backup
        .execute(
            include_str!("../sql/insert_bad_state.sql"),
            params![
                700_002_i64,
                5_i64,
                "invalid-state-test",
                "bad",
                "impossible"
            ],
        )
        .await
        .unwrap_err();
    assert!(matches!(invalid_state, Error::Constraint(_)));
}

#[tokio::test]
async fn backup_rejects_unsupported_paths_before_creating_a_copy() {
    let (dir, _db, conn) = prepared().await;
    let destination = dir.path().join("backup's.db");
    let error = backup_into(&conn, destination.to_str().unwrap())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Misuse(_)));
    assert!(!tokio::fs::try_exists(&destination).await.unwrap());
    assert!(
        !tokio::fs::try_exists(dir.path().join("backup''s.db"))
            .await
            .unwrap()
    );
    assert_eq!(queue_count(&conn, "queued").await.unwrap(), 0);
}

#[tokio::test]
async fn restore_does_not_create_a_missing_database() {
    let dir = tempfile::tempdir().unwrap();
    let missing = path(&dir);
    assert!(matches!(
        open_existing(&missing).await,
        Err(Error::Misuse(_))
    ));
    assert!(!tokio::fs::try_exists(&missing).await.unwrap());
}

async fn crash_worker() {
    let db_path = std::env::var("YOINK_TURSO_DB").unwrap();
    let handshake = std::env::var("YOINK_TURSO_HANDSHAKE").unwrap();
    let (_db, mut conn) = open(&db_path).await.unwrap();
    admit(&mut conn, 50, "committed-before-kill", "keep")
        .await
        .unwrap();
    admit(
        &mut conn,
        52,
        "committed-sending-before-kill",
        "ambiguous delivery",
    )
    .await
    .unwrap();
    let sending_id = claim(&mut conn, "crash-lease", 10, 100)
        .await
        .unwrap()
        .unwrap();
    let generation = job(&conn, sending_id).await.unwrap().generation;
    assert!(
        mark_sending(&mut conn, sending_id, generation, "crash-lease", 20)
            .await
            .unwrap()
    );
    let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
        .await
        .unwrap();
    tx.execute(INSERT_ACTOR_STRICT, params![51_i64])
        .await
        .unwrap();
    std::fs::write(handshake, b"uncommitted transaction open").unwrap();
    std::future::pending::<()>().await;
    tx.commit().await.unwrap();
}

#[tokio::test]
#[ignore = "invoked as a child by crash_kill_keeps_only_committed_changes"]
async fn crash_child_entrypoint() {
    if std::env::var_os("YOINK_TURSO_CRASH_CHILD").is_some() {
        crash_worker().await;
    }
}

#[tokio::test]
async fn crash_kill_keeps_only_committed_changes() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = path(&dir);
    let handshake = dir.path().join("ready").to_string_lossy().into_owned();
    let executable = std::env::current_exe().unwrap();
    let mut child = Command::new(executable)
        .arg("--exact")
        .arg("crash_child_entrypoint")
        .arg("--ignored")
        .arg("--nocapture")
        .env_clear()
        .env("YOINK_TURSO_CRASH_CHILD", "1")
        .env("YOINK_TURSO_DB", &db_path)
        .env("YOINK_TURSO_HANDSHAKE", &handshake)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !std::path::Path::new(&handshake).exists() {
        if Instant::now() >= deadline {
            let _ = child.kill().await;
            let _ = child.wait().await;
            panic!("child did not report its post-commit, pre-commit handshake");
        }
        sleep(Duration::from_millis(10)).await;
    }
    child.kill().await.expect("send SIGKILL");
    let status = child.wait().await.expect("reap child");
    assert_eq!(status.signal(), Some(9), "child must die from SIGKILL");
    let (_db, mut conn) = open_existing(&db_path).await.unwrap();
    assert_eq!(queue_count(&conn, "queued").await.unwrap(), 1);
    assert_eq!(queue_count(&conn, "sending").await.unwrap(), 1);
    let mut actor = conn
        .query(include_str!("../sql/count_actor.sql"), params![51_i64])
        .await
        .unwrap();
    assert_eq!(
        actor.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
    while actor.next().await.unwrap().is_some() {}
    let mut kept = conn
        .query(COUNT_KEPT_PAYLOAD, params!["keep"])
        .await
        .unwrap();
    assert_eq!(
        kept.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        1
    );
    while kept.next().await.unwrap().is_some() {}
    assert_eq!(recover_sending(&mut conn).await.unwrap(), 1);
    assert_eq!(queue_count(&conn, "delivery_unknown").await.unwrap(), 1);
    assert_eq!(job(&conn, 1).await.unwrap().state, "delivery_unknown");
    let mut queued_payload = conn
        .query(COUNT_PAYLOAD, params!["ambiguous delivery"])
        .await
        .unwrap();
    assert_eq!(
        queued_payload
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<i64>(0)
            .unwrap(),
        1
    );
    while queued_payload.next().await.unwrap().is_some() {}
    assert_eq!(
        claim(&mut conn, "after-crash", 30, 100).await.unwrap(),
        Some(2)
    );
    assert_eq!(claim(&mut conn, "next", 31, 100).await.unwrap(), None);
    conn.execute(INSERT_ACTOR_STRICT, params![51_i64])
        .await
        .unwrap();
}
