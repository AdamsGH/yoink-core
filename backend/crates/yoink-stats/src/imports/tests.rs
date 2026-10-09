use super::*;
use serde_json::json;
use yoink_db::query;
use yoink_identity::{IdentityStore, TrustedProfile};

struct Members;
impl crate::MembershipProvider for Members {
    fn lookup(&self, _: i64, _: i64) -> crate::MembershipFuture<'_> {
        Box::pin(async { Err(Error::Unavailable) })
    }
    fn lookup_name(&self, _: Option<i64>, _: i64) -> crate::NameFuture<'_> {
        Box::pin(async { Ok(None) })
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    store: yoink_storage::Store,
    stats: Stats,
    jobs: ImportJobs,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let store =
            yoink_storage::Store::open(&dir.path().join("stats.db"), true, Duration::from_secs(2))
                .await
                .unwrap();
        store.migrate().await.unwrap();
        let identities = Arc::new(IdentityStore::new(store.database().clone(), Some(1), false));
        for user_id in [1, 2] {
            identities
                .register(&TrustedProfile {
                    user_id,
                    username: None,
                    first_name: None,
                    photo_url: None,
                })
                .await
                .unwrap();
        }
        let stats = Stats::open(
            store.database().clone(),
            identities,
            Arc::new(Members),
            None,
            dir.path().join("search"),
        )
        .await
        .unwrap();
        let jobs = ImportJobs::new(
            stats.clone(),
            ImportConfig {
                root: dir.path().to_owned(),
                max_bytes: 16384,
                max_messages: 100,
                max_jobs: 1,
                timeout: Duration::from_secs(2),
            },
        )
        .unwrap();
        Self {
            _dir: dir,
            store,
            stats,
            jobs,
        }
    }
    async fn close(self) {
        self.jobs.cancel_all();
        self.jobs.drain().await;
        self.stats.shutdown().await.unwrap();
        self.store.close().await.unwrap();
    }
}
fn dump() -> Vec<u8> {
    serde_json::to_vec(&json!({"messages":[
        {"id":1,"type":"message","date":"2026-01-01T00:00:00","from_id":"user999","from":"Private fixture name","text":["hello ",{"type":"bold","text":"world"}]},
        {"id":2,"type":"message","date":"2026-01-01T00:00:01+00:00","photo":"photo.jpg","text":"caption"},
        {"id":3,"type":"service","date":"2026-01-01T00:00:02","action":"invite_members","members":["user999","user999",123]},
        {"id":4,"type":"unsupported"}
    ]})).unwrap()
}
#[tokio::test]
async fn desktop_entities_media_events_are_additive_and_never_register_users() {
    let f = Fixture::new().await;
    let c = f.stats.import_desktop(1, -100, dump(), 100).await.unwrap();
    assert_eq!((c.inserted, c.skipped, c.events, c.total), (3, 1, 2, 4));
    let row = query("SELECT text FROM stats_messages WHERE message_id=1")
        .fetch_one(f.store.database())
        .await
        .unwrap();
    assert_eq!(row.try_get::<String, _>(0).unwrap(), "hello world");
    let row = query("SELECT caption,msg_type FROM stats_messages WHERE message_id=2")
        .fetch_one(f.store.database())
        .await
        .unwrap();
    assert_eq!(row.try_get::<String, _>(0).unwrap(), "caption");
    assert_eq!(row.try_get::<String, _>(1).unwrap(), "photo");
    assert!(
        query("SELECT 1 FROM users WHERE id=999")
            .fetch_optional(f.store.database())
            .await
            .unwrap()
            .is_none()
    );
    query("UPDATE stats_messages SET text='live edit',is_edited=1 WHERE message_id=1")
        .execute(f.store.database())
        .await
        .unwrap();
    let c = f.stats.import_desktop(1, -100, dump(), 100).await.unwrap();
    assert_eq!((c.inserted, c.skipped, c.events), (0, 4, 0));
    assert_eq!(
        query("SELECT text FROM stats_messages WHERE message_id=1")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "live edit"
    );
    f.close().await;
}
#[tokio::test]
async fn invalid_later_row_rolls_back_messages_names_and_events() {
    let f = Fixture::new().await;
    let mut data: serde_json::Value = serde_json::from_slice(&dump()).unwrap();
    data["messages"][3] = json!({"id":4,"type":"message","date":"invalid","text":"secret content"});
    assert!(matches!(
        f.stats
            .import_desktop(1, -100, serde_json::to_vec(&data).unwrap(), 100)
            .await,
        Err(Error::Invalid)
    ));
    for table in ["stats_messages", "stats_user_names", "stats_user_events"] {
        assert_eq!(
            query(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(f.store.database())
                .await
                .unwrap()
                .try_get::<i64, _>(0)
                .unwrap(),
            0
        );
    }
    f.close().await;
}
#[tokio::test]
async fn canceled_file_work_keeps_its_capacity_and_is_joined_before_drain() {
    let f = Fixture::new().await;
    let path = f._dir.path().join("result.json");
    tokio::fs::write(&path, dump()).await.unwrap();
    let input = f.jobs.reserve_input().unwrap();
    assert!(f.jobs.reserve_input().is_err());
    let (entered, signal) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let jobs = f.jobs.clone();
    let reader = tokio::spawn(async move {
        jobs.blocking(input, move || {
            let file = std::fs::File::open(path).map_err(|_| Error::Invalid)?;
            let _ = entered.send(());
            wait.recv().map_err(|_| Error::Unavailable)?;
            Ok(file)
        })
        .await
    });
    signal.await.unwrap();
    reader.abort();
    assert!(reader.await.unwrap_err().is_cancelled());
    assert!(f.jobs.reserve_input().is_err());
    let jobs = f.jobs.clone();
    let mut draining = tokio::spawn(async move { jobs.drain().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut draining)
            .await
            .is_err()
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), draining)
        .await
        .unwrap()
        .unwrap();
    f.close().await;
}

