use std::{
    future::{Future, poll_fn},
    task::Poll,
    time::Duration,
};

use turso::{Connection, transaction::TransactionBehavior};
use yoink_db::{Database, Transaction};

async fn fixture() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let db = Database::open(
        &dir.path().join("transaction.db"),
        true,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    db.connection()
        .await
        .unwrap()
        .execute("CREATE TABLE items(id INTEGER PRIMARY KEY, data BLOB)", ())
        .await
        .unwrap();
    (dir, db)
}

async fn repeated_reads(connection: &Connection) -> i64 {
    let mut counts = Vec::new();
    for _ in 0..2 {
        let mut rows = connection
            .query("SELECT COUNT(*) FROM items", ())
            .await
            .unwrap();
        counts.push(rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap());
        assert!(rows.next().await.unwrap().is_none());
    }
    assert_eq!(counts[0], counts[1]);
    counts[0]
}

#[tokio::test]
async fn commit_pending_after_autocommit_cancellation_reuses_connection_and_sdk_control_fails() {
    for sdk_control in [true, false] {
        let engine = turso::Builder::new_local("terminal.db")
            .with_io_impl(std::sync::Arc::new(turso_core::MemoryYieldIO::new()))
            .build()
            .await
            .unwrap();
        let shared = tokio::sync::Mutex::new(engine.connect().unwrap());
        let mut c = shared.lock().await;
        c.execute("CREATE TABLE items(id INTEGER PRIMARY KEY, data BLOB)", ())
            .await
            .unwrap();
        let observer = c.clone();
        let mut terminal: std::pin::Pin<Box<dyn Future<Output = turso::Result<()>> + Send + '_>> =
            if sdk_control {
                let tx =
                    turso::transaction::Transaction::new(&mut c, TransactionBehavior::Immediate)
                        .await
                        .unwrap();
                tx.execute("INSERT INTO items VALUES(1,zeroblob(65536))", ())
                    .await
                    .unwrap();
                Box::pin(tx.commit())
            } else {
                let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
                    .await
                    .unwrap();
                tx.execute("INSERT INTO items VALUES(1,zeroblob(65536))", ())
                    .await
                    .unwrap();
                Box::pin(tx.commit())
            };
        assert!(!observer.is_autocommit().unwrap());
        // Observe the real SDK Future result, not an injected post-success pause.
        tokio::time::timeout(
            Duration::from_secs(5),
            poll_fn(|cx| match terminal.as_mut().poll(cx) {
                Poll::Pending if observer.is_autocommit().unwrap() => Poll::Ready(()),
                Poll::Pending => Poll::Pending,
                Poll::Ready(result) => {
                    panic!("COMMIT did not expose the required Pending boundary: {result:?}")
                }
            }),
        )
        .await
        .unwrap();
        drop(terminal);
        drop(c);
        let mut c = shared.lock().await;
        if sdk_control {
            for _ in 0..2 {
                let error = c
                    .query("SELECT COUNT(*) FROM items", ())
                    .await
                    .err()
                    .unwrap();
                assert!(
                    error.to_string().contains("no transaction is active"),
                    "{error}"
                );
            }
            assert!(
                Transaction::new(&mut c, TransactionBehavior::Immediate)
                    .await
                    .is_err()
            );
        } else {
            let count = repeated_reads(&c).await;
            assert!((0..=1).contains(&count));
            let admitted = c
                .execute(
                    "INSERT INTO items VALUES(1,NULL) ON CONFLICT(id) DO NOTHING",
                    (),
                )
                .await
                .unwrap();
            assert_eq!(admitted, u64::from(count == 0));
            assert_eq!(
                c.execute(
                    "INSERT INTO items VALUES(1,NULL) ON CONFLICT(id) DO NOTHING",
                    ()
                )
                .await
                .unwrap(),
                0
            );
            let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
                .await
                .unwrap();
            tx.execute("INSERT INTO items VALUES(2,NULL)", ())
                .await
                .unwrap();
            tx.commit().await.unwrap();
            repeated_reads(&c).await;
        }
    }
}

#[tokio::test]
async fn rollback_ready_and_unpolled_cancellation_reuse_connection() {
    for cancel_before_poll in [false, true] {
        let engine = turso::Builder::new_local("rollback.db")
            .with_io_impl(std::sync::Arc::new(turso_core::MemoryYieldIO::new()))
            .build()
            .await
            .unwrap();
        let shared = tokio::sync::Mutex::new(engine.connect().unwrap());
        let mut c = shared.lock().await;
        c.execute("CREATE TABLE items(id INTEGER PRIMARY KEY, data BLOB)", ())
            .await
            .unwrap();
        let observer = c.clone();
        let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
            .await
            .unwrap();
        tx.execute("INSERT INTO items VALUES(1,zeroblob(65536))", ())
            .await
            .unwrap();
        assert!(!observer.is_autocommit().unwrap());
        let mut terminal = Box::pin(tx.rollback());
        if !cancel_before_poll {
            poll_fn(|cx| match terminal.as_mut().poll(cx) {
            Poll::Ready(result) => { result.unwrap(); Poll::Ready(()) },
            Poll::Pending => panic!("Previously synchronous public rollback now exposes Pending; add coverage for this boundary"),
        }).await;
            assert!(observer.is_autocommit().unwrap());
        }
        drop(terminal);
        drop(c);
        let mut c = shared.lock().await;
        assert_eq!(repeated_reads(&c).await, 0);
        Transaction::new(&mut c, TransactionBehavior::Immediate)
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();
        assert_eq!(repeated_reads(&c).await, 0);
    }
}

#[tokio::test]
async fn engine_rollback_statement_error_does_not_install_stale_rollback() {
    for commit_after_rollback in [false, true] {
        let (_dir, db) = fixture().await;
        let mut c = db.connection().await.unwrap();
        c.execute("INSERT INTO items VALUES(1,NULL)", ())
            .await
            .unwrap();
        {
            let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
                .await
                .unwrap();
            tx.execute("INSERT INTO items VALUES(2,NULL)", ())
                .await
                .unwrap();
            assert!(
                tx.execute("INSERT OR ROLLBACK INTO items VALUES(1,NULL)", ())
                    .await
                    .is_err()
            );
            assert!(tx.is_autocommit().unwrap());
            if commit_after_rollback {
                assert!(tx.commit().await.is_err());
            }
        }
        assert_eq!(repeated_reads(&c).await, 1);
        let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
            .await
            .unwrap();
        tx.execute("INSERT INTO items VALUES(2,NULL)", ())
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(repeated_reads(&c).await, 1);
    }
}
