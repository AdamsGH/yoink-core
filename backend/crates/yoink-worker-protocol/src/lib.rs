//! Versioned wire contracts for isolated media/content executors.
//!
//! Request envelopes remain independent of application intent and authorization.
//! Shared bounded source prose carries no policy or trusted classification.
//!
//! Artifact paths are lexically constrained to relative paths. An executor/host
//! MUST canonicalize each artifact beneath its assigned operation workspace
//! before opening or consuming it. Lexical validation does not guarantee
//! symlink safety, including for paths created by an untrusted executor.

mod content;
pub use content::*;
mod media_info;
pub mod youtube_auth;
pub use media_info::*;
mod links;
pub use links::*;
mod search;
pub use search::*;

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

/// Maximum encoded size accepted for one JSON message.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const PROTOCOL_VERSION: u16 = 2;
pub const MAX_PROXY_URL_BYTES: usize = 4096;
pub const MAX_COOKIE_JAR_BYTES: usize = 512 * 1024;

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
    Image,
    Subtitles,
    Formats,
    Links,
    AudioLinks,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskKind {
    Download { media_mode: MediaMode },
    ExtractContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    H264,
    H265,
    Av1,
    Vp9,
    Any,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    Best,
    Aac,
    Mp3,
    Opus,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Container {
    Mp4,
    Mkv,
    Webm,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    Auto,
    YtDlp,
    Gallery,
    AutoYtDlpV1,
    AutoGalleryV1,
    AutoFallbackV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaylistRange {
    pub start: i32,
    pub end: Option<i32>,
}
impl PlaylistRange {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.start == 0 || self.end == Some(0) || (self.end.is_none() && self.start < -100) {
            return Err(ProtocolError::InvalidRequest);
        }
        if let Some(end) = self.end {
            let count = (i64::from(end) - i64::from(self.start)).unsigned_abs() + 1;
            if self.start.is_positive() != end.is_positive() || count > 100 {
                return Err(ProtocolError::InvalidRequest);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipRange {
    pub start_seconds: f64,
    pub end_seconds: f64,
}
impl ClipRange {
    fn validate(&self) -> Result<(), ProtocolError> {
        if !self.start_seconds.is_finite()
            || !self.end_seconds.is_finite()
            || self.start_seconds < 0.0
            || self.end_seconds <= self.start_seconds
        {
            return Err(ProtocolError::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubtitleOptions {
    pub language: String,
    pub automatic: bool,
}
impl SubtitleOptions {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let parts: Vec<_> = self.language.split('-').collect();
        if self.language.len() > 16
            || parts.len() > 2
            || !(2..=3).contains(&parts[0].len())
            || !parts[0].bytes().all(|b| b.is_ascii_lowercase())
            || parts.get(1).is_some_and(|part| {
                !(2..=8).contains(&part.len()) || !part.bytes().all(|b| b.is_ascii_alphanumeric())
            })
        {
            return Err(ProtocolError::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DownloadOptions {
    pub max_height: Option<u32>,
    pub video_codec: VideoCodec,
    pub audio_codec: AudioCodec,
    pub container: Container,
    pub engine: Engine,
    pub playlist: Option<PlaylistRange>,
    pub clips: Vec<ClipRange>,
    pub gallery_zip: bool,
    #[serde(default)]
    pub media_info: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitles: Option<SubtitleOptions>,
}
impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            max_height: None,
            video_codec: VideoCodec::H264,
            audio_codec: AudioCodec::Best,
            container: Container::Mp4,
            engine: Engine::Auto,
            playlist: None,
            clips: Vec::new(),
            gallery_zip: false,
            media_info: false,
            subtitles: None,
        }
    }
}
impl DownloadOptions {
    fn validate(&self) -> Result<(), ProtocolError> {
        if self.max_height == Some(0) || self.clips.len() > 20 {
            return Err(ProtocolError::InvalidRequest);
        }
        if let Some(playlist) = self.playlist {
            playlist.validate()?;
        }
        for clip in &self.clips {
            clip.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkCredentials {
    pub proxy_url: Option<String>,
    pub cookie_jar: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub youtube_oauth: Option<youtube_auth::YoutubeOAuth>,
}
impl fmt::Debug for NetworkCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetworkCredentials")
            .field("youtube_oauth", &self.youtube_oauth)
            .field("proxy_url", &self.proxy_url.as_ref().map(|_| "[REDACTED]"))
            .field(
                "cookie_jar",
                &self.cookie_jar.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}
impl NetworkCredentials {
    /// Validate transient stdin credentials without exposing their values in diagnostics.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if let Some(oauth) = &self.youtube_oauth {
            oauth.validate()?;
            if self.cookie_jar.is_some() {
                return Err(ProtocolError::InvalidRequest);
            }
        }
        if let Some(proxy) = &self.proxy_url {
            if proxy.is_empty()
                || proxy.len() > MAX_PROXY_URL_BYTES
                || proxy.chars().any(char::is_control)
            {
                return Err(ProtocolError::InvalidRequest);
            }
            let url = Url::parse(proxy).map_err(|_| ProtocolError::InvalidRequest)?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none_or(|host| host.is_empty())
                || proxy
                    .split_once("://")
                    .is_none_or(|(_, authority)| authority.starts_with('/'))
                || !matches!(url.path(), "" | "/")
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(ProtocolError::InvalidRequest);
            }
        }
        if let Some(cookies) = &self.cookie_jar {
            if cookies.len() > MAX_COOKIE_JAR_BYTES
                || cookies.is_empty()
                || cookies
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\t' | '\r' | '\n'))
            {
                return Err(ProtocolError::InvalidRequest);
            }
            for line in cookies.lines().filter(|line| {
                !line.is_empty() && (!line.starts_with('#') || line.starts_with("#HttpOnly_"))
            }) {
                let fields: Vec<_> = line.split('\t').collect();
                if fields.len() != 7
                    || fields[0].is_empty()
                    || fields[2].is_empty()
                    || fields[4].parse::<u64>().is_err()
                    || fields[5].is_empty()
                    || !matches!(fields[1], "TRUE" | "FALSE")
                    || !matches!(fields[3], "TRUE" | "FALSE")
                {
                    return Err(ProtocolError::InvalidRequest);
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRequest {
    pub version: u16,
    pub operation_id: Uuid,
    pub attempt: u32,
    pub source_url: HttpSourceUrl,
    pub task: TaskKind,
    #[serde(default)]
    pub options: DownloadOptions,
    #[serde(default)]
    pub network: Option<NetworkCredentials>,
}

impl WorkerRequest {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != PROTOCOL_VERSION || self.operation_id.is_nil() || self.attempt == 0 {
            return Err(ProtocolError::InvalidRequest);
        }
        self.options.validate()?;
        let subtitles_mode = matches!(
            self.task,
            TaskKind::Download {
                media_mode: MediaMode::Subtitles
            }
        );
        if subtitles_mode != self.options.subtitles.is_some() {
            return Err(ProtocolError::InvalidRequest);
        }
        if matches!(
            self.task,
            TaskKind::Download {
                media_mode: MediaMode::Links | MediaMode::AudioLinks
            }
        ) {
            let o = &self.options;
            if !matches!(o.max_height, None | Some(720 | 1080 | 480 | 360))
                || o.video_codec != VideoCodec::H264
                || o.audio_codec != AudioCodec::Best
                || o.container != Container::Mp4
                || !matches!(o.engine, Engine::Auto | Engine::YtDlp | Engine::AutoYtDlpV1)
                || o.playlist.is_some()
                || !o.clips.is_empty()
                || o.gallery_zip
                || o.subtitles.is_some()
                || (matches!(
                    self.task,
                    TaskKind::Download {
                        media_mode: MediaMode::AudioLinks
                    }
                ) && o.max_height.is_some())
            {
                return Err(ProtocolError::InvalidRequest);
            }
        }
        if matches!(
            self.task,
            TaskKind::Download {
                media_mode: MediaMode::Formats
            }
        ) {
            let o = &self.options;
            if !matches!(self.source_url.as_url().scheme(), "http" | "https")
                || o.max_height.is_some()
                || o.video_codec != VideoCodec::H264
                || o.audio_codec != AudioCodec::Best
                || o.container != Container::Mp4
                || matches!(
                    o.engine,
                    Engine::Gallery | Engine::AutoGalleryV1 | Engine::AutoFallbackV1
                )
                || o.playlist.is_some()
                || !o.clips.is_empty()
                || o.gallery_zip
                || o.subtitles.is_some()
            {
                return Err(ProtocolError::InvalidRequest);
            }
        }
        if let Some(subtitles) = &self.options.subtitles {
            subtitles.validate()?;
            let o = &self.options;
            if o.max_height.is_some()
                || o.video_codec != VideoCodec::H264
                || o.audio_codec != AudioCodec::Best
                || o.container != Container::Mp4
                || matches!(
                    o.engine,
                    Engine::Gallery | Engine::AutoGalleryV1 | Engine::AutoFallbackV1
                )
                || o.playlist.is_some()
                || !o.clips.is_empty()
                || o.gallery_zip
            {
                return Err(ProtocolError::InvalidRequest);
            }
        }
        if let Some(network) = &self.network {
            network.validate()?;
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
    /// Bounded prose only; Rust owns classification and authorization.
    #[serde(default)]
    pub metadata: Option<yoink_domain::ContentMetadata>,
    #[serde(default)]
    pub media_info: Option<MediaInfoReport>,
    /// MIME media type, e.g. "video/mp4".
    pub media_type: String,
}

impl ArtifactDescriptor {
    fn validate(&self) -> Result<(), ProtocolError> {
        if self
            .media_info
            .as_ref()
            .is_some_and(|r| !r.valid() || r.size_bytes != self.size_bytes)
        {
            return Err(ProtocolError::InvalidEvent);
        }
        if self.metadata.as_ref().is_some_and(|m| !m.valid()) {
            return Err(ProtocolError::InvalidEvent);
        }
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
    SearchCompleted { entries: Vec<SearchEntry> },
    ContentCompleted { result: ContentResult },
    ContentDenied,
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
            WorkerEventKind::ContentCompleted { result } if !result.valid() => {
                Err(ProtocolError::InvalidEvent)
            }
            WorkerEventKind::SearchCompleted { entries } => validate_entries(entries),
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
    #[error(
        "invalid search request: use version 2, nonzero ID/attempt and a 2..=256-character youtube_search query; source_url/options/cookies are unsupported"
    )]
    InvalidSearchRequest,
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
    #[test]
    fn nsfw_source_metadata_bounds_and_no_worker_classification_field() {
        let mut descriptor = ArtifactDescriptor {
            media_info: None,
            path: ArtifactPath::parse("media.mp4").unwrap(),
            size_bytes: 1,
            media_type: "video/mp4".into(),
            metadata: Some(yoink_domain::ContentMetadata::default()),
        };
        assert!(descriptor.validate().is_ok());
        descriptor.metadata.as_mut().unwrap().description = "x".repeat(2001);
        assert!(descriptor.validate().is_err());
        let value = serde_json::json!({"path":"media.mp4","size_bytes":1,"media_type":"video/mp4","metadata":{"title":"safe","description":"","tags":[],"categories":[],"is_nsfw":false}});
        assert!(serde_json::from_value::<ArtifactDescriptor>(value).is_err());
    }
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
            options: DownloadOptions::default(),
            network: None,
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
                media_info: None,
                metadata: None,
                path: ArtifactPath::parse("media/audio.m4a").unwrap(),
                size_bytes: 42,
                media_type: "audio/mp4".into(),
            }],
        });
        let decoded = decode_event_frame(&encode_event(&source).unwrap()).unwrap();
        assert_eq!(decoded, source);
    }

    #[test]
    fn formats_task_requires_unmodified_ytdlp_options() {
        let mut req = request();
        req.task = TaskKind::Download {
            media_mode: MediaMode::Formats,
        };
        assert_eq!(req.validate(), Ok(()));
        req.options.max_height = Some(720);
        assert_eq!(req.validate(), Err(ProtocolError::InvalidRequest));
        req.options.max_height = None;
        req.options.engine = Engine::Gallery;
        assert_eq!(req.validate(), Err(ProtocolError::InvalidRequest));
    }

    #[test]
    fn rejects_version_nil_id_zero_attempt_and_bad_url() {
        let mut request = request();
        request.version = 1;
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
        let bad_progress = br#"{"version":2,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"event":{"type":"progress","percent":101.0,"stage":"fetching"}}"#;
        assert_eq!(
            decode_event_frame(bad_progress),
            Err(ProtocolError::InvalidEvent)
        );

        let valid_progress = br#"{"version":2,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"event":{"type":"progress","percent":25.0,"stage":"fetching"}}"#;
        assert_eq!(
            decode_event_frame(valid_progress).unwrap().event,
            WorkerEventKind::Progress {
                percent: 25.0,
                stage: ProgressStage::Fetching
            }
        );

        let unsupported_version = br#"{"version":3,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"event":{"type":"progress","percent":25.0,"stage":"fetching"}}"#;
        assert_eq!(
            decode_event_frame(unsupported_version),
            Err(ProtocolError::InvalidEvent)
        );
        let supported_request = br#"{"version":2,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"source_url":"https://example.test/video","task":{"kind":"download","media_mode":"audio"}}"#;
        assert!(decode_request_frame(supported_request).is_ok());
        let unsupported_request = br#"{"version":3,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"source_url":"https://example.test/video","task":{"kind":"download","media_mode":"audio"}}"#;
        assert_eq!(
            decode_request_frame(unsupported_request),
            Err(ProtocolError::InvalidRequest)
        );

        let nan_progress = br#"{"version":2,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"event":{"type":"progress","percent":NaN,"stage":"fetching"}}"#;
        assert_eq!(
            decode_event_frame(nan_progress),
            Err(ProtocolError::InvalidMessage)
        );
        let wrong_payload = br#"{"version":2,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"event":{"type":"progress","percent":"25","stage":"fetching"}}"#;
        assert_eq!(
            decode_event_frame(wrong_payload),
            Err(ProtocolError::InvalidMessage)
        );
    }

    #[test]
    fn subtitle_options_are_mode_specific_and_bounded() {
        let mut req = request();
        req.task = TaskKind::Download {
            media_mode: MediaMode::Subtitles,
        };
        assert!(req.validate().is_err());
        req.options.subtitles = Some(SubtitleOptions {
            language: "ru".into(),
            automatic: true,
        });
        req.validate().unwrap();
        let decoded: WorkerRequest =
            serde_json::from_slice(&serde_json::to_vec(&req).unwrap()).unwrap();
        assert_eq!(decoded, req);
        for language in ["en.*", "en,ru", "../../x", "EN", "", "en-US-extra"] {
            req.options.subtitles.as_mut().unwrap().language = language.into();
            assert!(req.validate().is_err());
        }
        req.options.subtitles.as_mut().unwrap().language = "en".into();
        req.options.max_height = Some(720);
        assert!(req.validate().is_err());
        req.options.max_height = None;
        req.task = TaskKind::Download {
            media_mode: MediaMode::Video,
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn download_options_and_network_credentials_are_bounded_and_redacted() {
        let mut req = request();
        req.options = DownloadOptions {
            max_height: Some(1080),
            video_codec: VideoCodec::H265,
            audio_codec: AudioCodec::Opus,
            container: Container::Webm,
            engine: Engine::YtDlp,
            playlist: Some(PlaylistRange {
                start: -1,
                end: Some(-2),
            }),
            clips: vec![ClipRange {
                start_seconds: 0.5,
                end_seconds: 2.0,
            }],
            gallery_zip: true,
            media_info: false,
            subtitles: None,
        };
        req.network = Some(NetworkCredentials {
            youtube_oauth: None,
            proxy_url: Some("https://proxy.example:8443".into()),
            cookie_jar: Some(
                "# Netscape\n.example.test\tTRUE\t/\tFALSE\t2000000000\tname\tsecret-cookie".into(),
            ),
        });
        req.validate().unwrap();
        let json = serde_json::to_vec(&req).unwrap();
        let decoded: WorkerRequest = serde_json::from_slice(&json).unwrap();
        assert_eq!(decoded, req);
        let debug = format!("{:?}", req.network);
        assert!(!debug.contains("proxy.example"));
        assert!(!debug.contains("secret-cookie"));
        assert!(String::from_utf8(json).unwrap().contains("secret-cookie"));

        for proxy in [
            "",
            "ftp://proxy.test",
            "https:///missing-host",
            "https://proxy.test\n",
        ] {
            let mut bad = request();
            bad.network = Some(NetworkCredentials {
                youtube_oauth: None,
                proxy_url: Some(proxy.into()),
                cookie_jar: None,
            });
            assert_eq!(bad.validate(), Err(ProtocolError::InvalidRequest));
        }
        let mut too_long_proxy = request();
        too_long_proxy.network = Some(NetworkCredentials {
            youtube_oauth: None,
            proxy_url: Some(format!("http://x/{}", "a".repeat(MAX_PROXY_URL_BYTES))),
            cookie_jar: None,
        });
        assert_eq!(
            too_long_proxy.validate(),
            Err(ProtocolError::InvalidRequest)
        );
        let mut malformed_cookie = request();
        malformed_cookie.network = Some(NetworkCredentials {
            youtube_oauth: None,
            proxy_url: None,
            cookie_jar: Some("not-a-netscape-row".into()),
        });
        assert_eq!(
            malformed_cookie.validate(),
            Err(ProtocolError::InvalidRequest)
        );
        let mut control_cookie = request();
        control_cookie.network = Some(NetworkCredentials {
            youtube_oauth: None,
            proxy_url: None,
            cookie_jar: Some("name\u{0001}".into()),
        });
        assert_eq!(
            control_cookie.validate(),
            Err(ProtocolError::InvalidRequest)
        );
        let mut huge_cookie = request();
        huge_cookie.network = Some(NetworkCredentials {
            youtube_oauth: None,
            proxy_url: None,
            cookie_jar: Some("x".repeat(MAX_COOKIE_JAR_BYTES + 1)),
        });
        assert_eq!(huge_cookie.validate(), Err(ProtocolError::InvalidRequest));
    }

    #[test]
    fn validates_option_bounds_and_defaults_for_omitted_fields() {
        assert_eq!(DownloadOptions::default().video_codec, VideoCodec::H264);
        assert_eq!(DownloadOptions::default().audio_codec, AudioCodec::Best);
        assert_eq!(DownloadOptions::default().container, Container::Mp4);
        assert_eq!(DownloadOptions::default().engine, Engine::Auto);
        assert!(!DownloadOptions::default().gallery_zip);

        for options in [
            DownloadOptions {
                max_height: Some(0),
                ..DownloadOptions::default()
            },
            DownloadOptions {
                playlist: Some(PlaylistRange {
                    start: 1,
                    end: Some(101),
                }),
                ..DownloadOptions::default()
            },
            DownloadOptions {
                playlist: Some(PlaylistRange {
                    start: 1,
                    end: Some(-1),
                }),
                ..DownloadOptions::default()
            },
            DownloadOptions {
                clips: vec![ClipRange {
                    start_seconds: 1.0,
                    end_seconds: 1.0,
                }],
                ..DownloadOptions::default()
            },
            DownloadOptions {
                clips: vec![
                    ClipRange {
                        start_seconds: 0.0,
                        end_seconds: 1.0
                    };
                    21
                ],
                ..DownloadOptions::default()
            },
        ] {
            let mut req = request();
            req.options = options;
            assert_eq!(req.validate(), Err(ProtocolError::InvalidRequest));
        }
        let minimal = br#"{"version":2,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"source_url":"https://example.test/video","task":{"kind":"download","media_mode":"image"}}"#;
        let req = decode_request_frame(minimal).unwrap();
        assert_eq!(req.options, DownloadOptions::default());
        assert_eq!(
            req.task,
            TaskKind::Download {
                media_mode: MediaMode::Image
            }
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
                media_info: None,
                metadata: None,
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
        let frame = br#"{"version":2,"operation_id":"4f15ab1d-8f81-4886-999d-44c26db8b9ce","attempt":1,"event":{"type":"completed","artifacts":[]}}"#;
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