#[tokio::test]
async fn owner_checks_input_caps_retention_and_frontend_status_shape() {
    let f = Fixture::new().await;
    assert!(matches!(
        f.jobs.start(2, -100, dump()).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        f.jobs.start(1, 100, dump()).await,
        Err(Error::Invalid)
    ));
    assert!(matches!(
        f.jobs.start(1, -100, vec![0; 16385]).await,
        Err(Error::Invalid)
    ));
    assert!(matches!(
        f.stats.import_desktop(1, -100, dump(), 1).await,
        Err(Error::Invalid)
    ));
    let first = f.jobs.start(1, -100, dump()).await.unwrap();
    f.jobs.0.tasks.close();
    tokio::time::timeout(Duration::from_secs(2), f.jobs.0.tasks.wait())
        .await
        .unwrap();
    let status = f.jobs.status(1, &first.job_id).await.unwrap().unwrap();
    let shape = serde_json::to_value(status).unwrap();
    assert_eq!(shape["status"], "done");
    assert_eq!(shape["processed"], 4);
    assert!(shape["error"].is_null());
    for field in [
        "job_id",
        "status",
        "inserted",
        "skipped",
        "events",
        "processed",
        "total",
        "error",
    ] {
        assert!(shape.get(field).is_some());
    }
    // Job ownership is narrower than current owner role.
    assert!(matches!(
        f.jobs.status(2, &first.job_id).await,
        Err(Error::Forbidden)
    ));
    query("UPDATE users SET role='owner' WHERE id=2")
        .execute(f.store.database())
        .await
        .unwrap();
    assert!(f.jobs.status(2, &first.job_id).await.unwrap().is_none());
    let second = f.jobs.start(1, -100, dump()).await.unwrap();
    assert_ne!(first.job_id, second.job_id);
    assert!(f.jobs.status(1, &first.job_id).await.unwrap().is_none());
    f.jobs.0.tasks.close();
    f.jobs.0.tasks.wait().await;
    query("UPDATE users SET ban_until=4070908800000000 WHERE id=1")
        .execute(f.store.database())
        .await
        .unwrap();
    assert!(matches!(
        f.jobs.status(1, &second.job_id).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        f.jobs.start(1, -100, dump()).await,
        Err(Error::Forbidden)
    ));
    f.close().await;
}
#[tokio::test]
async fn canceled_bounded_parser_result_keeps_capacity_through_disposal() {
    struct Parsed {
        messages: Vec<serde_json::Value>,
        jobs: ImportJobs,
        disposed: Arc<std::sync::atomic::AtomicBool>,
    }
    impl Drop for Parsed {
        fn drop(&mut self) {
            assert_eq!(self.messages.len(), 4);
            assert!(self.jobs.reserve_input().is_err());
            self.messages.clear();
            self.disposed
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let f = Fixture::new().await;
    let input = f.jobs.reserve_input().unwrap();
    let disposed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = disposed.clone();
    let jobs = f.jobs.clone();
    let owner = jobs.clone();
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let caller = tokio::spawn(async move {
        jobs.blocking(input, move || {
            let parsed = Parsed {
                messages: parser::messages(&dump(), 4)?,
                jobs: owner,
                disposed: observed,
            };
            entered.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(2)).unwrap();
            Ok(parsed)
        })
        .await
        .map(drop)
    });
    tokio::time::timeout(Duration::from_secs(2), started)
        .await
        .unwrap()
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(f.jobs.reserve_input().is_err());
    assert!(!disposed.load(std::sync::atomic::Ordering::SeqCst));
    f.jobs.0.tasks.close();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), f.jobs.0.tasks.wait())
            .await
            .is_err()
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), f.jobs.0.tasks.wait())
        .await
        .unwrap();
    assert!(disposed.load(std::sync::atomic::Ordering::SeqCst));
    assert!(f.jobs.reserve_input().is_ok());
    f.close().await;
}

