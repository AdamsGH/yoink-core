use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc, time::Duration};
use tokio::sync::Notify;
use yoink_db::query;

use super::*;

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Stage {
    Snapshot,
    Publish,
}
#[derive(Debug)]
pub(crate) struct Pause {
    stage: Stage,
    entered: Notify,
    resume: Notify,
}
impl Pause {
    fn new(stage: Stage) -> Arc<Self> {
        Arc::new(Self {
            stage,
            entered: Notify::new(),
            resume: Notify::new(),
        })
    }
    pub(crate) async fn reach(&self, stage: Stage) {
        if self.stage == stage {
            self.entered.notify_one();
            self.resume.notified().await;
        }
    }
}
async fn prepared(path: &Path) -> Store {
    std::fs::set_permissions(
        path.parent().unwrap(),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let store = Store::open(path, true, Duration::from_secs(5))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    exec(
        &store,
        "INSERT INTO users(id, created_at, updated_at, first_name) VALUES(1,1,1,'before')",
    )
    .await;
    store
}
async fn exec(store: &Store, sql: &str) {
    store
        .database()
        .connection()
        .await
        .unwrap()
        .execute_batch(sql)
        .await
        .unwrap();
}
async fn name(store: &Store) -> String {
    query("SELECT first_name FROM users WHERE id=1")
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get("first_name")
        .unwrap()
}
fn shared(store: &Store) -> Store {
    Store::from_database(store.database().clone())
}
async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(15), future)
        .await
        .expect("bounded operation")
}

#[tokio::test]
async fn snapshot_schema_survives_rebuild_migrations() {
    let dir = tempfile::tempdir().unwrap();
    let store = prepared(&dir.path().join("source.db")).await;
    store.verify_schema().await.unwrap();
    async fn schema(store: &Store) -> std::collections::BTreeMap<String, Option<String>> {
        query(include_str!("../../yoink-storage/sql/verify_schema.sql"))
            .fetch_all(store.database())
            .await
            .unwrap()
            .into_iter()
            .map(|row| (row.try_get("name").unwrap(), row.try_get("sql").unwrap()))
            .collect()
    }
    exec(
        &store,
        include_str!("../../yoink-storage/sql/test_content_policy_upgrade_fixture.sql"),
    )
    .await;
    exec(
        &store,
        include_str!("../sql/test_content_policy_snapshot_failures.sql"),
    )
    .await;
    async fn runtime_rows(store: &Store) -> Vec<Vec<String>> {
        let mut result = Vec::new();
        for sql in [
            include_str!("../../yoink-storage/sql/test_content_policy_operations_rows.sql"),
            include_str!("../../yoink-storage/sql/test_content_policy_units_rows.sql"),
            include_str!("../../yoink-storage/sql/test_content_policy_notices_rows.sql"),
        ] {
            result.push(
                query(sql)
                    .fetch_all(store.database())
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|row| row.try_get("snapshot").unwrap())
                    .collect(),
            );
        }
        result
    }
    async fn failures(store: &Store) -> Vec<(String, Option<String>)> {
        query(include_str!(
            "../../yoink-storage/sql/test_content_policy_failures.sql"
        ))
        .fetch_all(store.database())
        .await
        .unwrap()
        .into_iter()
        .map(|row| {
            (
                row.try_get("id").unwrap(),
                row.try_get("content_failure").unwrap(),
            )
        })
        .collect()
    }
    let before_rows = runtime_rows(&store).await;
    let before_failures = failures(&store).await;
    assert!(
        before_failures
            .iter()
            .any(|(_, f)| f.as_deref() == Some("nsfw_blocked"))
    );
    assert!(
        before_failures
            .iter()
            .any(|(_, f)| f.as_deref() == Some("nsfw_policy_unavailable"))
    );
    let before = schema(&store).await;
    let destination = dir.path().join("snapshot.db");
    let path = destination.to_str().unwrap();
    assert!(!path.contains('\''));
    exec(
        &store,
        &include_str!("../sql/test_snapshot_schema.sql").replace("{destination}", path),
    )
    .await;
    let snapshot = Store::open(&destination, false, Duration::from_secs(5))
        .await
        .unwrap();
    let after = schema(&snapshot).await;
    assert_eq!(before.len(), after.len(), "snapshot schema object count");
    for (name, sql) in &before {
        assert_eq!(
            after.get(name),
            Some(sql),
            "snapshot schema changed for {name}"
        );
    }
    snapshot.verify_schema().await.unwrap();
    assert_eq!(runtime_rows(&snapshot).await, before_rows);
    assert_eq!(failures(&snapshot).await, before_failures);
    snapshot.close().await.unwrap();
    store.close().await.unwrap();
}

