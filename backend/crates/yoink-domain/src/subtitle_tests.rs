use super::*;

#[test]
fn subtitle_languages_are_exact_bounded_tags() {
    for language in ["en", "ru", "zh-Hans", "pt-BR", "eng", "all"] {
        let options = SubtitleOptions::new(language, true).unwrap();
        assert_eq!(options.language(), language);
        assert!(options.automatic());
    }
    for language in [
        "",
        "e",
        "en.*",
        "../../en",
        "en,ru",
        "RU",
        "en-US-extra",
        "abcd",
    ] {
        assert!(SubtitleOptions::new(language, false).is_err(), "{language}");
    }
}

#[test]
fn standalone_subtitle_requests_normalize_defaults_and_reject_media_options() {
    let request = DownloadRequest::new(
        42,
        "https://example.test/video",
        MediaMode::Subtitles,
        None,
        DownloadParams::default(),
    )
    .unwrap();
    assert_eq!(
        request.params().subtitles(),
        Some(&SubtitleOptions::default())
    );
    for params in [
        DownloadParams::new(Some(720)).unwrap(),
        DownloadParams::default().with_engine(Engine::Gallery),
        DownloadParams::default().with_gallery_zip(true),
        DownloadParams::default().with_playlist(Some(PlaylistRange::new(1, Some(2)).unwrap())),
    ] {
        assert!(request.clone().with_params(params).is_err());
    }
    assert!(
        DownloadRequest::new(
            42,
            "https://example.test/video",
            MediaMode::Subtitles,
            Some(ClipRange::new(1.0, 2.0).unwrap()),
            DownloadParams::default()
        )
        .is_err()
    );
    assert!(
        DownloadRequest::new(
            42,
            "https://example.test/video",
            MediaMode::Video,
            None,
            DownloadParams::default().with_subtitles(Some(SubtitleOptions::default()))
        )
        .is_err()
    );
}