#[tokio::test]
async fn active_job_cancellation_drains_written_transaction_before_database_reuse() {
    let f = Fixture::new().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *f.jobs.0.desktop_pause.lock().unwrap() = Some((entered.clone(), release));
    let job = f.jobs.start(1, -100, dump()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    assert!(f.jobs.reserve_input().is_err());
    f.jobs.cancel_all();
    tokio::time::timeout(Duration::from_secs(2), f.jobs.drain())
        .await
        .unwrap();
    let status = f.jobs.status(1, &job.job_id).await.unwrap().unwrap();
    assert_eq!(status.status, "error");
    assert_eq!(status.error, Some("import_stopped"));
    for table in ["stats_messages", "stats_user_names", "stats_user_events"] {
        assert_eq!(
            query(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(f.store.database())
                .await
                .unwrap()
                .try_get::<i64, _>(0)
                .unwrap(),
            0
        );
    }
    assert_eq!(
        f.stats
            .import_desktop(1, -100, dump(), 100)
            .await
            .unwrap()
            .inserted,
        3
    );
    f.close().await;
}
#[tokio::test]
async fn cancellation_closes_admission_drains_and_leaves_database_usable() {
    let f = Fixture::new().await;
    let job = f.jobs.start(1, -100, dump()).await.unwrap();
    // No yield since spawn: cancellation wins the biased branch before any import writes.
    f.jobs.cancel_all();
    tokio::time::timeout(Duration::from_secs(2), f.jobs.drain())
        .await
        .unwrap();
    assert_eq!(
        f.jobs.status(1, &job.job_id).await.unwrap().unwrap().error,
        Some("import_stopped")
    );
    assert!(matches!(
        f.jobs.start(1, -100, dump()).await,
        Err(Error::Unavailable)
    ));
    assert!(f.jobs.reserve_input().is_err());
    assert_eq!(
        query("SELECT count(*) FROM stats_messages")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<i64, _>(0)
            .unwrap(),
        0
    );
    f.close().await;
}
#[test]
fn config_rejects_zero_caps_relative_root_and_excessive_materialization() {
    let mut c = ImportConfig {
        root: "/imports".into(),
        max_bytes: 100,
        max_messages: 10,
        max_jobs: 1,
        timeout: Duration::from_secs(1),
    };
    assert!(c.validate().is_ok());
    c.max_bytes = 0;
    assert!(c.validate().is_err());
    c.max_bytes = usize::MAX;
    assert!(c.validate().is_err());
    c.max_bytes = 100;
    c.root = "relative".into();
    assert!(c.validate().is_err());
    c.root = "/imports".into();
    c.max_jobs = 0;
    assert!(c.validate().is_err());
    c.max_jobs = 1;
    c.max_messages = 0;
    assert!(c.validate().is_err());
    c.max_messages = 1;
    c.timeout = Duration::ZERO;
    assert!(c.validate().is_err());
}

#[tokio::test]
async fn cancelled_transaction_rolls_back_written_rows_before_connection_reuse() {
    let f = Fixture::new().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let stats = f.stats.clone();
    let signal = entered.clone();
    let hold = release.clone();
    let task = tokio::spawn(async move {
        stats
            .desktop_inner(
                1,
                -100,
                dump(),
                100,
                (&stats.0.search_tasks, None),
                Some((&signal, &hold)),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    for table in ["stats_messages", "stats_user_names", "stats_user_events"] {
        assert_eq!(
            query(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(f.store.database())
                .await
                .unwrap()
                .try_get::<i64, _>(0)
                .unwrap(),
            0
        );
    }
    assert_eq!(
        f.stats
            .import_desktop(1, -100, dump(), 100)
            .await
            .unwrap()
            .inserted,
        3
    );
    f.close().await;
}