#[tokio::test]
async fn snapshot_roundtrip_at_literal_paths_preserves_committed_state_and_source() {
    let dir = tempfile::Builder::new()
        .prefix("yoink ' quoted ")
        .tempdir()
        .unwrap();
    let store = prepared(&dir.path().join("source.db")).await;
    let backup_path = dir.path().join("backup ' ; --.ybk");
    let target = dir.path().join("restored.db");
    let backup_report = bounded(backup(
        shared(&store),
        backup_path.clone(),
        Options::default(),
        CancellationToken::new(),
    ))
    .await
    .unwrap();
    exec(&store, "UPDATE users SET first_name='after' WHERE id=1").await;
    let restore_report = bounded(restore(
        backup_path,
        target.clone(),
        Options::default(),
        CancellationToken::new(),
    ))
    .await
    .unwrap();
    assert_eq!(backup_report, restore_report);
    let restored = Store::open(&target, false, Duration::from_secs(5))
        .await
        .unwrap();
    restored.verify_schema().await.unwrap();
    assert_eq!(name(&restored).await, "before");
    assert_eq!(name(&store).await, "after");
    restored.close().await.unwrap();
    store.close().await.unwrap();
}

#[tokio::test]
async fn restored_database_rebuilds_existing_derived_index_before_returning_results() {
    use yoink_stats_search::{SearchRequest, SearchService};
    let dir = tempfile::tempdir().unwrap();
    let store = prepared(&dir.path().join("source.db")).await;
    exec(&store,"INSERT INTO stats_messages(id,message_id,chat_id,date,text) VALUES(1,1,-1,1234567,'before')").await;
    let index = dir.path().join("index");
    let search = SearchService::open(store.database().clone(), &index)
        .await
        .unwrap();
    let artifact = dir.path().join("archive");
    backup(
        shared(&store),
        artifact.clone(),
        Options::default(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    exec(&store, "UPDATE stats_messages SET text='after' WHERE id=1").await;
    search.refresh().await.unwrap();
    drop(search);
    let restored_path = dir.path().join("restored");
    restore(
        artifact,
        restored_path.clone(),
        Options::default(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let restored = Store::open(&restored_path, false, Duration::from_secs(5))
        .await
        .unwrap();
    let search = SearchService::open(restored.database().clone(), index)
        .await
        .unwrap();
    for (text, total) in [("before", 1), ("after", 0)] {
        let result = search
            .search(SearchRequest {
                chat_id: -1,
                query: Some(text.into()),
                limit: 10,
                ..SearchRequest::default()
            })
            .await
            .unwrap();
        assert_eq!(result.total, total);
    }
}

#[tokio::test]
async fn snapshot_holds_writer_gate_then_releases_without_mixing_later_commit() {
    let dir = tempfile::tempdir().unwrap();
    let store = prepared(&dir.path().join("source.db")).await;
    let pause = Pause::new(Stage::Snapshot);
    let options = Options {
        pause: Some(pause.clone()),
        ..Options::default()
    };
    let target = dir.path().join("backup");
    let task = tokio::spawn(backup(
        shared(&store),
        target.clone(),
        options,
        CancellationToken::new(),
    ));
    bounded(pause.entered.notified()).await;
    let writer_store = shared(&store);
    let mut writer = tokio::spawn(async move {
        exec(&writer_store,"BEGIN IMMEDIATE; UPDATE users SET first_name='later' WHERE id=1; UPDATE users SET updated_at=2 WHERE id=1; COMMIT;").await;
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut writer)
            .await
            .is_err()
    );
    pause.resume.notify_one();
    bounded(task).await.unwrap().unwrap();
    bounded(writer).await.unwrap();
    let restored = dir.path().join("restored");
    bounded(restore(
        target,
        restored.clone(),
        Options::default(),
        CancellationToken::new(),
    ))
    .await
    .unwrap();
    let restored = Store::open(&restored, false, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(name(&restored).await, "before");
    assert_eq!(name(&store).await, "later");
}

#[tokio::test]
async fn cancellation_after_snapshot_cleans_owned_staging_and_allows_retry() {
    let dir = tempfile::tempdir().unwrap();
    let store = prepared(&dir.path().join("source.db")).await;
    let target = dir.path().join("backup");
    let pause = Pause::new(Stage::Publish);
    let cancel = CancellationToken::new();
    let task = tokio::spawn(backup(
        shared(&store),
        target.clone(),
        Options {
            pause: Some(pause.clone()),
            ..Options::default()
        },
        cancel.clone(),
    ));
    bounded(pause.entered.notified()).await;
    cancel.cancel();
    pause.resume.notify_one();
    assert!(matches!(
        bounded(task).await.unwrap(),
        Err(Error::Cancelled)
    ));
    assert!(!target.exists());
    assert!(!dir.path().join("backup.yoink-lock").exists());
    assert!(!std::fs::read_dir(dir.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".yoink-publication-")
    }));
    bounded(backup(
        shared(&store),
        target,
        Options::default(),
        CancellationToken::new(),
    ))
    .await
    .unwrap();
}

#[tokio::test]
async fn collisions_limits_corruption_and_runtime_ownership_preserve_existing_files() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.db");
    let store = prepared(&source).await;
    assert!(
        Store::open(&source, false, Duration::from_millis(20))
            .await
            .is_err()
    );
    assert!(
        backup(
            shared(&store),
            source.clone(),
            Options::default(),
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    assert_eq!(name(&store).await, "before");
    let target = dir.path().join("backup");
    assert!(matches!(
        backup(
            shared(&store),
            target.clone(),
            Options {
                max_bytes: 1,
                ..Options::default()
            },
            CancellationToken::new()
        )
        .await,
        Err(Error::Archive)
    ));
    assert!(!target.exists());
    backup(
        shared(&store),
        target.clone(),
        Options::default(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let before = std::fs::read(&target).unwrap();
    assert!(
        backup(
            shared(&store),
            target.clone(),
            Options::default(),
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read(&target).unwrap(), before);
    assert!(
        restore(
            target.clone(),
            source,
            Options::default(),
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    let mut damaged = before;
    let last = damaged.len() - 1;
    damaged[last] ^= 1;
    std::fs::write(&target, damaged).unwrap();
    let new = dir.path().join("new");
    assert!(
        restore(
            target,
            new.clone(),
            Options::default(),
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    assert!(!new.exists());
    assert!(!dir.path().join("new.yoink-lock").exists());
}

#[tokio::test]
async fn dropped_caller_and_restore_cancellation_release_only_owned_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let store = prepared(&dir.path().join("source.db")).await;
    let target = dir.path().join("backup");
    let pause = Pause::new(Stage::Publish);
    let caller = tokio::spawn(backup(
        shared(&store),
        target.clone(),
        Options {
            pause: Some(pause.clone()),
            ..Options::default()
        },
        CancellationToken::new(),
    ));
    bounded(pause.entered.notified()).await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    pause.resume.notify_one();
    bounded(async {
        while dir.path().join("backup.yoink-lock").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(!target.exists());
    backup(
        shared(&store),
        target.clone(),
        Options::default(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let restored = dir.path().join("restored");
    let pause = Pause::new(Stage::Publish);
    let cancel = CancellationToken::new();
    let operation = tokio::spawn(restore(
        target.clone(),
        restored.clone(),
        Options {
            pause: Some(pause.clone()),
            ..Options::default()
        },
        cancel.clone(),
    ));
    bounded(pause.entered.notified()).await;
    cancel.cancel();
    pause.resume.notify_one();
    assert!(matches!(operation.await.unwrap(), Err(Error::Cancelled)));
    assert!(!restored.exists());
    assert!(!dir.path().join("restored.yoink-lock").exists());
    restore(
        target,
        restored,
        Options::default(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
}

#[test]
#[ignore = "subprocess fixture, launched by process_crash_preserves_source_and_never_publishes_partial_data"]
fn operation_process_child() {
    let root = PathBuf::from(std::env::var_os("YOINK_OPS_TEST_ROOT").expect("fixture root"));
    let mode = std::env::var("YOINK_OPS_TEST_MODE").unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let pause = Pause::new(Stage::Publish);
        let options = Options {
            pause: Some(pause.clone()),
            ..Options::default()
        };
        let target = root.join("target");
        let worker_root = root.clone();
        let action = tokio::spawn(async move {
            if mode == "backup" {
                let store = Store::open(
                    &worker_root.join("source.db"),
                    false,
                    Duration::from_secs(5),
                )
                .await
                .unwrap();
                backup(store, target, options, CancellationToken::new()).await
            } else {
                restore(
                    worker_root.join("archive"),
                    target,
                    options,
                    CancellationToken::new(),
                )
                .await
            }
        });
        pause.entered.notified().await;
        tokio::fs::write(root.join("ready"), b"before publication")
            .await
            .unwrap();
        if std::env::var_os("YOINK_OPS_TEST_PUBLISH").is_some() {
            pause.resume.notify_one();
            action.await.unwrap().unwrap();
            tokio::fs::write(root.join("published"), b"complete")
                .await
                .unwrap();
        }
        std::future::pending::<()>().await;
    });
}

#[tokio::test]
async fn process_crash_preserves_source_and_never_publishes_partial_data() {
    use std::os::unix::process::ExitStatusExt;
    for mode in ["backup", "restore"] {
        for published in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("source.db");
            let store = prepared(&source).await;
            backup(
                shared(&store),
                dir.path().join("archive"),
                Options::default(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
            store.close().await.unwrap();
            drop(store);
            let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", "tests::operation_process_child", "--ignored"])
                .env("YOINK_OPS_TEST_ROOT", dir.path())
                .env("YOINK_OPS_TEST_MODE", mode)
                .kill_on_drop(true);
            if published {
                command.env("YOINK_OPS_TEST_PUBLISH", "1");
            }
            let mut child = command.spawn().unwrap();
            let marker = dir
                .path()
                .join(if published { "published" } else { "ready" });
            bounded(async {
                while !marker.exists() {
                    assert!(
                        child.try_wait().unwrap().is_none(),
                        "child exited before marker"
                    );
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
            child.kill().await.unwrap();
            assert_eq!(child.wait().await.unwrap().signal(), Some(9));
            let target = dir.path().join("target");
            assert_eq!(target.exists(), published);
            let source_store = Store::open(&source, false, Duration::from_secs(5))
                .await
                .unwrap();
            source_store.verify_schema().await.unwrap();
            assert_eq!(name(&source_store).await, "before");
            if published {
                let restored = if mode == "backup" {
                    let out = dir.path().join("verified");
                    restore(
                        target.clone(),
                        out.clone(),
                        Options::default(),
                        CancellationToken::new(),
                    )
                    .await
                    .unwrap();
                    out
                } else {
                    target.clone()
                };
                let restored = Store::open(&restored, false, Duration::from_secs(5))
                    .await
                    .unwrap();
                restored.verify_schema().await.unwrap();
                assert_eq!(name(&restored).await, "before");
            }
            assert!(
                backup(
                    shared(&source_store),
                    target,
                    Options::default(),
                    CancellationToken::new()
                )
                .await
                .is_err()
            );
            backup(
                shared(&source_store),
                dir.path().join("retry"),
                Options::default(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        }
    }
}

#[tokio::test]
async fn schema_drift_and_orphan_references_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let store = prepared(&dir.path().join("source.db")).await;
    exec(&store,"PRAGMA foreign_keys=OFF; INSERT INTO inbox_user_settings(user_id) VALUES(999); PRAGMA foreign_keys=ON;").await;
    let target = dir.path().join("backup");
    assert!(matches!(
        backup(
            shared(&store),
            target.clone(),
            Options::default(),
            CancellationToken::new()
        )
        .await,
        Err(Error::ForeignKey)
    ));
    exec(
        &store,
        "DELETE FROM inbox_user_settings WHERE user_id=999; CREATE TABLE drift(id INTEGER);",
    )
    .await;
    assert!(matches!(
        backup(
            shared(&store),
            target.clone(),
            Options::default(),
            CancellationToken::new()
        )
        .await,
        Err(Error::Schema)
    ));
    assert!(!target.exists());
}
