use super::*;
use yoink_downloads::TelegramSubmission;
fn target(actor: u64) -> DeliveryTarget {
    DeliveryTarget::new(BOT, actor as i64, None, None).unwrap()
}
fn url(actor: u64, id: i32, mode: MediaMode) -> DownloadRequest {
    DownloadRequest::new(actor, URL, mode, None, DownloadParams::default())
        .unwrap()
        .with_target(DeliveryTarget::new(BOT, actor as i64, None, Some(id)).unwrap())
        .unwrap()
}
async fn submit(
    service: &DownloadService,
    request: &DownloadRequest,
    id: i32,
    plain: bool,
) -> yoink_storage::Admission {
    match service
        .submit_telegram_media(request, id, DownloadPreferenceOverrides::default(), plain)
        .await
        .unwrap()
    {
        TelegramSubmission::Admitted(a) => a,
        _ => panic!("unexpected quality"),
    }
}
#[tokio::test]
async fn audio_image_one_shot_video_reset_and_original_replay() {
    let (_dir, store, service) = setup_with_quotas(
        None,
        yoink_storage::QuotaLimits {
            minute: 20,
            hour: 30,
            day: 100,
        },
    )
    .await;
    yoink_db::raw_sql("UPDATE users SET role='user' WHERE id=42")
        .execute(store.database())
        .await
        .unwrap();
    for (index, mode) in [MediaMode::Audio, MediaMode::Image].into_iter().enumerate() {
        let id = 100 + index as i32 * 10;
        service
            .arm_media(ACTOR, target(ACTOR), id, mode)
            .await
            .unwrap();
        let request = url(ACTOR, id + 1, MediaMode::Video);
        let first = submit(&service, &request, id + 1, true).await;
        let frozen = persisted_request(&store, &first.operation_id).await;
        assert_eq!(
            frozen["media_mode"],
            if mode == MediaMode::Audio {
                "audio"
            } else {
                "image"
            }
        );
        service
            .arm_media(ACTOR, target(ACTOR), id + 2, MediaMode::Video)
            .await
            .unwrap();
        let replay = submit(&service, &request, id + 1, true).await;
        let changed = DownloadRequest::new(
            ACTOR,
            &format!("{URL}&changed=true"),
            MediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .unwrap()
        .with_target(*request.target().unwrap())
        .unwrap();
        assert!(matches!(
            service
                .submit_telegram_media(
                    &changed,
                    id + 1,
                    DownloadPreferenceOverrides::default(),
                    true
                )
                .await,
            Err(DownloadError::Conflict)
        ));
        assert!(!replay.created);
        assert_eq!(first.operation_id, replay.operation_id);
        assert_eq!(persisted_request(&store, &first.operation_id).await, frozen);
        let next = submit(
            &service,
            &url(ACTOR, id + 3, MediaMode::Video),
            id + 3,
            true,
        )
        .await;
        assert_eq!(
            persisted_request(&store, &next.operation_id).await["media_mode"],
            "video"
        );
        let default = submit(
            &service,
            &url(ACTOR, id + 4, MediaMode::Video),
            id + 4,
            true,
        )
        .await;
        assert_eq!(
            persisted_request(&store, &default.operation_id).await["media_mode"],
            "video"
        );
    }
    service
        .arm_media(ACTOR, target(ACTOR), 200, MediaMode::Audio)
        .await
        .unwrap();
    service
        .arm_media(ACTOR, target(ACTOR), 201, MediaMode::Video)
        .await
        .unwrap();
    let a = submit(&service, &url(ACTOR, 202, MediaMode::Video), 202, true).await;
    assert_eq!(
        persisted_request(&store, &a.operation_id).await["media_mode"],
        "video"
    );
}
#[tokio::test]
async fn explicit_override_duplicate_older_commands_and_conflict_preserve_pending() {
    let (_dir, store, service) = setup().await;
    service
        .arm_media(ACTOR, target(ACTOR), 10, MediaMode::Audio)
        .await
        .unwrap();
    let a = submit(&service, &url(ACTOR, 11, MediaMode::Image), 11, false).await;
    assert_eq!(
        persisted_request(&store, &a.operation_id).await["media_mode"],
        "image"
    );
    service
        .arm_media(ACTOR, target(ACTOR), 10, MediaMode::Audio)
        .await
        .unwrap();
    assert!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode
            .is_none()
    );
    service
        .arm_media(ACTOR, target(ACTOR), 20, MediaMode::Audio)
        .await
        .unwrap();
    service
        .arm_media(ACTOR, target(ACTOR), 19, MediaMode::Image)
        .await
        .unwrap();
    assert_eq!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode,
        Some(MediaMode::Audio)
    );
    assert!(matches!(
        service
            .submit_telegram_media(
                &url(ACTOR, 11, MediaMode::Video),
                11,
                DownloadPreferenceOverrides::default(),
                true
            )
            .await,
        Err(DownloadError::Conflict)
    ));
    assert_eq!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode,
        Some(MediaMode::Audio)
    );
    assert!(
        service
            .submit_telegram_media(
                &url(ACTOR, 21, MediaMode::Video),
                0,
                DownloadPreferenceOverrides::default(),
                true
            )
            .await
            .is_err()
    );
    assert_eq!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode,
        Some(MediaMode::Audio)
    );
}
#[tokio::test]
async fn scope_and_authorization_revocation_preserve_state() {
    let (_dir, store, service) = setup().await;
    service
        .arm_media(ACTOR, target(ACTOR), 10, MediaMode::Audio)
        .await
        .unwrap();
    for bad in [
        DeliveryTarget::new(BOT, -42, None, None).unwrap(),
        target(43),
        DeliveryTarget::new(BOT + 1, 42, None, None).unwrap(),
    ] {
        assert!(
            service
                .arm_media(ACTOR, bad, 11, MediaMode::Image)
                .await
                .is_err()
        );
    }
    let other = submit(&service, &url(43, 12, MediaMode::Video), 12, true).await;
    assert_eq!(
        persisted_request(&store, &other.operation_id).await["media_mode"],
        "video"
    );
    assert!(
        store
            .deferred_media_state(ACTOR, DeliveryTarget::new(BOT + 1, 42, None, None).unwrap())
            .await
            .unwrap()
            .is_none()
    );
    yoink_db::raw_sql(include_str!("../deferred_media_ban.sql"))
        .execute(store.database())
        .await
        .unwrap();
    assert!(matches!(
        service
            .arm_media(ACTOR, target(ACTOR), 20, MediaMode::Image)
            .await,
        Err(DownloadError::Denied)
    ));
    assert!(matches!(
        service
            .submit_telegram_media(
                &url(ACTOR, 21, MediaMode::Video),
                21,
                DownloadPreferenceOverrides::default(),
                true
            )
            .await,
        Err(DownloadError::Denied)
    ));
    assert_eq!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode,
        Some(MediaMode::Audio)
    );
}
#[tokio::test]
async fn concurrent_new_urls_consume_once_with_nsfw_and_exact_replay_charge() {
    let (_dir, store, service) = setup().await;
    service
        .arm_media(ACTOR, target(ACTOR), 10, MediaMode::Audio)
        .await
        .unwrap();
    service
        .mark_next_nsfw(ACTOR, target(ACTOR), 10)
        .await
        .unwrap();
    let r1 = url(ACTOR, 11, MediaMode::Video);
    let r2 = url(ACTOR, 12, MediaMode::Video);
    let (a, b) = tokio::join!(
        submit(&service, &r1, 11, true),
        submit(&service, &r2, 12, true)
    );
    let ra = persisted_request(&store, &a.operation_id).await;
    let rb = persisted_request(&store, &b.operation_id).await;
    assert_eq!(
        [&ra, &rb]
            .into_iter()
            .filter(|r| r["media_mode"] == "audio")
            .count(),
        1
    );
    assert_eq!(
        [&ra, &rb]
            .into_iter()
            .filter(|r| r["force_nsfw"] == true)
            .count(),
        1
    );
    for frozen in [&ra, &rb] {
        assert_eq!(
            frozen["media_mode"] == "audio",
            frozen["force_nsfw"] == true
        );
    }
    let r = url(ACTOR, 13, MediaMode::Video);
    let (a, b) = tokio::join!(
        submit(&service, &r, 13, true),
        submit(&service, &r, 13, true)
    );
    assert_eq!(a.operation_id, b.operation_id);
    assert_ne!(a.created, b.created);
    let n: i64 = yoink_db::query(include_str!("../deferred_media_charge.sql"))
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(n, 3);
}
#[tokio::test]
async fn quota_rejection_preserves_both_arms() {
    let (_dir, store, service) = setup_with_quotas(
        None,
        yoink_storage::QuotaLimits {
            minute: 1,
            hour: 2,
            day: 3,
        },
    )
    .await;
    submit(&service, &url(ACTOR, 1, MediaMode::Video), 1, true).await;
    service
        .arm_media(ACTOR, target(ACTOR), 10, MediaMode::Image)
        .await
        .unwrap();
    service
        .mark_next_nsfw(ACTOR, target(ACTOR), 10)
        .await
        .unwrap();
    assert!(
        service
            .submit_telegram_media(
                &url(ACTOR, 11, MediaMode::Video),
                11,
                DownloadPreferenceOverrides::default(),
                true
            )
            .await
            .is_err()
    );
    assert_eq!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode,
        Some(MediaMode::Image)
    );
    let count: i64 = yoink_db::query(include_str!("../nsfw_mark_state.sql"))
        .bind(ACTOR as i64)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(count, 1);
}
#[tokio::test]
async fn restart_preserves_arm_and_queued_snapshot() {
    let (dir, store, service) = setup().await;
    service
        .arm_media(ACTOR, target(ACTOR), 10, MediaMode::Image)
        .await
        .unwrap();
    drop(service);
    store.close().await.unwrap();
    drop(store);
    let store = Arc::new(
        Store::open(
            &dir.path().join("preferences.db"),
            false,
            Duration::from_secs(2),
        )
        .await
        .unwrap(),
    );
    store.verify_schema().await.unwrap();
    assert_eq!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode,
        Some(MediaMode::Image)
    );
    // Construct the production service over the reopened database, no in-memory arm.
    let executor = Executor::new(ExecutorConfig {
        program: "/bin/false".into(),
        args: vec![],
        workspace_root: dir.path().join("workspace"),
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
        Arc::new(IdentityStore::new(store.database().clone(), None, false)),
        executor,
        Arc::new(PanicDelivery),
        DownloadConfig::default(),
    )
    .unwrap();
    let a = submit(&service, &url(ACTOR, 11, MediaMode::Video), 11, true).await;
    drop(service);
    store.close().await.unwrap();
    drop(store);
    let store = Store::open(
        &dir.path().join("preferences.db"),
        false,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    let claim = store
        .claim("restart", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.operation_id, a.operation_id);
    assert_eq!(claim.request.media_mode(), MediaMode::Image);
    assert!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode
            .is_none()
    );
}
#[tokio::test]
async fn capacity_rejection_preserves_intent() {
    let (_dir, store, service) = setup_with_quotas(
        None,
        yoink_storage::QuotaLimits {
            minute: 20,
            hour: 30,
            day: 100,
        },
    )
    .await;
    for id in 1..=10 {
        submit(&service, &url(ACTOR, id, MediaMode::Video), id, true).await;
    }
    service
        .arm_media(ACTOR, target(ACTOR), 20, MediaMode::Image)
        .await
        .unwrap();
    assert!(matches!(
        service
            .submit_telegram_media(
                &url(ACTOR, 21, MediaMode::Video),
                21,
                DownloadPreferenceOverrides::default(),
                true
            )
            .await,
        Err(DownloadError::Capacity)
    ));
    assert_eq!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode,
        Some(MediaMode::Image)
    );
}
#[tokio::test]
async fn user_quality_draft_preserves_arm_until_confirmation_and_replay() {
    let (_dir, store, service) = setup_with_owner(Some(43)).await;
    yoink_db::raw_sql("UPDATE users SET role='user' WHERE id=42")
        .execute(store.database())
        .await
        .unwrap();
    let change =
        yoink_downloads::SettingChange::new(yoink_downloads::SettingField::Quality, "ask").unwrap();
    service
        .change_setting(ACTOR, target(ACTOR), change, None)
        .await
        .unwrap();
    service
        .arm_media(ACTOR, target(ACTOR), 10, MediaMode::Video)
        .await
        .unwrap();
    let r = url(ACTOR, 11, MediaMode::Video);
    let TelegramSubmission::Quality(session) = service
        .submit_telegram_media(&r, 11, DownloadPreferenceOverrides::default(), true)
        .await
        .unwrap()
    else {
        panic!("expected quality")
    };
    assert_eq!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode,
        Some(MediaMode::Video)
    );
    let a = service
        .quality_confirm(
            ACTOR,
            target(ACTOR),
            session.id,
            session.revision,
            yoink_downloads::QualityChoice::parse("720").unwrap(),
        )
        .await
        .unwrap();
    assert!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode
            .is_none()
    );
    service
        .arm_media(ACTOR, target(ACTOR), 20, MediaMode::Image)
        .await
        .unwrap();
    assert_eq!(
        submit(&service, &r, 11, true).await.operation_id,
        a.operation_id
    );
}
#[tokio::test]
async fn insertion_failure_rolls_back_media_nsfw_and_quota() {
    let (_dir, store, service) = setup().await;
    service
        .arm_media(ACTOR, target(ACTOR), 10, MediaMode::Audio)
        .await
        .unwrap();
    service
        .mark_next_nsfw(ACTOR, target(ACTOR), 10)
        .await
        .unwrap();
    yoink_db::raw_sql(include_str!(
        "../../../yoink-storage/sql/test_nsfw_mark_fail_insert.sql"
    ))
    .execute(store.database())
    .await
    .unwrap();
    assert!(matches!(
        service
            .submit_telegram_media(
                &url(ACTOR, 11, MediaMode::Video),
                11,
                DownloadPreferenceOverrides::default(),
                true
            )
            .await,
        Err(DownloadError::Database)
    ));
    assert_eq!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode,
        Some(MediaMode::Audio)
    );
    yoink_db::raw_sql(include_str!(
        "../../../yoink-storage/sql/test_nsfw_mark_drop_trigger.sql"
    ))
    .execute(store.database())
    .await
    .unwrap();
    let a = submit(&service, &url(ACTOR, 11, MediaMode::Video), 11, true).await;
    let frozen = persisted_request(&store, &a.operation_id).await;
    assert_eq!(frozen["media_mode"], "audio");
    assert_eq!(frozen["force_nsfw"], true);
    let n: i64 = yoink_db::query(include_str!("../deferred_media_charge.sql"))
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(n, 1);
}
#[tokio::test]
async fn older_url_cannot_consume_newer_command_and_role_revocation_blocks_use() {
    let (_dir, store, service) = setup().await;
    service
        .arm_media(ACTOR, target(ACTOR), 20, MediaMode::Image)
        .await
        .unwrap();
    let a = submit(&service, &url(ACTOR, 19, MediaMode::Video), 19, true).await;
    assert_eq!(
        persisted_request(&store, &a.operation_id).await["media_mode"],
        "video"
    );
    assert_eq!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode,
        Some(MediaMode::Image)
    );
    yoink_db::raw_sql(include_str!("../deferred_media_restrict.sql"))
        .execute(store.database())
        .await
        .unwrap();
    assert!(matches!(
        service
            .submit_telegram_media(
                &url(ACTOR, 21, MediaMode::Video),
                21,
                DownloadPreferenceOverrides::default(),
                true
            )
            .await,
        Err(DownloadError::Denied)
    ));
    assert!(matches!(
        service
            .arm_media(ACTOR, target(ACTOR), 22, MediaMode::Audio)
            .await,
        Err(DownloadError::Denied)
    ));
    assert_eq!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode,
        Some(MediaMode::Image)
    );
}
struct BotDelivery(i64);
impl Delivery for BotDelivery {
    fn bot_id(&self) -> i64 {
        self.0
    }
    fn deliver(&self, _: DeliveryTarget, _: bool, _: Vec<DeliveryArtifact>) -> DeliveryFuture<'_> {
        panic!("admission only")
    }
}
#[tokio::test]
async fn other_bot_and_group_admissions_do_not_observe_or_consume_private_arm() {
    let (dir, store, service) = setup().await;
    service
        .arm_media(ACTOR, target(ACTOR), 10, MediaMode::Audio)
        .await
        .unwrap();
    let executor = Executor::new(ExecutorConfig {
        program: "/bin/false".into(),
        args: vec![],
        workspace_root: dir.path().join("workspace"),
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
    let other = DownloadService::new(
        store.clone(),
        Arc::new(IdentityStore::new(store.database().clone(), None, false)),
        executor,
        Arc::new(BotDelivery(BOT + 1)),
        DownloadConfig::default(),
    )
    .unwrap();
    let r = url(ACTOR, 11, MediaMode::Video)
        .with_target(DeliveryTarget::new(BOT + 1, 42, None, Some(11)).unwrap())
        .unwrap();
    let a = submit(&other, &r, 11, true).await;
    assert_eq!(
        persisted_request(&store, &a.operation_id).await["media_mode"],
        "video"
    );
    assert_eq!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode,
        Some(MediaMode::Audio)
    );
    let actor = 8000000000011;
    let group = DeliveryTarget::new(BOT, -8000000000011, None, Some(12)).unwrap();
    let service = service.with_group_access(true);
    service
        .arm_media(actor, target(actor), 10, MediaMode::Image)
        .await
        .unwrap();
    assert!(
        service
            .arm_media(actor, group, 11, MediaMode::Audio)
            .await
            .is_err()
    );
    let r = url(actor, 12, MediaMode::Video).with_target(group).unwrap();
    let a = submit(&service, &r, 12, true).await;
    assert_eq!(
        persisted_request(&store, &a.operation_id).await["media_mode"],
        "video"
    );
    assert_eq!(
        store
            .deferred_media_state(actor, target(actor))
            .await
            .unwrap()
            .unwrap()
            .mode,
        Some(MediaMode::Image)
    );
}

#[tokio::test]
async fn superseded_quality_original_is_removed_without_losing_operation_replay() {
    let (_dir, store, service) = setup_with_owner(Some(ACTOR as i64)).await;
    let change =
        yoink_downloads::SettingChange::new(yoink_downloads::SettingField::Quality, "ask").unwrap();
    service
        .change_setting(ACTOR, target(ACTOR), change, None)
        .await
        .unwrap();
    service
        .arm_media(ACTOR, target(ACTOR), 10, MediaMode::Video)
        .await
        .unwrap();
    let original_request = url(ACTOR, 11, MediaMode::Video);
    let TelegramSubmission::Quality(first) = service
        .submit_telegram_media(
            &original_request,
            11,
            DownloadPreferenceOverrides::default(),
            true,
        )
        .await
        .unwrap()
    else {
        panic!("expected quality draft")
    };
    let admission = service
        .quality_confirm(
            ACTOR,
            target(ACTOR),
            first.id,
            first.revision,
            yoink_downloads::QualityChoice::Best,
        )
        .await
        .unwrap();
    let frozen = persisted_request(&store, &admission.operation_id).await;
    assert!(
        store
            .quality_media_intent(first.id)
            .await
            .unwrap()
            .is_some()
    );
    let TelegramSubmission::Quality(replacement) = service
        .submit_telegram_media(
            &url(ACTOR, 12, MediaMode::Video),
            12,
            DownloadPreferenceOverrides::default(),
            true,
        )
        .await
        .unwrap()
    else {
        panic!("expected replacement draft")
    };
    assert_ne!(replacement.id, first.id);
    assert!(
        store
            .quality_media_intent(first.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .quality_media_intent(replacement.id)
            .await
            .unwrap()
            .is_some()
    );
    let replay = submit(&service, &original_request, 11, true).await;
    assert_eq!(replay.operation_id, admission.operation_id);
    assert!(!replay.created);
    assert_eq!(
        persisted_request(&store, &admission.operation_id).await,
        frozen
    );
    assert_eq!(
        store
            .quality_current(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .id,
        replacement.id
    );
}

#[tokio::test]
async fn user_superseded_quality_cut_original_still_supports_admission_and_replay() {
    let (_dir, store, service) = setup_with_owner(Some(43)).await;
    yoink_db::raw_sql("UPDATE users SET role='user' WHERE id=42")
        .execute(store.database())
        .await
        .unwrap();
    let change =
        yoink_downloads::SettingChange::new(yoink_downloads::SettingField::Quality, "ask").unwrap();
    service
        .change_setting(ACTOR, target(ACTOR), change, None)
        .await
        .unwrap();
    service
        .arm_media(ACTOR, target(ACTOR), 10, MediaMode::Video)
        .await
        .unwrap();
    let clipped = DownloadRequest::new(
        ACTOR,
        URL,
        MediaMode::Video,
        Some(yoink_domain::ClipRange::new(0.2, 0.7).unwrap()),
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(*url(ACTOR, 11, MediaMode::Video).target().unwrap())
    .unwrap();
    let TelegramSubmission::Quality(first) = service
        .submit_telegram_media(&clipped, 11, DownloadPreferenceOverrides::default(), true)
        .await
        .unwrap()
    else {
        panic!("expected quality draft")
    };
    let cut = service
        .quality_cut(ACTOR, target(ACTOR), first.id, first.revision)
        .await
        .unwrap();
    let TelegramSubmission::Quality(replacement) = service
        .submit_telegram_media(
            &url(ACTOR, 12, MediaMode::Video),
            12,
            DownloadPreferenceOverrides::default(),
            true,
        )
        .await
        .unwrap()
    else {
        panic!("expected replacement draft")
    };
    assert_ne!(first.id, replacement.id);
    assert!(
        store
            .quality_media_intent(first.id)
            .await
            .unwrap()
            .is_some()
    );
    let yoink_downloads::CutSubmission::Quality(chooser) = service
        .cut_submit(ACTOR, target(ACTOR), cut.id, cut.revision)
        .await
        .unwrap()
    else {
        panic!("expected cut quality chooser")
    };
    let selected = service
        .cut_choose(
            ACTOR,
            target(ACTOR),
            chooser.id,
            chooser.revision,
            yoink_downloads::QualityChoice::Height(720),
        )
        .await
        .unwrap();
    let admission = service
        .cut_confirm(ACTOR, target(ACTOR), selected.id, selected.revision)
        .await
        .unwrap();
    assert!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode
            .is_none(),
        "old cut must still consume the prior arm through its original intent"
    );
    service
        .arm_media(ACTOR, target(ACTOR), 20, MediaMode::Image)
        .await
        .unwrap();
    let replay = service
        .cut_confirm(ACTOR, target(ACTOR), selected.id, selected.revision)
        .await
        .unwrap();
    assert_eq!(replay.operation_id, admission.operation_id);
    assert!(!replay.created);
    assert_eq!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode,
        Some(MediaMode::Image)
    );
    let original_intent = store.quality_media_intent(first.id).await.unwrap().unwrap();
    let frozen = persisted_request(&store, &admission.operation_id).await;
    let replacement_cut = service
        .quality_cut(ACTOR, target(ACTOR), replacement.id, replacement.revision)
        .await
        .unwrap();
    assert_ne!(replacement_cut.id, selected.id);
    assert!(
        store
            .quality_media_intent(first.id)
            .await
            .unwrap()
            .is_none(),
        "the last cut reference has now been replaced"
    );
    assert!(
        store
            .quality_media_intent(replacement.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        service
            .cut_confirm(ACTOR, target(ACTOR), selected.id, selected.revision)
            .await,
        Err(DownloadError::Conflict)
    ));
    // Cut-session replay is now fenced, but its immutable operation original and
    // worker request must remain intact: lifecycle cleanup touches neither.
    let key = format!("cut:{}:{}:{}", BOT, ACTOR, selected.id.simple());
    let replay = store
        .replay_telegram_media(&original_intent, &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.operation_id, admission.operation_id);
    assert!(!replay.created);
    assert_eq!(
        persisted_request(&store, &admission.operation_id).await,
        frozen
    );
    assert_eq!(
        store
            .deferred_media_state(ACTOR, target(ACTOR))
            .await
            .unwrap()
            .unwrap()
            .mode,
        Some(MediaMode::Image)
    );
}
