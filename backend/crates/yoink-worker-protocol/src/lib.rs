//! Versioned wire contracts for isolated media/content executors.
//!
//! These are deliberately independent of domain types. Keep the JSON envelope
//! versioned even when the application model evolves independently.
//!
//! Artifact paths are lexically constrained to relative paths. An executor/host
//! MUST canonicalize each artifact beneath its assigned operation workspace
//! before opening or consuming it. Lexical validation does not guarantee
//! symlink safety, including for paths created by an untrusted executor.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

/// Maximum encoded size accepted for one JSON message.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Clone, PartialEq, Eq)]
pub struct HttpSourceUrl(Url);

impl HttpSourceUrl {
    pub fn parse(value: &str) -> Result<Self, ProtocolError> {
        let url = Url::parse(value).map_err(|_| ProtocolError::InvalidRequest)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(ProtocolError::InvalidRequest);
        }
        Ok(Self(url))
    }

    pub fn as_url(&self) -> &Url {
        &self.0
    }
}

impl fmt::Debug for HttpSourceUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HttpSourceUrl([REDACTED])")
    }
}

impl Serialize for HttpSourceUrl {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.0.as_str())
    }
}

impl<'de> Deserialize<'de> for HttpSourceUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaMode {
    Best,
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskKind {
    Download { media_mode: MediaMode },
    ExtractContent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRequest {
    pub version: u16,
    pub operation_id: Uuid,
    pub attempt: u32,
    pub source_url: HttpSourceUrl,
    pub task: TaskKind,
}

impl WorkerRequest {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != PROTOCOL_VERSION || self.operation_id.is_nil() || self.attempt == 0 {
            return Err(ProtocolError::InvalidRequest);
        }
        Ok(())
    }
}

/// A validated path encoded as a portable, forward-slash-separated relative path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactPath(String);

impl ArtifactPath {
    pub fn parse(value: impl Into<String>) -> Result<Self, ProtocolError> {
        let value = value.into();
        let path = Path::new(&value);
        if value.is_empty()
            || value.contains('\\')
            || value.chars().any(char::is_control)
            || value.starts_with('/')
            || value.starts_with('~')
            || value.contains(':')
            || path
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
            || value
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(ProtocolError::InvalidEvent);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ArtifactPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactDescriptor {
    pub path: ArtifactPath,
    pub size_bytes: u64,
    /// MIME media type, e.g. "video/mp4". No executor-provided prose is carried.
    pub media_type: String,
}

impl ArtifactDescriptor {
    fn validate(&self) -> Result<(), ProtocolError> {
        let mut parts = self.media_type.split('/');
        let valid_token = |part: &str| {
            !part.is_empty()
                && part.len() <= 127
                && part.bytes().all(|b| {
                    b.is_ascii_alphanumeric()
                        || matches!(
                            b,
                            b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                        )
                })
        };
        if !parts.next().is_some_and(valid_token)
            || !parts.next().is_some_and(valid_token)
            || parts.next().is_some()
        {
            return Err(ProtocolError::InvalidEvent);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressStage {
    Fetching,
    Processing,
    WritingArtifact,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerEventKind {
    Progress { percent: f32, stage: ProgressStage },
    Completed { artifacts: Vec<ArtifactDescriptor> },
    Failure { class: FailureClass },
    CancellationAcknowledged,
}

impl WorkerEventKind {
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Progress { .. })
    }

    pub fn is_progress(&self) -> bool {
        matches!(self, Self::Progress { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    UnsupportedSource,
    SourceUnavailable,
    RateLimited,
    InvalidMedia,
    ProcessingFailed,
    ResourceLimit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerEvent {
    pub version: u16,
    pub operation_id: Uuid,
    pub attempt: u32,
    pub event: WorkerEventKind,
}

impl WorkerEvent {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != PROTOCOL_VERSION || self.operation_id.is_nil() || self.attempt == 0 {
            return Err(ProtocolError::InvalidEvent);
        }
        match &self.event {
            WorkerEventKind::Progress { percent, .. }
                if !percent.is_finite() || !(0.0..=100.0).contains(percent) =>
            {
                Err(ProtocolError::InvalidEvent)
            }
            WorkerEventKind::Completed { artifacts } => {
                if artifacts.is_empty() {
                    return Err(ProtocolError::InvalidEvent);
                }
                for artifact in artifacts {
                    artifact.validate()?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// Diagnostics intentionally contain only fixed messages, never parser input or URL data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    #[error("worker protocol frame exceeds the size limit")]
    FrameTooLarge,
    #[error("invalid worker request")]
    InvalidRequest,
    #[error("invalid worker event")]
    InvalidEvent,
    #[error("invalid worker protocol message")]
    InvalidMessage,
    #[error("could not encode worker protocol message")]
    EncodeFailed,
}

pub fn decode_request_frame(frame: &[u8]) -> Result<WorkerRequest, ProtocolError> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    let request: WorkerRequest =
        serde_json::from_slice(frame).map_err(|_| ProtocolError::InvalidMessage)?;
    request.validate()?;
    Ok(request)
}

pub fn decode_event_frame(frame: &[u8]) -> Result<WorkerEvent, ProtocolError> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    let event: WorkerEvent =
        serde_json::from_slice(frame).map_err(|_| ProtocolError::InvalidMessage)?;
    event.validate()?;
    Ok(event)
}

pub fn encode_request(request: &WorkerRequest) -> Result<Vec<u8>, ProtocolError> {
    request.validate()?;
    encode(request)
}

pub fn encode_event(event: &WorkerEvent) -> Result<Vec<u8>, ProtocolError> {
    event.validate()?;
    encode(event)
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    let frame = serde_json::to_vec(value).map_err(|_| ProtocolError::EncodeFailed)?;
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> WorkerRequest {
        WorkerRequest {
            version: PROTOCOL_VERSION,
            operation_id: Uuid::new_v4(),
            attempt: 1,
            source_url: HttpSourceUrl::parse("https://example.test/watch?id=secret").unwrap(),
            task: TaskKind::Download {
                media_mode: MediaMode::Audio,
            },
        }
    }

    fn event(event: WorkerEventKind) -> WorkerEvent {
        WorkerEvent {
            version: PROTOCOL_VERSION,
            operation_id: Uuid::new_v4(),
            attempt: 1,
            event,
        }
    }

    #[test]
    fn request_and_event_round_trip() {
        let request = request();
        let decoded = decode_request_frame(&encode_request(&request).unwrap()).unwrap();
        assert_eq!(decoded, request);

        let source = event(WorkerEventKind::Completed {
            artifacts: vec![ArtifactDescriptor {
                path: ArtifactPath::parse("media/audio.m4a").unwrap(),
                size_bytes: 42,
                media_type: "audio/mp4".into(),
            }],
        });
        let decoded = decode_event_frame(&encode_event(&source).unwrap()).unwrap();
        assert_eq!(decoded, source);
    }

    #[test]
    fn rejects_version_nil_id_zero_attempt_and_bad_url() {
        let mut request = request();
        request.version = 2;
        assert_eq!(request.validate(), Err(ProtocolError::InvalidRequest));
        request.version = PROTOCOL_VERSION;
        request.operation_id = Uuid::nil();
        assert_eq!(request.validate(), Err(ProtocolError::InvalidRequest));
        request.operation_id = Uuid::new_v4();
        request.attempt = 0;
        assert_eq!(request.validate(), Err(ProtocolError::InvalidRequest));
        assert_eq!(
            HttpSourceUrl::parse("file:///etc/passwd"),
            Err(ProtocolError::InvalidRequest)
        );
        assert_eq!(
            HttpSourceUrl::parse("https://user:password@example.test/x"),
            Err(ProtocolError::InvalidRequest)
        );
    }

    #[test]
    fn rejects_invalid_json_payloads_and_unknown_message_types() {
        assert_eq!(
            decode_request_frame(br#"{"version":9}"#),
            Err(ProtocolError::InvalidMessage)
        );
        let unknown = br#"{"version":1,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"event":{"type":"mystery"}}"#;
        assert_eq!(
            decode_event_frame(unknown),
            Err(ProtocolError::InvalidMessage)
        );
        let bad_progress = br#"{"version":1,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"event":{"type":"progress","percent":101.0,"stage":"fetching"}}"#;
        assert_eq!(
            decode_event_frame(bad_progress),
            Err(ProtocolError::InvalidEvent)
        );

        let valid_progress = br#"{"version":1,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"event":{"type":"progress","percent":25.0,"stage":"fetching"}}"#;
        assert_eq!(
            decode_event_frame(valid_progress).unwrap().event,
            WorkerEventKind::Progress {
                percent: 25.0,
                stage: ProgressStage::Fetching
            }
        );

        let unsupported_version = br#"{"version":2,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"event":{"type":"progress","percent":25.0,"stage":"fetching"}}"#;
        assert_eq!(
            decode_event_frame(unsupported_version),
            Err(ProtocolError::InvalidEvent)
        );
        let supported_request = br#"{"version":1,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"source_url":"https://example.test/video","task":{"kind":"download","media_mode":"audio"}}"#;
        assert!(decode_request_frame(supported_request).is_ok());
        let unsupported_request = br#"{"version":2,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"source_url":"https://example.test/video","task":{"kind":"download","media_mode":"audio"}}"#;
        assert_eq!(
            decode_request_frame(unsupported_request),
            Err(ProtocolError::InvalidRequest)
        );

        let nan_progress = br#"{"version":1,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"event":{"type":"progress","percent":NaN,"stage":"fetching"}}"#;
        assert_eq!(
            decode_event_frame(nan_progress),
            Err(ProtocolError::InvalidMessage)
        );
        let wrong_payload = br#"{"version":1,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"event":{"type":"progress","percent":"25","stage":"fetching"}}"#;
        assert_eq!(
            decode_event_frame(wrong_payload),
            Err(ProtocolError::InvalidMessage)
        );
    }

    #[test]
    fn bounds_frames_before_parsing() {
        let frame = vec![b' '; MAX_FRAME_BYTES + 1];
        assert_eq!(
            decode_request_frame(&frame),
            Err(ProtocolError::FrameTooLarge)
        );
        assert_eq!(
            decode_event_frame(&frame),
            Err(ProtocolError::FrameTooLarge)
        );
    }

    #[test]
    fn distinguishes_progress_from_terminal_events() {
        let progress = WorkerEventKind::Progress {
            percent: 25.0,
            stage: ProgressStage::Fetching,
        };
        let complete = WorkerEventKind::Completed {
            artifacts: vec![ArtifactDescriptor {
                path: ArtifactPath::parse("result.bin").unwrap(),
                size_bytes: 1,
                media_type: "application/octet-stream".into(),
            }],
        };
        let failure = WorkerEventKind::Failure {
            class: FailureClass::ProcessingFailed,
        };
        let cancelled = WorkerEventKind::CancellationAcknowledged;
        assert!(progress.is_progress());
        assert!(!progress.is_terminal());
        assert!(complete.is_terminal());
        assert!(failure.is_terminal());
        assert!(cancelled.is_terminal());
    }

    #[test]
    fn rejects_unsafe_artifact_paths() {
        for path in [
            "../secret",
            "a/../secret",
            "/tmp/file",
            "C:/file",
            "a\\b",
            "a//b",
            ".",
            "a\0b",
            "a\nb",
            "a\tb",
        ] {
            assert!(
                ArtifactPath::parse(path).is_err(),
                "accepted unsafe path: {path:?}"
            );
        }
        assert_eq!(
            ArtifactPath::parse("nested/media/file.mp4")
                .unwrap()
                .as_str(),
            "nested/media/file.mp4"
        );
    }

    #[test]
    fn rejects_completed_events_without_artifacts() {
        let complete = event(WorkerEventKind::Completed { artifacts: vec![] });
        assert_eq!(complete.validate(), Err(ProtocolError::InvalidEvent));
        let frame = br#"{"version":1,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"event":{"type":"completed","artifacts":[]}}"#;
        assert_eq!(decode_event_frame(frame), Err(ProtocolError::InvalidEvent));
    }

    #[test]
    fn diagnostics_and_debug_redact_source_url() {
        let request = request();
        let debug = format!("{request:?}");
        assert!(!debug.contains("secret"));
        assert!(!debug.contains("example.test"));
    }
}
