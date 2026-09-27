use std::time::Duration;

use yoink_domain::{CachedMedia, CachedMediaKind};
use yoink_executor::ArtifactLimits;

pub(super) fn cache_read_enabled(ttl: Duration, bypassed: bool) -> bool {
    !ttl.is_zero() && !bypassed
}

pub(super) fn valid_cached_media(
    media: &[CachedMedia],
    limits: ArtifactLimits,
    send_as_file: bool,
) -> bool {
    if media.is_empty() || media.len() > limits.max_artifacts {
        return false;
    }
    let mut total = 0u64;
    for item in media {
        let size = item.size_bytes();
        if size < 1024
            || size > limits.max_artifact_bytes
            || (send_as_file && item.kind() != CachedMediaKind::Document)
        {
            return false;
        }
        let Some(next) = total.checked_add(size) else {
            return false;
        };
        if next > limits.max_workspace_bytes {
            return false;
        }
        total = next;
    }
    true
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MediaCategory {
    Visual,
    Audio,
    Document,
}

pub(super) fn upload_kind(media_type: &str, send_as_file: bool) -> CachedMediaKind {
    if send_as_file {
        return CachedMediaKind::Document;
    }
    match media_type {
        "image/jpeg" | "image/png" => CachedMediaKind::Photo,
        "video/mp4" => CachedMediaKind::Video,
        "audio/mpeg" | "audio/mp4" => CachedMediaKind::Audio,
        _ => CachedMediaKind::Document,
    }
}

pub(super) fn upload_metadata(
    media_type: &str,
    size_bytes: u64,
    send_as_file: bool,
) -> (CachedMediaKind, u64) {
    (upload_kind(media_type, send_as_file), size_bytes)
}

pub(super) fn category(kind: CachedMediaKind) -> MediaCategory {
    match kind {
        CachedMediaKind::Photo | CachedMediaKind::Video => MediaCategory::Visual,
        CachedMediaKind::Audio => MediaCategory::Audio,
        CachedMediaKind::Document => MediaCategory::Document,
    }
}

pub(super) fn build_cached_media(
    metadata: &[Option<(CachedMediaKind, u64)>],
    file_ids: Vec<Option<String>>,
) -> Option<Vec<CachedMedia>> {
    if metadata.is_empty() || metadata.len() != file_ids.len() {
        return None;
    }
    metadata
        .iter()
        .zip(file_ids)
        .map(|(metadata, file_id)| {
            let (kind, size) = metadata.as_ref().copied()?;
            CachedMedia::new(kind, file_id?, size).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> ArtifactLimits {
        ArtifactLimits {
            max_artifact_bytes: 10_000,
            max_workspace_bytes: 15_000,
            max_artifacts: 3,
        }
    }

    fn media(kind: CachedMediaKind, size: u64) -> CachedMedia {
        CachedMedia::new(kind, "opaque-id".into(), size).unwrap()
    }

    #[test]
    fn cache_read_predicate_respects_ttl_and_recovery_bypass() {
        assert!(!cache_read_enabled(Duration::ZERO, false));
        assert!(!cache_read_enabled(Duration::from_secs(1), true));
        assert!(cache_read_enabled(Duration::from_secs(1), false));
    }

    #[test]
    fn cached_media_validation_enforces_item_file_total_minimum_and_file_mode() {
        assert!(valid_cached_media(
            &[media(CachedMediaKind::Photo, 1024)],
            limits(),
            false
        ));
        assert!(!valid_cached_media(&[], limits(), false));
        assert!(!valid_cached_media(
            &[media(CachedMediaKind::Photo, 1023)],
            limits(),
            false
        ));
        assert!(!valid_cached_media(
            &[media(CachedMediaKind::Video, 10_001)],
            limits(),
            false
        ));
        assert!(!valid_cached_media(
            &[
                media(CachedMediaKind::Document, 8000),
                media(CachedMediaKind::Document, 8000)
            ],
            limits(),
            false,
        ));
        assert!(!valid_cached_media(
            &[
                media(CachedMediaKind::Document, 1024),
                media(CachedMediaKind::Document, 1024),
                media(CachedMediaKind::Document, 1024),
                media(CachedMediaKind::Document, 1024),
            ],
            limits(),
            false,
        ));
        assert!(!valid_cached_media(
            &[media(CachedMediaKind::Video, 1024)],
            limits(),
            true
        ));
        assert!(valid_cached_media(
            &[media(CachedMediaKind::Document, 1024)],
            limits(),
            true
        ));
    }

    #[test]
    fn cache_publication_validation_is_all_or_nothing() {
        let metadata = [
            Some((CachedMediaKind::Photo, 2048)),
            Some((CachedMediaKind::Video, 4096)),
        ];
        let valid = build_cached_media(
            &metadata,
            vec![Some("valid-one".into()), Some("valid-two".into())],
        )
        .unwrap();
        assert_eq!(valid.len(), 2);
        assert!(
            build_cached_media(
                &metadata,
                vec![Some("valid-one".into()), Some("invalid id".into())],
            )
            .is_none()
        );
        assert!(build_cached_media(&metadata, vec![Some("only-one".into())]).is_none());
    }

    #[test]
    fn source_kind_and_grouping_match_sender_mime_policy() {
        assert_eq!(
            upload_metadata("image/jpeg", 2048, false),
            (CachedMediaKind::Photo, 2048)
        );
        assert_eq!(upload_kind("image/jpeg", false), CachedMediaKind::Photo);
        assert_eq!(upload_kind("video/mp4", false), CachedMediaKind::Video);
        assert_eq!(upload_kind("audio/mpeg", false), CachedMediaKind::Audio);
        assert_eq!(upload_kind("video/webm", false), CachedMediaKind::Document);
        assert_eq!(upload_kind("image/png", true), CachedMediaKind::Document);
        assert_eq!(
            category(CachedMediaKind::Photo),
            category(CachedMediaKind::Video)
        );
        assert_ne!(
            category(CachedMediaKind::Audio),
            category(CachedMediaKind::Document)
        );
    }

    #[test]
    fn cached_debug_redacts_ids_and_constructor_rejects_bad_ids() {
        let item = media(CachedMediaKind::Photo, 1024);
        assert!(!format!("{item:?}").contains("opaque-id"));
        assert!(CachedMedia::new(CachedMediaKind::Photo, "bad id".into(), 1024).is_err());
    }
}
