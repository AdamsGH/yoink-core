use std::fmt;

use thiserror::Error;

/// Telegram media representation retained for bounded file-ID reuse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CachedMediaKind {
    Photo,
    Video,
    Audio,
    Document,
}

/// A validated cached media reference. Debug output never includes Telegram's file ID.
#[derive(Clone, Eq, PartialEq)]
pub struct CachedMedia {
    kind: CachedMediaKind,
    file_id: String,
    size_bytes: u64,
    metadata: Option<crate::ContentMetadata>,
}

impl CachedMedia {
    pub const MAX_FILE_ID_BYTES: usize = 4096;
    pub const MAX_SIZE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

    pub fn new(
        kind: CachedMediaKind,
        file_id: String,
        size_bytes: u64,
    ) -> Result<Self, CachedMediaError> {
        if file_id.is_empty()
            || file_id.len() > Self::MAX_FILE_ID_BYTES
            // Bot API uses the same string field for file IDs, URLs and attachment
            // names. Only opaque base64 identifiers may enter the cache path.
            || !file_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'='))
        {
            return Err(CachedMediaError::InvalidFileId);
        }
        if size_bytes == 0 || size_bytes > Self::MAX_SIZE_BYTES {
            return Err(CachedMediaError::InvalidSize);
        }
        Ok(Self {
            kind,
            file_id,
            size_bytes,
            metadata: None,
        })
    }

    pub fn with_metadata(
        mut self,
        metadata: crate::ContentMetadata,
    ) -> Result<Self, CachedMediaError> {
        if !metadata.valid() {
            return Err(CachedMediaError::InvalidMetadata);
        }
        self.metadata = Some(metadata);
        Ok(self)
    }
    pub fn metadata(&self) -> Option<&crate::ContentMetadata> {
        self.metadata.as_ref()
    }

    pub const fn kind(&self) -> CachedMediaKind {
        self.kind
    }

    pub fn file_id(&self) -> &str {
        &self.file_id
    }

    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
}

impl fmt::Debug for CachedMedia {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CachedMedia")
            .field("kind", &self.kind)
            .field("file_id", &"[REDACTED]")
            .field("size_bytes", &self.size_bytes)
            .finish()
    }
}

/// Static validation failures for cached media.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CachedMediaError {
    #[error("cached content metadata is invalid")]
    InvalidMetadata,
    #[error("cached media file ID is invalid")]
    InvalidFileId,
    #[error("cached media size is invalid")]
    InvalidSize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_bounds_and_ascii_file_id() {
        let media = CachedMedia::new(CachedMediaKind::Video, "opaque_id-1".into(), 1).unwrap();
        assert_eq!(media.kind(), CachedMediaKind::Video);
        assert_eq!(media.file_id(), "opaque_id-1");
        assert_eq!(media.size_bytes(), 1);
        assert!(CachedMedia::new(CachedMediaKind::Photo, "x".repeat(4096), 1).is_ok());

        for id in [
            "",
            "with space",
            "with\nnewline",
            "with\tcontrol",
            "é",
            "http://127.0.0.1/private",
            "https://example.com/media",
            "attach://file",
            "file:///etc/passwd",
            "/tmp/file",
            "../file",
        ] {
            assert_eq!(
                CachedMedia::new(CachedMediaKind::Document, id.to_owned(), 1),
                Err(CachedMediaError::InvalidFileId)
            );
        }
        assert_eq!(
            CachedMedia::new(CachedMediaKind::Photo, "x".repeat(4097), 1),
            Err(CachedMediaError::InvalidFileId)
        );
        assert_eq!(
            CachedMedia::new(CachedMediaKind::Audio, "valid".into(), 0),
            Err(CachedMediaError::InvalidSize)
        );
        assert_eq!(
            CachedMedia::new(
                CachedMediaKind::Audio,
                "valid".into(),
                CachedMedia::MAX_SIZE_BYTES + 1
            ),
            Err(CachedMediaError::InvalidSize)
        );
    }

    #[test]
    fn debug_redacts_file_id_and_errors_are_static() {
        let media = CachedMedia::new(CachedMediaKind::Photo, "private-file-id".into(), 42).unwrap();
        let debug = format!("{media:?}");
        assert!(!debug.contains("private-file-id"));
        assert!(debug.contains("[REDACTED]"));
        assert_eq!(
            CachedMedia::new(CachedMediaKind::Photo, "bad id".into(), 42)
                .unwrap_err()
                .to_string(),
            "cached media file ID is invalid"
        );
    }
}
