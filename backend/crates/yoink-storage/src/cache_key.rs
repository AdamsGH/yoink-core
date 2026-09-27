use sha2::{Digest, Sha256};
use yoink_domain::DownloadRequest;

use crate::{StoreError, codec};

/// A cache identity, not an authorization capability. Lookup still requires live access checks.
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct MediaCacheKey(String);

impl MediaCacheKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for MediaCacheKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MediaCacheKey([REDACTED])")
    }
}

/// Hash the canonical persisted request so new output parameters also enter the cache identity.
/// Delivery placement is excluded, but the actor and bot are mandatory partitions. Raw source
/// URLs and credential versions never appear in the returned key or its Debug representation.
pub fn media_cache_key(request: &DownloadRequest) -> Result<MediaCacheKey, StoreError> {
    let target = request.target().ok_or(StoreError::InvalidInput)?;
    let mut material = codec::encode(request)?;
    let object = material
        .as_object_mut()
        .ok_or(StoreError::InvalidStoredData)?;
    object.insert(
        "target".to_owned(),
        serde_json::json!({ "bot_id": target.bot_id() }),
    );
    let canonical = serde_json::to_vec(&material).map_err(|_| StoreError::InvalidStoredData)?;
    let mut hash = Sha256::new();
    hash.update(b"yoink:media-cache:v1\0");
    hash.update(canonical);
    Ok(MediaCacheKey(format!("v1:{:x}", hash.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoink_domain::{
        AudioCodec, ClipRange, Container, DeliveryTarget, DownloadParams, Engine, MediaMode,
        PlaylistRange, VideoCodec,
    };

    fn request(actor: u64, bot: i64, mode: MediaMode, params: DownloadParams) -> DownloadRequest {
        DownloadRequest::new(
            actor,
            "https://example.com/media?token=private-fixture",
            mode,
            None,
            params,
        )
        .unwrap()
        .with_target(DeliveryTarget::new(bot, actor as i64, None, None).unwrap())
        .unwrap()
    }

    #[test]
    fn partitions_actors_bots_modes_and_every_output_parameter() {
        let base = request(42, 700, MediaMode::Video, DownloadParams::default());
        let key = media_cache_key(&base).unwrap();
        assert_ne!(
            key,
            media_cache_key(&request(
                43,
                700,
                MediaMode::Video,
                DownloadParams::default()
            ))
            .unwrap()
        );
        assert_ne!(
            key,
            media_cache_key(&request(
                42,
                701,
                MediaMode::Video,
                DownloadParams::default()
            ))
            .unwrap()
        );
        for mode in [MediaMode::Audio, MediaMode::Image] {
            assert_ne!(
                key,
                media_cache_key(&request(42, 700, mode, DownloadParams::default())).unwrap()
            );
        }
        let variants = [
            DownloadParams::new(Some(720)).unwrap(),
            DownloadParams::default().with_video_codec(VideoCodec::Av1),
            DownloadParams::default().with_audio_codec(AudioCodec::Mp3),
            DownloadParams::default().with_container(Container::Mkv),
            DownloadParams::default().with_engine(Engine::Gallery),
            DownloadParams::default().with_playlist(Some(PlaylistRange::new(2, Some(3)).unwrap())),
            DownloadParams::default()
                .with_clips(vec![ClipRange::new(1.0, 2.0).unwrap()])
                .unwrap(),
            DownloadParams::default().with_send_as_file(true),
            DownloadParams::default().with_gallery_zip(true),
        ];
        for params in variants {
            assert_ne!(
                key,
                media_cache_key(&request(42, 700, MediaMode::Video, params)).unwrap()
            );
        }
        let clip = DownloadRequest::new(
            42,
            base.source_url().as_str(),
            MediaMode::Video,
            Some(ClipRange::new(3.0, 4.0).unwrap()),
            DownloadParams::default(),
        )
        .unwrap()
        .with_target(*base.target().unwrap())
        .unwrap();
        assert_ne!(key, media_cache_key(&clip).unwrap());
        let different_url = DownloadRequest::new(
            42,
            "https://example.com/other",
            MediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .unwrap()
        .with_target(*base.target().unwrap())
        .unwrap();
        assert_ne!(key, media_cache_key(&different_url).unwrap());
    }

    #[test]
    fn partitions_credential_identity_version_scope_and_proxy_source() {
        use yoink_domain::{
            CookieReference, CredentialVersion, NetworkSelection, ProxyReference, ProxySource,
        };
        let input = request(42, 700, MediaMode::Video, DownloadParams::default());
        let cookie = |id, owner, domain, pool, value: &str| {
            CookieReference::new(
                id,
                owner,
                domain,
                pool,
                CredentialVersion::new(value.repeat(64)).unwrap(),
            )
            .unwrap()
        };
        let key = |network| media_cache_key(&input.clone().with_network(network).unwrap()).unwrap();
        let baseline = NetworkSelection::new(Some(cookie(1, 42, "example.com", false, "a")), None);
        let original = key(baseline.clone());
        for changed in [
            NetworkSelection::NONE,
            NetworkSelection::new(Some(cookie(2, 42, "example.com", false, "a")), None),
            NetworkSelection::new(Some(cookie(1, 42, "example.com", false, "b")), None),
            NetworkSelection::new(Some(cookie(1, 42, "example.com", true, "a")), None),
            NetworkSelection::new(Some(cookie(1, 99, "example.com", true, "a")), None),
            NetworkSelection::new(Some(cookie(1, 42, "media.example.com", false, "a")), None),
        ] {
            assert_ne!(original, key(changed));
        }
        let proxy = |source, value: &str| {
            NetworkSelection::new(
                baseline.cookie().cloned(),
                Some(
                    ProxyReference::new(source, CredentialVersion::new(value.repeat(64)).unwrap())
                        .unwrap(),
                ),
            )
        };
        let system = key(proxy(ProxySource::System, "a"));
        assert_ne!(original, system);
        assert_ne!(system, key(proxy(ProxySource::System, "b")));
        assert_ne!(
            system,
            key(proxy(ProxySource::Personal { actor_id: 42 }, "a"))
        );
    }

    #[test]
    fn delivery_placement_does_not_change_media_but_bot_is_required() {
        let original = request(42, 700, MediaMode::Video, DownloadParams::default());
        let moved = original
            .clone()
            .with_target(DeliveryTarget::new(700, -100, Some(9), Some(21)).unwrap())
            .unwrap();
        assert_eq!(
            media_cache_key(&original).unwrap(),
            media_cache_key(&moved).unwrap()
        );
        let no_target = DownloadRequest::new(
            42,
            "https://example.com/",
            MediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .unwrap();
        assert_eq!(media_cache_key(&no_target), Err(StoreError::InvalidInput));
    }

    #[test]
    fn stable_key_contains_no_source_or_credential_text() {
        let input = request(42, 700, MediaMode::Video, DownloadParams::default());
        let key = media_cache_key(&input).unwrap();
        assert_eq!(key, media_cache_key(&input.clone()).unwrap());
        assert_eq!(key.as_str().len(), 67);
        assert!(key.as_str().starts_with("v1:"));
        assert!(
            key.as_str()[3..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
        assert!(!key.as_str().contains("private-fixture"));
        assert_eq!(format!("{key:?}"), "MediaCacheKey([REDACTED])");
    }
}
