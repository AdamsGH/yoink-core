//! Framework-independent domain contracts for Yoink operations and downloads.
//!
//! These types model intent and lifecycle only. They perform no network or runtime I/O.
//! URL validation here is syntactic; adapters remain responsible for network policy and SSRF defenses.

use std::fmt;

use thiserror::Error;
use url::Url;
use uuid::Uuid;

pub mod content;
pub mod downloader_settings;
pub use content::ContentMetadata;
pub mod cache;
pub use cache::{CachedMedia, CachedMediaError, CachedMediaKind};

pub mod network;
pub use network::{
    CookieReference, CredentialVersion, NetworkReferenceError, NetworkSelection, ProxyReference,
    ProxySource,
};

/// Stable, opaque identifier shared by every attempt of one operation.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct OperationId(Uuid);

impl OperationId {
    /// Creates a new unpredictable operation identifier.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Returns the identifier as a UUID.
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for OperationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("OperationId").field(&self.0).finish()
    }
}

#[cfg(test)]
mod subtitle_tests;

/// The representation requested from a downloader.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaMode {
    Video,
    Audio,
    Image,
    Subtitles,
    Formats,
    Links,
    AudioLinks,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoCodec {
    H264,
    H265,
    Av1,
    Vp9,
    Any,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioCodec {
    Best,
    Aac,
    Mp3,
    Opus,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Container {
    Mp4,
    Mkv,
    Webm,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Engine {
    Auto,
    YtDlp,
    Gallery,
    AutoYtDlpV1,
    AutoGalleryV1,
    AutoFallbackV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlaylistRange {
    start: i32,
    end: Option<i32>,
}

impl PlaylistRange {
    pub fn new(start: i32, end: Option<i32>) -> Result<Self, RequestValidationError> {
        if start == 0 || end == Some(0) || (end.is_none() && start < -100) {
            return Err(RequestValidationError::InvalidParameters);
        }
        if let Some(end) = end {
            let count = (i64::from(end) - i64::from(start)).unsigned_abs() + 1;
            if (start.is_positive() != end.is_positive()) || count > 100 {
                return Err(RequestValidationError::InvalidParameters);
            }
        }
        Ok(Self { start, end })
    }

    pub const fn start(&self) -> i32 {
        self.start
    }
    pub const fn end(&self) -> Option<i32> {
        self.end
    }

    pub fn entry_count(self) -> usize {
        match self.end {
            Some(end) => (i64::from(end) - i64::from(self.start)).unsigned_abs() as usize + 1,
            None if self.start > 0 => 1,
            None => self.start.unsigned_abs() as usize,
        }
    }
}

/// A validated delivery destination. Credentials and transport state do not belong here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeliveryTarget {
    bot_id: i64,
    chat_id: i64,
    thread_id: Option<i64>,
    reply_to_message_id: Option<i32>,
}

impl DeliveryTarget {
    pub fn new(
        bot_id: i64,
        chat_id: i64,
        thread_id: Option<i64>,
        reply_to_message_id: Option<i32>,
    ) -> Result<Self, RequestValidationError> {
        if bot_id <= 0
            || chat_id == 0
            || thread_id.is_some_and(|id| id <= 0)
            || reply_to_message_id.is_some_and(|id| id <= 0)
        {
            return Err(RequestValidationError::InvalidDeliveryTarget);
        }
        Ok(Self {
            bot_id,
            chat_id,
            thread_id,
            reply_to_message_id,
        })
    }

    pub const fn bot_id(&self) -> i64 {
        self.bot_id
    }
    pub const fn chat_id(&self) -> i64 {
        self.chat_id
    }
    pub const fn thread_id(&self) -> Option<i64> {
        self.thread_id
    }
    pub const fn reply_to_message_id(&self) -> Option<i32> {
        self.reply_to_message_id
    }
}

/// A validated time range in seconds, with an exclusive end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipRange {
    start_seconds: f64,
    end_seconds: f64,
}

impl ClipRange {
    /// Creates a range with a finite, nonnegative start and a strictly later end.
    pub fn new(start_seconds: f64, end_seconds: f64) -> Result<Self, RequestValidationError> {
        if !start_seconds.is_finite()
            || !end_seconds.is_finite()
            || start_seconds < 0.0
            || end_seconds < 0.0
            || end_seconds <= start_seconds
        {
            return Err(RequestValidationError::InvalidClipRange);
        }
        Ok(Self {
            start_seconds,
            end_seconds,
        })
    }

    pub const fn start_seconds(&self) -> f64 {
        self.start_seconds
    }

    pub const fn end_seconds(&self) -> f64 {
        self.end_seconds
    }
}

/// Exact subtitle language selection, never a provider regex or filename.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubtitleOptions {
    language: String,
    automatic: bool,
}

impl Default for SubtitleOptions {
    fn default() -> Self {
        Self {
            language: "en".into(),
            automatic: false,
        }
    }
}

impl SubtitleOptions {
    pub fn new(language: &str, automatic: bool) -> Result<Self, RequestValidationError> {
        let parts: Vec<_> = language.split('-').collect();
        if language.len() > 16
            || parts.len() > 2
            || !(2..=3).contains(&parts[0].len())
            || !parts[0].bytes().all(|b| b.is_ascii_lowercase())
            || parts.get(1).is_some_and(|part| {
                !(2..=8).contains(&part.len()) || !part.bytes().all(|b| b.is_ascii_alphanumeric())
            })
        {
            return Err(RequestValidationError::InvalidParameters);
        }
        Ok(Self {
            language: language.into(),
            automatic,
        })
    }
    pub fn language(&self) -> &str {
        &self.language
    }
    pub const fn automatic(&self) -> bool {
        self.automatic
    }
}

/// Caller-selected format overrides used to freeze saved preferences at admission.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DownloadPreferenceOverrides {
    pub max_height: Option<Option<u32>>,
    pub video_codec: Option<VideoCodec>,
    pub audio_codec: Option<AudioCodec>,
    pub container: Option<Container>,
    pub send_as_file: Option<bool>,
    pub gallery_zip: Option<bool>,
    pub subtitle_language: Option<String>,
    pub subtitle_automatic: Option<bool>,
}

