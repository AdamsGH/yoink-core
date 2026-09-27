//! Framework-independent domain contracts for Yoink operations and downloads.
//!
//! These types model intent and lifecycle only. They perform no network or runtime I/O.
//! URL validation here is syntactic; adapters remain responsible for network policy and SSRF defenses.

use std::fmt;

use thiserror::Error;
use url::Url;
use uuid::Uuid;

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

/// The representation requested from a downloader.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaMode {
    Video,
    Audio,
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

/// Per-operation download options. These values are never stored as shared user state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DownloadParams {
    max_height: Option<u32>,
}

impl DownloadParams {
    /// Creates options with an optional maximum video height. Zero is not a meaningful limit.
    pub fn new(max_height: Option<u32>) -> Result<Self, RequestValidationError> {
        if max_height == Some(0) {
            return Err(RequestValidationError::InvalidParameters);
        }
        Ok(Self { max_height })
    }

    pub const fn max_height(&self) -> Option<u32> {
        self.max_height
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
}

impl DownloadRequest {
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

        Ok(Self {
            actor_id,
            source_url,
            media_mode,
            clip_range,
            params,
        })
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
}

impl fmt::Debug for DownloadRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadRequest")
            .field("actor_id", &self.actor_id)
            .field("source_url", &"[REDACTED]")
            .field("media_mode", &self.media_mode)
            .field("clip_range", &self.clip_range)
            .field("params", &self.params)
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
