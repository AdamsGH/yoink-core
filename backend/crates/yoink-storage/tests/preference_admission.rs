use std::time::Duration;

use yoink_domain::{
    ClipRange, DeliveryTarget, DownloadParams, DownloadPreferenceOverrides, DownloadRequest,
    MediaMode, PlaylistRange, VideoCodec,
};
use yoink_storage::{AdmissionPolicy, StoreError};

mod common;

const ACTOR: u64 = 8_000_000_000_501;
const BOT: i64 = 8_000_000_000_502;
const URL: &str = "https://example.com/media";

fn target() -> DeliveryTarget {
    DeliveryTarget::new(BOT, ACTOR as i64, None, None).unwrap()
}

fn request(
    url: &str,
    mode: MediaMode,
    params: DownloadParams,
    overrides: Option<DownloadPreferenceOverrides>,
) -> DownloadRequest {
    let request = DownloadRequest::new(ACTOR, url, mode, None, params)
        .unwrap()
        .with_target(target())
        .unwrap();
    match overrides {
        Some(overrides) => request.with_preference_overrides(overrides),
        None => request,
    }
}

fn preferred(height: u32) -> DownloadRequest {
    request(
        URL,
        MediaMode::Video,
        DownloadParams::new(Some(height)).unwrap(),
        Some(DownloadPreferenceOverrides::default()),
    )
}

fn policy() -> AdmissionPolicy {
    AdmissionPolicy {
        max_attempts: 2,
        retry_delay: Duration::ZERO,
        max_pending_global: 1000,
        max_pending_actor: 10,
    }
}

#[tokio::test]
async fn preference_intent_admission_keeps_first_resolution_and_rejects_changed_intent() {
    let store = common::store().await;
    let first_resolution = preferred(720);
    let second_resolution = preferred(1080);
    let key = "preferred-format-race";

    let (first, second) = tokio::join!(
        store.admit_network_snapshot(&first_resolution, key, policy()),
        store.admit_network_snapshot(&second_resolution, key, policy()),
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.operation_id, second.operation_id);
    assert_ne!(first.created, second.created);

    let expected_winner = if first.created {
        first_resolution
    } else {
        second_resolution
    };
    let replay = store
        .replay_network_admission(
            &request(
                URL,
                MediaMode::Video,
                DownloadParams::default(),
                Some(DownloadPreferenceOverrides::default()),
            ),
            key,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.operation_id, first.operation_id);
    assert!(!replay.created);
    let claim = store
        .claim_for_bot("preference-admission", Duration::from_secs(30), BOT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.operation_id, first.operation_id);
    assert_eq!(claim.request, expected_winner);
    assert_eq!(
        claim.request.params().max_height(),
        expected_winner.params().max_height()
    );
    assert_eq!(
        claim.request.network(),
        &yoink_domain::NetworkSelection::NONE
    );
    store.fail(&claim, false).await.unwrap();

    let explicit_override = DownloadPreferenceOverrides {
        max_height: Some(None),
        video_codec: Some(VideoCodec::Any),
        send_as_file: Some(false),
        ..Default::default()
    };

    let mut playlist_params = DownloadParams::default();
    playlist_params = playlist_params.with_playlist(Some(PlaylistRange::new(1, Some(2)).unwrap()));
    let playlist_changed = request(
        URL,
        MediaMode::Video,
        playlist_params,
        Some(DownloadPreferenceOverrides::default()),
    );
    let clipped = DownloadParams::default()
        .with_clips(vec![ClipRange::new(1.0, 2.0).unwrap()])
        .unwrap();
    let clip_changed = request(
        URL,
        MediaMode::Video,
        clipped,
        Some(DownloadPreferenceOverrides::default()),
    );
    let target_changed = DownloadRequest::new(
        ACTOR,
        URL,
        MediaMode::Video,
        None,
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(BOT + 1, ACTOR as i64, None, None).unwrap())
    .unwrap()
    .with_preference_overrides(DownloadPreferenceOverrides::default());

    let changed = [
        request(
            URL,
            MediaMode::Video,
            DownloadParams::default(),
            Some(DownloadPreferenceOverrides {
                max_height: Some(None),
                ..Default::default()
            }),
        ),
        request(
            URL,
            MediaMode::Video,
            DownloadParams::default(),
            Some(DownloadPreferenceOverrides {
                send_as_file: Some(false),
                ..Default::default()
            }),
        ),
        request(
            URL,
            MediaMode::Video,
            DownloadParams::default(),
            Some(explicit_override),
        ),
        request(
            "https://example.com/other",
            MediaMode::Video,
            DownloadParams::default(),
            Some(DownloadPreferenceOverrides::default()),
        ),
        request(
            URL,
            MediaMode::Audio,
            DownloadParams::default(),
            Some(DownloadPreferenceOverrides::default()),
        ),
        playlist_changed,
        clip_changed,
        target_changed,
        request(URL, MediaMode::Video, DownloadParams::default(), None),
    ];
    for candidate in changed {
        assert_eq!(
            store.replay_network_admission(&candidate, key).await,
            Err(StoreError::Conflict)
        );
        assert_eq!(
            store
                .admit_network_snapshot(&candidate, key, policy())
                .await,
            Err(StoreError::Conflict)
        );
    }
}