/// Per-operation download options. These values are never stored as shared user state.
#[derive(Clone, Debug, PartialEq)]
pub struct DownloadParams {
    max_height: Option<u32>,
    video_codec: VideoCodec,
    audio_codec: AudioCodec,
    container: Container,
    engine: Engine,
    playlist: Option<PlaylistRange>,
    clips: Vec<ClipRange>,
    send_as_file: bool,
    gallery_zip: bool,
    subtitles: Option<SubtitleOptions>,
    media_info: bool,
}

impl Default for DownloadParams {
    fn default() -> Self {
        Self {
            max_height: None,
            video_codec: VideoCodec::H264,
            audio_codec: AudioCodec::Best,
            container: Container::Mp4,
            engine: Engine::Auto,
            playlist: None,
            clips: Vec::new(),
            send_as_file: false,
            gallery_zip: false,
            subtitles: None,
            media_info: false,
        }
    }
}

impl DownloadParams {
    pub fn media_info(&self) -> bool {
        self.media_info
    }
    pub fn with_media_info(mut self, enabled: bool) -> Self {
        self.media_info = enabled;
        self
    }
    /// Creates options with an optional maximum video height. Zero is not a meaningful limit.
    pub fn new(max_height: Option<u32>) -> Result<Self, RequestValidationError> {
        if max_height == Some(0) {
            return Err(RequestValidationError::InvalidParameters);
        }
        Ok(Self {
            max_height,
            ..Self::default()
        })
    }

    pub const fn max_height(&self) -> Option<u32> {
        self.max_height
    }
    pub const fn video_codec(&self) -> VideoCodec {
        self.video_codec
    }
    pub const fn audio_codec(&self) -> AudioCodec {
        self.audio_codec
    }
    pub const fn container(&self) -> Container {
        self.container
    }
    pub const fn engine(&self) -> Engine {
        self.engine
    }
    pub const fn playlist(&self) -> Option<PlaylistRange> {
        self.playlist
    }
    pub fn clips(&self) -> &[ClipRange] {
        &self.clips
    }
    pub const fn send_as_file(&self) -> bool {
        self.send_as_file
    }
    pub const fn gallery_zip(&self) -> bool {
        self.gallery_zip
    }

    pub fn with_max_height(mut self, value: Option<u32>) -> Result<Self, RequestValidationError> {
        if value == Some(0) {
            return Err(RequestValidationError::InvalidParameters);
        }
        self.max_height = value;
        Ok(self)
    }
    pub fn with_video_codec(mut self, value: VideoCodec) -> Self {
        self.video_codec = value;
        self
    }
    pub fn with_audio_codec(mut self, value: AudioCodec) -> Self {
        self.audio_codec = value;
        self
    }
    pub fn with_container(mut self, value: Container) -> Self {
        self.container = value;
        self
    }
    pub fn with_engine(mut self, value: Engine) -> Self {
        self.engine = value;
        self
    }
    pub fn with_playlist(mut self, value: Option<PlaylistRange>) -> Self {
        self.playlist = value;
        self
    }
    pub fn with_clips(mut self, value: Vec<ClipRange>) -> Result<Self, RequestValidationError> {
        if value.len() > 20 {
            return Err(RequestValidationError::InvalidParameters);
        }
        self.clips = value;
        Ok(self)
    }
    pub fn with_send_as_file(mut self, value: bool) -> Self {
        self.send_as_file = value;
        self
    }
    pub fn subtitles(&self) -> Option<&SubtitleOptions> {
        self.subtitles.as_ref()
    }
    pub fn with_subtitles(mut self, value: Option<SubtitleOptions>) -> Self {
        self.subtitles = value;
        self
    }

