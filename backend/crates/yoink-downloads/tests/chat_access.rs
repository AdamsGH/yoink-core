use std::{path::PathBuf, sync::Arc, time::Duration};
use yoink_access::Role;
use yoink_domain::{DeliveryTarget, DownloadParams, DownloadRequest, MediaMode};
use yoink_downloads::{
    Delivery, DeliveryArtifact, DeliveryFuture, DownloadConfig, DownloadError, DownloadService,
};
use yoink_executor::{Executor, ExecutorConfig};
use yoink_identity::IdentityStore;
use yoink_storage::Store;

const USER: u64 = 8000000000011;
const RESTRICTED: u64 = 8000000000012;
const GROUP: i64 = -8000000000011;
const BOT: i64 = 123456;
struct NoDelivery;
impl Delivery for NoDelivery {
    fn bot_id(&self) -> i64 {
        BOT
    }
    fn deliver(&self, _: DeliveryTarget, _: bool, _: Vec<DeliveryArtifact>) -> DeliveryFuture<'_> {
        panic!("admission tests must not deliver")
    }
}
fn target(chat: i64, thread: Option<i64>) -> DeliveryTarget {
    DeliveryTarget::new(BOT, chat, thread, Some(10)).unwrap()
}
fn request(actor: u64, target: DeliveryTarget) -> DownloadRequest {
    DownloadRequest::new(
        actor,
        "https://example.com/media.mp4",
        MediaMode::Video,
        None,
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(target)
    .unwrap()
}
async fn setup() -> (
    tempfile::TempDir,
    Arc<Store>,
    Arc<IdentityStore>,
    DownloadService,
) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let workspace = dir.path().join("worker");
    std::fs::create_dir(&workspace).unwrap();
    let store = Arc::new(
        Store::open(&dir.path().join("groups.db"), true, Duration::from_secs(2))
            .await
            .unwrap(),
    );
    store.migrate().await.unwrap();
    yoink_db::raw_sql(include_str!("../../yoink-identity/tests/fixtures.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let identities = Arc::new(IdentityStore::new(store.database().clone(), None, false));
    let executor = Executor::new(ExecutorConfig {
        program: PathBuf::from("/bin/sh"),
        args: vec![
            "-c".into(),
            "printf started > \"$1\"".into(),
            "fixture-worker".into(),
            dir.path().join("worker-started").to_str().unwrap().into(),
        ],
        workspace_root: workspace,
        timeout: Duration::from_secs(30),
        processing_timeout: Duration::from_secs(20),
        socket_timeout: Duration::from_secs(5),
        max_artifact_bytes: 1024 * 1024,
        max_workspace_bytes: 2 * 1024 * 1024,
        max_output_bytes: 1024 * 1024,
        max_artifacts: 1,
        allowed_proxy_origins: vec![],
        allowed_private_origins: vec![],
    })
    .unwrap();
    let service = DownloadService::new(
        store.clone(),
        identities.clone(),
        executor,
        Arc::new(NoDelivery),
        DownloadConfig::default(),
    )
    .unwrap();
    (dir, store, identities, service)
}

#[tokio::test]
async fn queued_jobs_recheck_group_topic_and_configuration_before_starting_a_worker() {
    for revoke in ["group", "topic", "configuration", "none"] {
        let (dir, store, _identities, service) = setup().await;
        let mut service = service.with_group_access(true);
        let id = service
            .submit_request(&request(USER, target(GROUP, Some(18))), "queued-group")
            .await
            .unwrap()
            .operation_id;
        match revoke {
            "group" => {
                yoink_db::query(include_str!("sql/set_group_enabled.sql"))
                    .bind(false)
                    .bind(GROUP)
                    .execute(store.database())
                    .await
                    .unwrap();
            }
            "topic" => {
                yoink_db::query(include_str!("sql/set_thread_enabled.sql"))
                    .bind(false)
                    .bind(GROUP)
                    .bind(18_i64)
                    .execute(store.database())
                    .await
                    .unwrap();
            }
            "configuration" => service = service.with_group_access(false),
            _ => {}
        }
        let shutdown = tokio_util::sync::CancellationToken::new();
        let token = shutdown.clone();
        let runner = tokio::spawn(async move { service.run(token).await });
        let marker = dir.path().join("worker-started");
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let record = store.get(USER, id).await.unwrap().unwrap();
                if marker.exists() || record.status == yoink_storage::OperationStatus::Failed {
                    break record;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(3), runner)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let record = result.expect("worker execution or terminal denial must be observable");
        assert_eq!(marker.exists(), revoke == "none", "revocation={revoke}");
        if revoke != "none" {
            assert_eq!(record.status, yoink_storage::OperationStatus::Failed);
            assert_eq!(record.attempt, 1);
        }
    }
}

#[tokio::test]
async fn groups_require_opt_in_and_fresh_group_thread_and_global_permissions() {
    let (_dir, store, identities, service) = setup().await;
    let permitted = request(RESTRICTED, target(GROUP, Some(18)));
    assert!(matches!(
        service.submit_request(&permitted, "group").await,
        Err(DownloadError::Denied)
    ));
    let service = service.with_group_access(true);
    for actor in [8000000000013, 8000000000014] {
        assert!(matches!(
            service
                .submit_request(&request(actor, target(GROUP, Some(18))), "blocked")
                .await,
            Err(DownloadError::Denied)
        ));
    }
    for destination in [
        target(GROUP, Some(17)),
        target(GROUP, Some(19)),
        target(-8000000000012, None),
    ] {
        assert!(matches!(
            service
                .submit_request(&request(USER, destination), "blocked-context")
                .await,
            Err(DownloadError::Denied)
        ));
    }
    let admitted = service.submit_request(&permitted, "group").await.unwrap();
    assert!(admitted.created);
    assert_eq!(
        identities
            .snapshot(RESTRICTED as i64, None)
            .await
            .unwrap()
            .unwrap()
            .principal
            .role,
        Role::Restricted
    );
    assert!(matches!(
        service
            .submit_request(
                &request(RESTRICTED, target(RESTRICTED as i64, None)),
                "private"
            )
            .await,
        Err(DownloadError::Denied)
    ));
    yoink_db::query(include_str!("sql/set_group_role.sql"))
        .bind(Some("restricted"))
        .bind(RESTRICTED as i64)
        .bind(GROUP)
        .execute(store.database())
        .await
        .unwrap();
    assert!(matches!(
        service.submit_request(&permitted, "group").await,
        Err(DownloadError::Denied)
    ));
    yoink_db::query(include_str!("sql/set_group_role.sql"))
        .bind(None::<&str>)
        .bind(RESTRICTED as i64)
        .bind(GROUP)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        service
            .submit_request(&permitted, "group")
            .await
            .unwrap()
            .operation_id,
        admitted.operation_id,
        "auto grant applies locally after removing an explicit override"
    );
    yoink_db::query(include_str!("sql/set_thread_enabled.sql"))
        .bind(false)
        .bind(GROUP)
        .bind(18_i64)
        .execute(store.database())
        .await
        .unwrap();
    assert!(matches!(
        service.status(RESTRICTED, admitted.operation_id).await,
        Err(DownloadError::Denied)
    ));
    assert!(matches!(
        service
            .register_progress_message(RESTRICTED, admitted.operation_id, 100, "en")
            .await,
        Err(DownloadError::Denied)
    ));
    yoink_db::query(include_str!("sql/set_thread_enabled.sql"))
        .bind(true)
        .bind(GROUP)
        .bind(18_i64)
        .execute(store.database())
        .await
        .unwrap();
    yoink_db::query(include_str!("sql/set_group_enabled.sql"))
        .bind(false)
        .bind(GROUP)
        .execute(store.database())
        .await
        .unwrap();
    assert!(matches!(
        service.cancel(RESTRICTED, admitted.operation_id).await,
        Err(DownloadError::Denied)
    ));
}

#[tokio::test]
async fn chat_queries_cannot_cross_actor_chat_topic_or_bot_boundaries() {
    let (_dir, _store, _identities, service) = setup().await;
    let service = service.with_group_access(true);
    let destination = target(GROUP, Some(18));
    let id = service
        .submit_request(&request(USER, destination), "group-query")
        .await
        .unwrap()
        .operation_id;
    service
        .register_progress_message(USER, id, 1001, "en")
        .await
        .unwrap();
    for message_id in [10, 1001] {
        assert_eq!(
            service
                .operation_by_reply(USER, destination, message_id)
                .await
                .unwrap()
                .unwrap()
                .operation_id,
            id
        );
        assert!(
            service
                .operation_by_reply(USER, target(GROUP, None), message_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            service
                .operation_by_reply(8000000000015, destination, message_id)
                .await
                .unwrap()
                .is_none()
        );
    }
    for current in [target(GROUP, None), target(USER as i64, None)] {
        assert!(
            service
                .status_in_chat(USER, id, current)
                .await
                .unwrap()
                .is_none()
        );
        assert!(!service.cancel_in_chat(USER, id, current).await.unwrap());
    }
    assert!(
        service
            .status_in_chat(8000000000015, id, destination)
            .await
            .unwrap()
            .is_none()
    );
    for invalid in [
        target(8000000000015, None),
        target(USER as i64, Some(18)),
        DeliveryTarget::new(BOT + 1, GROUP, Some(18), None).unwrap(),
    ] {
        assert!(matches!(
            service.status_in_chat(USER, id, invalid).await,
            Err(DownloadError::Denied)
        ));
    }
    let private_id = service
        .submit_request(&request(USER, target(USER as i64, None)), "private-query")
        .await
        .unwrap()
        .operation_id;
    assert!(
        service
            .status_in_chat(USER, private_id, destination)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !service
            .cancel_in_chat(USER, private_id, destination)
            .await
            .unwrap()
    );
    let reply_changed = DeliveryTarget::new(BOT, GROUP, Some(18), Some(99)).unwrap();
    assert!(
        service
            .status_in_chat(USER, id, reply_changed)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        service
            .cancel_in_chat(USER, id, reply_changed)
            .await
            .unwrap()
    );
    let disabled = service.with_group_access(false);
    assert!(matches!(
        disabled.operation_by_reply(USER, destination, 1001).await,
        Err(DownloadError::Denied)
    ));
    assert!(matches!(
        disabled.status(USER, id).await,
        Err(DownloadError::Denied)
    ));
    assert!(
        disabled
            .status_in_chat(USER, private_id, target(USER as i64, None))
            .await
            .unwrap()
            .is_some()
    );
}
