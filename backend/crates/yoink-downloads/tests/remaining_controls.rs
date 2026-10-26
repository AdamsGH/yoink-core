use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use yoink_domain::{DeliveryTarget, DownloadParams, DownloadRequest, MediaMode};
use yoink_downloads::{
    DeliveredMedia, Delivery, DeliveryArtifact, DeliveryFuture, DownloadConfig, DownloadError,
    DownloadService, SettingChange, SettingField,
};
use yoink_executor::{Executor, ExecutorConfig};
use yoink_identity::IdentityStore;
use yoink_storage::{OperationStatus, Store};

#[derive(Default)]
struct Recorder(
    Mutex<Vec<(String, bool)>>,
    std::sync::atomic::AtomicBool,
    tokio::sync::Notify,
    tokio::sync::Notify,
);
struct DiagnosticGuard<'a>(&'a Recorder);
impl Drop for DiagnosticGuard<'_> {
    fn drop(&mut self) {
        self.0.3.notify_one();
    }
}
impl Delivery for Recorder {
    fn deliver_diagnostic(
        &self,
        _: DeliveryTarget,
        _: yoink_worker_protocol::MediaInfoReport,
    ) -> yoink_downloads::DiagnosticFuture<'_> {
        Box::pin(async move {
            if self.1.load(std::sync::atomic::Ordering::SeqCst) {
                let _guard = DiagnosticGuard(self);
                self.2.notify_one();
                std::future::pending::<()>().await;
            }
        })
    }
    fn bot_id(&self) -> i64 {
        123
    }
    fn deliver(
        &self,
        _: DeliveryTarget,
        _: bool,
        artifacts: Vec<DeliveryArtifact>,
    ) -> DeliveryFuture<'_> {
        Box::pin(async move {
            let mut receipts = Vec::new();
            for a in artifacts {
                let DeliveryArtifact::Upload(a) = a else {
                    panic!("must not replay metadata output through cache")
                };
                self.0
                    .lock()
                    .unwrap()
                    .push((a.filename.clone(), a.media_info.is_some()));
                receipts.push(DeliveredMedia {
                    message_id: 77 + receipts.len() as i64,
                    file_id: None,
                });
            }
            Ok(receipts)
        })
    }
}
async fn fixture() -> (
    tempfile::TempDir,
    Arc<Store>,
    DownloadService,
    Arc<Recorder>,
) {
    fixture_config(DownloadConfig::default()).await
}
async fn fixture_config(
    config: DownloadConfig,
) -> (
    tempfile::TempDir,
    Arc<Store>,
    DownloadService,
    Arc<Recorder>,
) {
    let d = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let root = d.path().join("worker");
    std::fs::create_dir(&root).unwrap();
    let store = Arc::new(
        Store::open(&d.path().join("db"), true, Duration::from_secs(2))
            .await
            .unwrap(),
    );
    store.migrate().await.unwrap();
    yoink_db::raw_sql(include_str!("admission_users.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let executor = Executor::new(ExecutorConfig {
        program: "/usr/bin/env".into(),
        args: vec![
            "node".into(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/remaining_worker.mjs").into(),
        ],
        workspace_root: root,
        timeout: Duration::from_secs(30),
        processing_timeout: Duration::from_secs(20),
        socket_timeout: Duration::from_secs(5),
        max_artifact_bytes: 1024 * 1024,
        max_workspace_bytes: 2 * 1024 * 1024,
        max_output_bytes: 1024 * 1024,
        max_artifacts: 4,
        allowed_proxy_origins: vec![],
        allowed_private_origins: vec![],
    })
    .unwrap();
    let recorder = Arc::new(Recorder::default());
    let service = DownloadService::new(
        store.clone(),
        Arc::new(IdentityStore::new(
            store.database().clone(),
            Some(42),
            false,
        )),
        executor,
        recorder.clone(),
        config,
    )
    .unwrap();
    (d, store, service, recorder)
}
fn request(mode: MediaMode) -> DownloadRequest {
    let params = DownloadParams::default()
        .with_send_as_file(matches!(mode, MediaMode::Links | MediaMode::AudioLinks));
    DownloadRequest::new(43, "https://example.com/source", mode, None, params)
        .unwrap()
        .with_target(DeliveryTarget::new(123, 43, None, None).unwrap())
        .unwrap()
}
async fn finish(store: &Store, service: DownloadService, id: uuid::Uuid) {
    let stop = tokio_util::sync::CancellationToken::new();
    let token = stop.clone();
    let runner = tokio::spawn(async move { service.run(token).await });
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let r = store.get(43, id).await.unwrap().unwrap();
            if !matches!(
                r.status,
                OperationStatus::Queued | OperationStatus::Running | OperationStatus::Delivering
            ) {
                break r.status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    stop.cancel();
    runner.await.unwrap().unwrap();
    assert_eq!(result.unwrap(), OperationStatus::Succeeded);
}
#[tokio::test]
async fn playlist_entries_do_not_limit_clip_artifacts() {
    let (_d, store, service, recorder) = fixture().await;
    for (limit, playlist, expected) in [
        (1, None, 2),
        (
            2,
            Some(yoink_domain::PlaylistRange::new(1, Some(2)).unwrap()),
            4,
        ),
    ] {
        yoink_db::query(include_str!(
            "../../yoink-storage/sql/test_downloader_setting.sql"
        ))
        .bind("dl.max_playlist_count")
        .bind(limit.to_string())
        .execute(store.database())
        .await
        .unwrap();
        let params = DownloadParams::default()
            .with_clips(vec![
                yoink_domain::ClipRange::new(0.0, 1.0).unwrap(),
                yoink_domain::ClipRange::new(2.0, 3.0).unwrap(),
            ])
            .unwrap()
            .with_playlist(playlist);
        let req = request(MediaMode::Video).with_params(params).unwrap();
        let before = recorder.0.lock().unwrap().len();
        let operation = service
            .submit_request(&req, &format!("clips-{limit}"))
            .await
            .unwrap();
        finish(&store, service.clone(), operation.operation_id).await;
        assert_eq!(recorder.0.lock().unwrap().len() - before, expected);
    }
}

#[tokio::test]
async fn direct_links_use_real_admission_replay_permissions_and_no_cache() {
    let (_d, store, service, recorder) = fixture().await;
    for mode in [MediaMode::Links, MediaMode::AudioLinks] {
        let r = request(mode);
        let key = format!("{mode:?}");
        let a = service.submit_request(&r, &key).await.unwrap();
        assert_eq!(
            service.submit_request(&r, &key).await.unwrap().operation_id,
            a.operation_id
        );
        finish(&store, service.clone(), a.operation_id).await;
        assert_eq!(
            service.submit_request(&r, &key).await.unwrap().operation_id,
            a.operation_id
        );
        assert!(store.lookup_media_cache(&r).await.unwrap().is_none());
        let wrong = r
            .clone()
            .with_target(DeliveryTarget::new(123, -100, None, None).unwrap())
            .unwrap();
        assert!(matches!(
            service.submit_request(&wrong, "group").await,
            Err(DownloadError::Denied)
        ));
    }
    assert_eq!(
        *recorder.0.lock().unwrap(),
        vec![("links.json".into(), false), ("links.json".into(), false)]
    );
}
#[tokio::test]
async fn media_info_is_frozen_typed_and_ignored_for_replay_intent() {
    let (_d, store, service, recorder) = fixture().await;
    let target = DeliveryTarget::new(123, 43, None, None).unwrap();
    service
        .change_setting(
            43,
            target,
            SettingChange::new(SettingField::MediaInfo, "true").unwrap(),
            None,
        )
        .await
        .unwrap();
    let r = request(MediaMode::Video);
    let a = service.submit_request(&r, "report").await.unwrap();
    service
        .change_setting(
            43,
            target,
            SettingChange::new(SettingField::MediaInfo, "false").unwrap(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .submit_request(&r, "report")
            .await
            .unwrap()
            .operation_id,
        a.operation_id
    );
    finish(&store, service.clone(), a.operation_id).await;
    assert_eq!(
        *recorder.0.lock().unwrap(),
        vec![("media.mp4".into(), true)]
    );
    let b = service.submit_request(&r, "no-report").await.unwrap();
    finish(&store, service, b.operation_id).await;
    assert_eq!(recorder.0.lock().unwrap()[1], ("media.mp4".into(), false));
}
#[tokio::test]
async fn old_request_json_defaults_media_info_false_new_snapshot_survives_claim() {
    let (_d, store, service, _) = fixture().await;
    let r = request(MediaMode::Video);
    service.submit_request(&r, "old").await.unwrap();
    // Old persisted envelopes lack the new option; decoding remains deterministic.
    yoink_db::query(
        "UPDATE runtime_operations SET request = json_remove(request, '$.params.media_info')",
    )
    .execute(store.database())
    .await
    .unwrap();
    let claim = store
        .claim("fixture", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert!(!claim.request.params().media_info());
}

#[tokio::test]
async fn stalled_diagnostic_timeout_or_shutdown_never_changes_confirmed_receipt() {
    for cancel in [false, true] {
        let config = DownloadConfig {
            delivery_timeout: if cancel {
                Duration::from_secs(20)
            } else {
                Duration::from_millis(100)
            },
            ..Default::default()
        };
        let (_dir, store, service, recorder) = fixture_config(config).await;
        recorder.1.store(true, std::sync::atomic::Ordering::SeqCst);
        let target = DeliveryTarget::new(123, 43, None, None).unwrap();
        service
            .change_setting(
                43,
                target,
                SettingChange::new(SettingField::MediaInfo, "true").unwrap(),
                None,
            )
            .await
            .unwrap();
        let admission = service
            .submit_request(&request(MediaMode::Video), "stalled-report")
            .await
            .unwrap();
        let stop = tokio_util::sync::CancellationToken::new();
        let token = stop.clone();
        let runner = tokio::spawn(async move { service.run(token).await });
        tokio::time::timeout(Duration::from_secs(5), recorder.2.notified())
            .await
            .unwrap();
        // The report is stalled now, but the media receipt is already durable.
        let confirmed = store
            .get(43, admission.operation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(confirmed.status, OperationStatus::Succeeded);
        assert_eq!(confirmed.delivered_message_id, Some(77));
        if cancel {
            stop.cancel();
        }
        tokio::time::timeout(Duration::from_secs(5), recorder.3.notified())
            .await
            .unwrap();
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), runner)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let after = store
            .get(43, admission.operation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.status, OperationStatus::Succeeded);
        assert_eq!(after.delivered_message_id, Some(77));
    }
}