    fn validate_mode(
        &self,
        mode: MediaMode,
        clip: Option<ClipRange>,
    ) -> Result<(), RequestValidationError> {
        if matches!(mode, MediaMode::Links | MediaMode::AudioLinks) {
            if clip.is_some()
                || !matches!(self.max_height, None | Some(720 | 1080 | 480 | 360))
                || self.video_codec != VideoCodec::H264
                || self.audio_codec != AudioCodec::Best
                || self.container != Container::Mp4
                || !self.send_as_file
                || !matches!(
                    self.engine,
                    Engine::Auto | Engine::YtDlp | Engine::AutoYtDlpV1
                )
                || self.playlist.is_some()
                || !self.clips.is_empty()
                || self.gallery_zip
                || self.subtitles.is_some()
                || (mode == MediaMode::AudioLinks && self.max_height.is_some())
            {
                return Err(RequestValidationError::InvalidParameters);
            }
        } else if mode == MediaMode::Formats {
            if clip.is_some()
                || self.max_height.is_some()
                || self.video_codec != VideoCodec::H264
                || self.audio_codec != AudioCodec::Best
                || self.container != Container::Mp4
                || !self.send_as_file
                || matches!(
                    self.engine,
                    Engine::Gallery | Engine::AutoGalleryV1 | Engine::AutoFallbackV1
                )
                || self.playlist.is_some()
                || !self.clips.is_empty()
                || self.gallery_zip
                || self.subtitles.is_some()
            {
                return Err(RequestValidationError::InvalidParameters);
            }
        } else if mode == MediaMode::Subtitles {
            if clip.is_some()
                || self.max_height.is_some()
                || self.video_codec != VideoCodec::H264
                || self.audio_codec != AudioCodec::Best
                || self.container != Container::Mp4
                || matches!(
                    self.engine,
                    Engine::Gallery | Engine::AutoGalleryV1 | Engine::AutoFallbackV1
                )
                || self.playlist.is_some()
                || !self.clips.is_empty()
                || self.gallery_zip
            {
                return Err(RequestValidationError::InvalidParameters);
            }
        } else if self.subtitles.is_some() {
            return Err(RequestValidationError::InvalidParameters);
        }
        Ok(())
    }

    pub fn with_gallery_zip(mut self, value: bool) -> Self {
        self.gallery_zip = value;
        self
    }
}

/// Immutable, validated input for a single download operation.
///
/// Actor IDs are positive signed 64-bit values so they can be persisted without narrowing.
/// Debug output deliberately omits the URL. The URL accessor is available to the adapter that
/// needs it, but callers should not log it because URLs may contain sensitive query parameters.
#[derive(Clone, PartialEq)]
pub struct DownloadRequest {
    actor_id: u64,
    source_url: Url,
    media_mode: MediaMode,
    clip_range: Option<ClipRange>,
    params: DownloadParams,
    target: Option<DeliveryTarget>,
    network: NetworkSelection,
    preference_overrides: Option<DownloadPreferenceOverrides>,
    downloader_settings: Option<downloader_settings::DownloaderSettings>,
    force_nsfw: bool,
}

impl DownloadRequest {
    /// Freeze the automatic routing table at admission; explicit engine intent is unchanged.
    pub fn freeze_provider_routing(mut self) -> Self {
        if self.params.engine == Engine::Auto {
            self.params.engine = automatic_provider_v1(&self.source_url, self.media_mode);
        }
        self
    }

    pub fn new(
        actor_id: u64,
        source_url: &str,
        media_mode: MediaMode,
        clip_range: Option<ClipRange>,
        params: DownloadParams,
    ) -> Result<Self, RequestValidationError> {
        if actor_id == 0 || actor_id > i64::MAX as u64 {
            return Err(RequestValidationError::InvalidActorId);
        }

        let source_url =
            Url::parse(source_url).map_err(|_| RequestValidationError::InvalidSourceUrl)?;
        if !matches!(source_url.scheme(), "http" | "https")
            || source_url.host_str().is_none()
            || !source_url.username().is_empty()
            || source_url.password().is_some()
        {
            return Err(RequestValidationError::InvalidSourceUrl);
        }

        if clip_range.is_some() && !params.clips().is_empty() {
            return Err(RequestValidationError::InvalidParameters);
        }

        let mut params = params;
        params.validate_mode(media_mode, clip_range)?;
        if media_mode == MediaMode::Subtitles {
            params
                .subtitles
                .get_or_insert_with(SubtitleOptions::default);
        }
        Ok(Self {
            actor_id,
            source_url,
            media_mode,
            clip_range,
            params,
            target: None,
            network: NetworkSelection::NONE,
            preference_overrides: None,
            downloader_settings: None,
            force_nsfw: false,
        })
    }

    /// Rust-owned admission intent; never sent to workers or stored in cache metadata.
    pub const fn force_nsfw(&self) -> bool {
        self.force_nsfw
    }

    pub fn with_force_nsfw(mut self, forced: bool) -> Self {
        self.force_nsfw = forced;
        self
    }

    pub const fn actor_id(&self) -> u64 {
        self.actor_id
    }

    /// Returns the parsed URL. Do not include it in logs or diagnostics.
    pub fn source_url(&self) -> &Url {
        &self.source_url
    }

    pub const fn media_mode(&self) -> MediaMode {
        self.media_mode
    }

    pub const fn clip_range(&self) -> Option<ClipRange> {
        self.clip_range
    }

    pub const fn params(&self) -> &DownloadParams {
        &self.params
    }

