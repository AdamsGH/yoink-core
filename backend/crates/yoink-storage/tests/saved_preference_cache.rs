use yoink_domain::{
    AudioCodec, DownloadParams, DownloadPreferenceOverrides, DownloadRequest, MediaMode, VideoCodec,
};

const ACTOR: u64 = 8_000_000_000_501;

fn request(params: DownloadParams, intent: DownloadPreferenceOverrides) -> DownloadRequest {
    DownloadRequest::new(
        ACTOR,
        "https://example.com/media",
        MediaMode::Video,
        None,
        params,
    )
    .unwrap()
    .with_target(yoink_domain::DeliveryTarget::new(123456, ACTOR as i64, None, None).unwrap())
    .unwrap()
    .with_preference_overrides(intent)
}

#[test]
fn cache_identity_uses_resolved_format_not_saved_preference_intent() {
    let default_intent = DownloadPreferenceOverrides::default();
    let explicit_same_result = DownloadPreferenceOverrides {
        max_height: Some(None),
        ..DownloadPreferenceOverrides::default()
    };
    let base = request(DownloadParams::default(), default_intent);
    let same_result = request(DownloadParams::default(), explicit_same_result);
    assert_eq!(
        yoink_storage::media_cache_key(&base).unwrap(),
        yoink_storage::media_cache_key(&same_result).unwrap()
    );

    let taller = request(
        DownloadParams::new(Some(720)).unwrap(),
        DownloadPreferenceOverrides::default(),
    );
    assert_ne!(
        yoink_storage::media_cache_key(&base).unwrap(),
        yoink_storage::media_cache_key(&taller).unwrap()
    );
    let other_codec = request(
        DownloadParams::default()
            .with_audio_codec(AudioCodec::Opus)
            .with_video_codec(VideoCodec::Vp9),
        DownloadPreferenceOverrides::default(),
    );
    assert_ne!(
        yoink_storage::media_cache_key(&base).unwrap(),
        yoink_storage::media_cache_key(&other_codec).unwrap()
    );
}