    pub const fn target(&self) -> Option<&DeliveryTarget> {
        self.target.as_ref()
    }

    pub const fn network(&self) -> &NetworkSelection {
        &self.network
    }

    pub fn preference_overrides(&self) -> Option<&DownloadPreferenceOverrides> {
        self.preference_overrides.as_ref()
    }

    pub fn with_preference_overrides(mut self, overrides: DownloadPreferenceOverrides) -> Self {
        self.preference_overrides = Some(overrides);
        self
    }

    pub const fn downloader_settings(&self) -> Option<downloader_settings::DownloaderSettings> {
        self.downloader_settings
    }

    /// Admission-owned snapshot, excluded from replay intent and worker input.
    pub fn with_downloader_settings(
        mut self,
        settings: Option<downloader_settings::DownloaderSettings>,
    ) -> Result<Self, RequestValidationError> {
        if let Some(settings) = settings {
            settings
                .validate()
                .map_err(|_| RequestValidationError::InvalidParameters)?;
        }
        self.downloader_settings = settings;
        Ok(self)
    }

    pub fn with_params(mut self, params: DownloadParams) -> Result<Self, RequestValidationError> {
        if self.clip_range.is_some() && !params.clips().is_empty() {
            return Err(RequestValidationError::InvalidParameters);
        }
        let mut params = params;
        params.validate_mode(self.media_mode, self.clip_range)?;
        if self.media_mode == MediaMode::Subtitles {
            params
                .subtitles
                .get_or_insert_with(SubtitleOptions::default);
        }
        self.params = params;
        Ok(self)
    }

    pub fn with_network(
        mut self,
        network: NetworkSelection,
    ) -> Result<Self, RequestValidationError> {
        network
            .validate_actor(self.actor_id)
            .map_err(|_| RequestValidationError::NetworkActorMismatch)?;
        self.network = network;
        Ok(self)
    }

    pub fn with_target(mut self, target: DeliveryTarget) -> Result<Self, RequestValidationError> {
        if target.chat_id() > 0 && target.chat_id() as u64 != self.actor_id {
            return Err(RequestValidationError::InvalidDeliveryTarget);
        }
        self.target = Some(target);
        Ok(self)
    }
}

impl fmt::Debug for DownloadRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadRequest")
            .field("actor_id", &self.actor_id)
            .field("source_url", &"[REDACTED]")
            .field("media_mode", &self.media_mode)
            .field("clip_range", &self.clip_range)
            .field("params", &self.params)
            .field("target", &self.target)
            .field("network", &self.network)
            .finish()
    }
}

/// Errors from constructing a validated domain request.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RequestValidationError {
    #[error("actor ID must be a positive signed 64-bit integer")]
    InvalidActorId,
    #[error("source URL must be a valid HTTP(S) URL without embedded credentials")]
    InvalidSourceUrl,
    #[error("clip range must have finite nonnegative bounds and a later end")]
    InvalidClipRange,
    #[error("download parameters are invalid")]
    InvalidParameters,
    #[error("delivery target is invalid")]
    InvalidDeliveryTarget,
    #[error("personal network reference belongs to a different actor")]
    NetworkActorMismatch,
}

/// Broad, stable failure categories suitable for domain decisions and persistence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureClass {
    InvalidInput,
    UnsupportedSource,
    SourceUnavailable,
    RateLimited,
    ProcessingFailed,
    ResourceLimit,
    Internal,
}

/// Classified operation failure; it carries no arbitrary executor text or credentials.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationFailure {
    class: FailureClass,
}

impl OperationFailure {
    pub const fn new(class: FailureClass) -> Self {
        Self { class }
    }

    pub const fn class(&self) -> FailureClass {
        self.class
    }
}

/// Current lifecycle state. Succeeded, Failed, and Cancelled are terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationState {
    Queued,
    Running,
    Succeeded,
    Failed(OperationFailure),
    Cancelled,
}

/// Rejected lifecycle operation. It contains no request data.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum TransitionError {
    #[error("operation can only start while queued")]
    NotQueued,
    #[error("operation can only complete while running")]
    NotRunning,
    #[error("operation is already terminal")]
    Terminal,
    #[error("cancellation must be requested before executor acknowledgement")]
    CancellationNotRequested,
    #[error("only a failed operation can start a new attempt")]
    NotFailed,
    #[error("operation attempt counter is exhausted")]
    AttemptsExhausted,
}

/// One attempt to perform a download.
/// Retries are new values with the same operation ID and an incremented attempt number.
#[derive(Clone, Debug, PartialEq)]
pub struct Operation {
    id: OperationId,
    attempt: u32,
    request: DownloadRequest,
    state: OperationState,
    cancel_requested: bool,
}

impl Operation {
    pub fn new(request: DownloadRequest) -> Self {
        Self {
            id: OperationId::new(),
            attempt: 1,
            request,
            state: OperationState::Queued,
            cancel_requested: false,
        }
    }

    pub const fn id(&self) -> OperationId {
        self.id
    }

    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    pub const fn request(&self) -> &DownloadRequest {
        &self.request
    }

    pub const fn state(&self) -> OperationState {
        self.state
    }

    pub const fn cancellation_requested(&self) -> bool {
        self.cancel_requested
    }

    pub fn start(&mut self) -> Result<(), TransitionError> {
        if self.state != OperationState::Queued {
            return Err(if is_terminal(self.state) {
                TransitionError::Terminal
            } else {
                TransitionError::NotQueued
            });
        }
        self.state = OperationState::Running;
        Ok(())
    }

    pub fn succeed(&mut self) -> Result<(), TransitionError> {
        self.require_running()?;
        self.state = OperationState::Succeeded;
        Ok(())
    }

    pub fn fail(&mut self, failure: OperationFailure) -> Result<(), TransitionError> {
        self.require_running()?;
        self.state = OperationState::Failed(failure);
        Ok(())
    }

    /// Records a cancellation request without claiming that execution has stopped.
    /// A later successful completion is still valid until cancellation is acknowledged.
    pub fn request_cancel(&mut self) -> Result<(), TransitionError> {
        if is_terminal(self.state) {
            return Err(TransitionError::Terminal);
        }
        self.cancel_requested = true;
        Ok(())
    }

    /// Marks cancellation only after the executor confirms that the work has stopped.
    pub fn acknowledge_cancel(&mut self) -> Result<(), TransitionError> {
        if is_terminal(self.state) {
            return Err(TransitionError::Terminal);
        }
        if !self.cancel_requested {
            return Err(TransitionError::CancellationNotRequested);
        }
        self.state = OperationState::Cancelled;
        Ok(())
    }

    /// Creates a fresh queued attempt after failure, retaining the operation ID and immutable request.
    /// The failed value remains unchanged and terminal; attempt numbers never wrap.
    pub fn new_attempt(&self) -> Result<Self, TransitionError> {
        if !matches!(self.state, OperationState::Failed(_)) {
            return Err(TransitionError::NotFailed);
        }
        let attempt = self
            .attempt
            .checked_add(1)
            .ok_or(TransitionError::AttemptsExhausted)?;
        Ok(Self {
            id: self.id,
            attempt,
            request: self.request.clone(),
            state: OperationState::Queued,
            cancel_requested: false,
        })
    }

    fn require_running(&self) -> Result<(), TransitionError> {
        if self.state == OperationState::Running {
            Ok(())
        } else if is_terminal(self.state) {
            Err(TransitionError::Terminal)
        } else {
            Err(TransitionError::NotRunning)
        }
    }
}

const fn is_terminal(state: OperationState) -> bool {
    matches!(
        state,
        OperationState::Succeeded | OperationState::Failed(_) | OperationState::Cancelled
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> DownloadRequest {
        DownloadRequest::new(
            42,
            "https://example.com/watch?v=private",
            MediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .unwrap()
    }

    #[test]
    fn follows_successful_lifecycle_and_refuses_invalid_transitions() {
        let mut op = Operation::new(request());
        assert_eq!(op.state(), OperationState::Queued);
        assert_eq!(op.succeed(), Err(TransitionError::NotRunning));
        op.start().unwrap();
        assert_eq!(op.start(), Err(TransitionError::NotQueued));
        op.succeed().unwrap();
        assert_eq!(op.state(), OperationState::Succeeded);
        assert_eq!(op.start(), Err(TransitionError::Terminal));
        assert_eq!(
            op.fail(OperationFailure::new(FailureClass::Internal)),
            Err(TransitionError::Terminal)
        );
        assert_eq!(op.acknowledge_cancel(), Err(TransitionError::Terminal));
        assert_eq!(op.new_attempt(), Err(TransitionError::NotFailed));
    }

    #[test]
    fn failure_is_terminal_and_retry_is_a_new_attempt() {
        let mut failed = Operation::new(request());
        failed.start().unwrap();
        failed
            .fail(OperationFailure::new(FailureClass::SourceUnavailable))
            .unwrap();
        assert_eq!(
            failed.state(),
            OperationState::Failed(OperationFailure::new(FailureClass::SourceUnavailable))
        );
        assert_eq!(failed.start(), Err(TransitionError::Terminal));

        let previous = failed.clone();
        let retry = failed.new_attempt().unwrap();
        assert_eq!(retry.id(), failed.id());
        assert_eq!(retry.attempt(), failed.attempt() + 1);
        assert_eq!(retry.state(), OperationState::Queued);
        assert_eq!(retry.request(), failed.request());
        assert_eq!(failed, previous);
        assert_eq!(
            failed.state(),
            OperationState::Failed(OperationFailure::new(FailureClass::SourceUnavailable))
        );
    }

    #[test]
    fn cancellation_is_pending_until_executor_acknowledges() {
        let mut op = Operation::new(request());
        assert_eq!(
            op.acknowledge_cancel(),
            Err(TransitionError::CancellationNotRequested)
        );
        op.request_cancel().unwrap();
        assert_eq!(op.state(), OperationState::Queued);
        assert!(op.cancellation_requested());
        op.start().unwrap();
        op.acknowledge_cancel().unwrap();
        assert_eq!(op.state(), OperationState::Cancelled);
        assert_eq!(op.succeed(), Err(TransitionError::Terminal));
        assert_eq!(op.request_cancel(), Err(TransitionError::Terminal));
        assert_eq!(op.new_attempt(), Err(TransitionError::NotFailed));
    }

    #[test]
    fn all_terminal_states_reject_mutation() {
        let mut succeeded = Operation::new(request());
        succeeded.start().unwrap();
        succeeded.succeed().unwrap();

        let mut failed = Operation::new(request());
        failed.start().unwrap();
        failed
            .fail(OperationFailure::new(FailureClass::Internal))
            .unwrap();

        let mut cancelled = Operation::new(request());
        cancelled.request_cancel().unwrap();
        cancelled.acknowledge_cancel().unwrap();

        for terminal in [&mut succeeded, &mut failed, &mut cancelled] {
            assert_eq!(terminal.start(), Err(TransitionError::Terminal));
            assert_eq!(terminal.request_cancel(), Err(TransitionError::Terminal));
            assert_eq!(
                terminal.acknowledge_cancel(),
                Err(TransitionError::Terminal)
            );
            assert_eq!(terminal.succeed(), Err(TransitionError::Terminal));
            assert_eq!(
                terminal.fail(OperationFailure::new(FailureClass::Internal)),
                Err(TransitionError::Terminal)
            );
        }
    }

    #[test]
    fn validates_actor_urls_ranges_and_parameters() {
        assert_eq!(
            DownloadRequest::new(
                0,
                "https://example.com",
                MediaMode::Video,
                None,
                DownloadParams::default(),
            )
            .unwrap_err(),
            RequestValidationError::InvalidActorId
        );
        assert_eq!(
            DownloadRequest::new(
                i64::MAX as u64,
                "https://example.com",
                MediaMode::Video,
                None,
                DownloadParams::default(),
            )
            .unwrap()
            .actor_id(),
            i64::MAX as u64
        );
        assert_eq!(
            DownloadRequest::new(
                i64::MAX as u64 + 1,
                "https://example.com",
                MediaMode::Video,
                None,
                DownloadParams::default(),
            )
            .unwrap_err(),
            RequestValidationError::InvalidActorId
        );
        for url in [
            "file:///tmp/media",
            "ftp://example.com/a",
            "not a URL",
            "https://user:secret@example.com/a",
        ] {
            assert_eq!(
                DownloadRequest::new(42, url, MediaMode::Video, None, DownloadParams::default(),)
                    .unwrap_err(),
                RequestValidationError::InvalidSourceUrl
            );
        }
        for (start, end) in [
            (-1.0, 1.0),
            (1.0, 1.0),
            (2.0, 1.0),
            (f64::NAN, 2.0),
            (0.0, f64::INFINITY),
        ] {
            assert_eq!(
                ClipRange::new(start, end).unwrap_err(),
                RequestValidationError::InvalidClipRange
            );
        }
        assert_eq!(
            DownloadParams::new(Some(0)),
            Err(RequestValidationError::InvalidParameters)
        );
        assert!(
            DownloadRequest::new(
                42,
                "http://example.com/path",
                MediaMode::Audio,
                Some(ClipRange::new(0.0, 1.5).unwrap()),
                DownloadParams::new(Some(720)).unwrap(),
            )
            .is_ok()
        );
    }

    #[test]
    fn formats_mode_accepts_only_report_parameters() {
        let defaults = DownloadParams::default().with_send_as_file(true);
        assert!(
            DownloadRequest::new(
                42,
                "https://example.com/a",
                MediaMode::Formats,
                None,
                defaults
            )
            .is_ok()
        );
        assert!(
            DownloadRequest::new(
                42,
                "https://example.com/a",
                MediaMode::Formats,
                Some(ClipRange::new(1.0, 2.0).unwrap()),
                DownloadParams::default()
            )
            .is_err()
        );
        assert!(
            DownloadRequest::new(
                42,
                "https://example.com/a",
                MediaMode::Formats,
                None,
                DownloadParams::default()
                    .with_max_height(Some(720))
                    .unwrap()
            )
            .is_err()
        );
        assert!(
            DownloadRequest::new(
                42,
                "https://example.com/a",
                MediaMode::Formats,
                None,
                DownloadParams::default()
                    .with_playlist(Some(PlaylistRange::new(1, Some(2)).unwrap()))
            )
            .is_err()
        );
    }

    #[test]
    fn debug_and_validation_errors_do_not_reveal_sensitive_url_data() {
        let req = DownloadRequest::new(
            42,
            "https://example.com/watch?token=secret-value",
            MediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .unwrap();
        let output = format!("{req:?}");
        assert!(!output.contains("example.com"));
        assert!(!output.contains("secret-value"));
        let error = DownloadRequest::new(
            42,
            "https://user:password@example.com",
            MediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .unwrap_err();
        assert!(!error.to_string().contains("password"));
        assert!(!error.to_string().contains("example.com"));
    }

    #[test]
    fn requests_keep_actor_and_options_independent() {
        let first = DownloadRequest::new(
            11,
            "https://one.example/video",
            MediaMode::Video,
            None,
            DownloadParams::new(Some(720)).unwrap(),
        )
        .unwrap();
        let second = DownloadRequest::new(
            22,
            "https://two.example/audio",
            MediaMode::Audio,
            Some(ClipRange::new(3.0, 8.0).unwrap()),
            DownloadParams::default(),
        )
        .unwrap();
        assert_eq!(first.actor_id(), 11);
        assert_eq!(first.params().max_height(), Some(720));
        assert_eq!(second.actor_id(), 22);
        assert_eq!(second.params().max_height(), None);
        assert_eq!(second.clip_range().unwrap().start_seconds(), 3.0);
        assert_eq!(first.network(), &NetworkSelection::NONE);
        assert_eq!(second.network(), &NetworkSelection::NONE);
    }

    #[test]
    fn typed_options_playlist_and_delivery_target_are_validated() {
        assert!(PlaylistRange::new(1, Some(100)).is_ok());
        assert_eq!(
            PlaylistRange::new(1, Some(101)),
            Err(RequestValidationError::InvalidParameters)
        );
        assert!(PlaylistRange::new(-2, Some(-1)).is_ok());
        assert!(PlaylistRange::new(5, Some(1)).is_ok());
        assert!(PlaylistRange::new(-100, None).is_ok());
        assert!(PlaylistRange::new(-101, None).is_err());
        assert!(PlaylistRange::new(i32::MIN, None).is_err());
        assert!(PlaylistRange::new(-2, Some(-3)).is_ok());
        for (start, end, count) in [
            (2, Some(1), 2),
            (-2, None, 2),
            (2, None, 1),
            (-2, Some(-3), 2),
        ] {
            assert_eq!(PlaylistRange::new(start, end).unwrap().entry_count(), count);
        }
        assert_eq!(
            DeliveryTarget::new(0, 42, None, None),
            Err(RequestValidationError::InvalidDeliveryTarget)
        );
        assert_eq!(
            DeliveryTarget::new(1, 0, None, None),
            Err(RequestValidationError::InvalidDeliveryTarget)
        );
        assert_eq!(
            DeliveryTarget::new(1, 42, Some(0), None),
            Err(RequestValidationError::InvalidDeliveryTarget)
        );
        assert_eq!(
            DeliveryTarget::new(1, 42, None, Some(0)),
            Err(RequestValidationError::InvalidDeliveryTarget)
        );

        let defaults = DownloadParams::default();
        assert_eq!(defaults.video_codec(), VideoCodec::H264);
        assert_eq!(defaults.audio_codec(), AudioCodec::Best);
        assert_eq!(defaults.container(), Container::Mp4);
        assert_eq!(defaults.engine(), Engine::Auto);
        assert_eq!(defaults.max_height(), None);
        assert!(!defaults.send_as_file());
        assert!(!defaults.gallery_zip());

        let params = DownloadParams::default()
            .with_video_codec(VideoCodec::Vp9)
            .with_audio_codec(AudioCodec::Opus)
            .with_container(Container::Webm)
            .with_engine(Engine::Gallery)
            .with_playlist(Some(PlaylistRange::new(2, Some(3)).unwrap()))
            .with_send_as_file(true)
            .with_gallery_zip(true);
        assert_eq!(params.video_codec(), VideoCodec::Vp9);
        assert_eq!(params.audio_codec(), AudioCodec::Opus);
        assert_eq!(params.container(), Container::Webm);
        assert_eq!(params.engine(), Engine::Gallery);
        assert_eq!(params.max_height(), None);
        let targeted = request()
            .with_target(DeliveryTarget::new(1, 42, Some(3), Some(9)).unwrap())
            .unwrap();
        assert_eq!(targeted.target().unwrap().bot_id(), 1);
        assert_eq!(targeted.target().unwrap().chat_id(), 42);
        assert_eq!(targeted.target().unwrap().thread_id(), Some(3));
        assert_eq!(targeted.target().unwrap().reply_to_message_id(), Some(9));
        assert!(request().target().is_none());
        assert_eq!(
            request().with_target(DeliveryTarget::new(1, 99, None, None).unwrap()),
            Err(RequestValidationError::InvalidDeliveryTarget)
        );
    }

    #[test]
    fn rejects_too_many_clips_and_legacy_additional_clip_conflict() {
        let range = ClipRange::new(0.0, 1.0).unwrap();
        assert_eq!(
            DownloadParams::default().with_clips(vec![range; 21]),
            Err(RequestValidationError::InvalidParameters)
        );
        let params = DownloadParams::default().with_clips(vec![range]).unwrap();
        assert_eq!(
            DownloadRequest::new(
                42,
                "https://example.com",
                MediaMode::Video,
                Some(range),
                params
            )
            .unwrap_err(),
            RequestValidationError::InvalidParameters
        );
    }

    #[test]
    fn attempt_overflow_is_rejected_without_mutating_the_failed_value() {
        let mut failed = Operation::new(request());
        failed.attempt = u32::MAX;
        failed.state = OperationState::Failed(OperationFailure::new(FailureClass::Internal));
        let previous = failed.clone();

        assert_eq!(
            failed.new_attempt(),
            Err(TransitionError::AttemptsExhausted)
        );
        assert_eq!(failed, previous);
        assert_eq!(failed.attempt(), u32::MAX);
        assert_eq!(
            failed.state(),
            OperationState::Failed(OperationFailure::new(FailureClass::Internal))
        );
    }

    #[test]
    fn cancellation_request_does_not_prevent_success_before_acknowledgement() {
        let mut op = Operation::new(request());
        op.start().unwrap();
        op.request_cancel().unwrap();
        assert_eq!(op.state(), OperationState::Running);
        assert!(op.cancellation_requested());

        op.succeed().unwrap();
        assert_eq!(op.state(), OperationState::Succeeded);
        assert_eq!(op.acknowledge_cancel(), Err(TransitionError::Terminal));
    }
}

/// Versioned legacy defaults. Host labels and literal path prefixes, never query substrings.
pub fn automatic_provider_v1(url: &Url, mode: MediaMode) -> Engine {
    if mode == MediaMode::Image {
        return Engine::AutoGalleryV1;
    }
    if matches!(
        mode,
        MediaMode::Subtitles | MediaMode::Formats | MediaMode::Links | MediaMode::AudioLinks
    ) {
        return Engine::AutoYtDlpV1;
    }
    let host = url.host_str().unwrap_or("").trim_end_matches('.');
    let matches = |domain: &str| {
        host == domain
            || host
                .strip_suffix(domain)
                .is_some_and(|prefix| prefix.ends_with('.'))
    };
    if ["youtube.com", "youtu.be"].iter().any(|d| matches(d)) {
        return Engine::AutoYtDlpV1;
    }
    if [
        "2ch.su",
        "4chan.org",
        "e-hentai.org",
        "e621.net",
        "gelbooru.com",
        "kemono.cr",
        "kemono.party",
        "coomer.party",
        "nhentai.net",
        "civitai.com",
        "wallhaven.cc",
    ]
    .iter()
    .any(|d| matches(d))
        || (matches("vk.com")
            && ["/wall-", "/album-"]
                .iter()
                .any(|p| url.path().starts_with(p)))
    {
        return Engine::AutoGalleryV1;
    }
    if matches("instagram.com") {
        Engine::AutoFallbackV1
    } else {
        Engine::AutoYtDlpV1
    }
}

#[cfg(test)]
mod provider_routing_tests {
    use super::*;
    #[test]
    fn provider_routing_table_and_deceptive_urls() {
        for host in [
            "2ch.su",
            "4chan.org",
            "e-hentai.org",
            "e621.net",
            "gelbooru.com",
            "kemono.cr",
            "kemono.party",
            "coomer.party",
            "nhentai.net",
            "civitai.com",
            "wallhaven.cc",
        ] {
            for prefix in ["", "www.", "media."] {
                let url = Url::parse(&format!("https://{prefix}{host}/post")).unwrap();
                assert_eq!(
                    automatic_provider_v1(&url, MediaMode::Video),
                    Engine::AutoGalleryV1
                );
            }
            let url =
                Url::parse(&format!("https://{host}.evil.test/?url=https://{host}/")).unwrap();
            assert_eq!(
                automatic_provider_v1(&url, MediaMode::Video),
                Engine::AutoYtDlpV1
            );
        }
        for (url, expected) in [
            (
                "https://m.vk.com/wall-123_456?foo=bar",
                Engine::AutoGalleryV1,
            ),
            ("https://VK.com/album-123_456", Engine::AutoGalleryV1),
            ("https://vk.com/?url=vk.com/wall-123", Engine::AutoYtDlpV1),
            ("https://evil.test/vk.com/wall-123", Engine::AutoYtDlpV1),
            ("https://vk.com.evil.test/wall-123", Engine::AutoYtDlpV1),
            ("https://vk.com/path/wall-123", Engine::AutoYtDlpV1),
            ("https://instagram.com/p/id", Engine::AutoFallbackV1),
            ("https://www.instagram.com/p/id", Engine::AutoFallbackV1),
            ("https://notinstagram.com/p/id", Engine::AutoYtDlpV1),
            (
                "https://youtube.com/watch?url=vk.com/wall-123",
                Engine::AutoYtDlpV1,
            ),
        ] {
            assert_eq!(
                automatic_provider_v1(&Url::parse(url).unwrap(), MediaMode::Video),
                expected
            );
        }
        assert_eq!(
            automatic_provider_v1(
                &Url::parse("https://youtube.com/watch?v=id").unwrap(),
                MediaMode::Image
            ),
            Engine::AutoGalleryV1
        );
    }
    #[test]
    fn provider_routing_freezes_only_automatic_intent() {
        let request = DownloadRequest::new(
            42,
            "https://instagram.com/p/id",
            MediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .unwrap();
        assert_eq!(request.params().engine(), Engine::Auto);
        let frozen = request.freeze_provider_routing();
        assert_eq!(frozen.params().engine(), Engine::AutoFallbackV1);
        assert_eq!(frozen.clone().freeze_provider_routing(), frozen);
        let explicit = frozen
            .with_params(DownloadParams::default().with_engine(Engine::YtDlp))
            .unwrap()
            .freeze_provider_routing();
        assert_eq!(explicit.params().engine(), Engine::YtDlp);
    }
}
