use std::{
    fmt,
    fs::File,
    io::{Read, Seek},
    os::{fd::OwnedFd, unix::fs::PermissionsExt},
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use rustix::{
    fs::{Mode, OFlags, fstat, openat},
    process::{Pid, Signal, kill_process_group},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::{Instant, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;
use yoink_worker_protocol::{
    FailureClass, MAX_FRAME_BYTES, SearchEntry, SearchRequest, WorkerEvent, WorkerEventKind,
    WorkerRequest, decode_event_frame, encode_request, encode_search_request,
};

const MAX_ARTIFACT: u64 = 2 * 1024 * 1024 * 1024;
const MAX_WORKSPACE: u64 = 8 * 1024 * 1024 * 1024;
const MAX_OUTPUT: usize = 4 * 1024 * 1024;
const MAX_FILES: usize = 512;
const MAX_ARTIFACTS: usize = 100;
const MAX_SNIFF_BYTES: usize = 64;

#[derive(Clone)]
pub struct Executor {
    config: ExecutorConfig,
    youtube_auth_adapter: bool,
    full_tasks: tokio_util::task::TaskTracker,
    full_shutdown: CancellationToken,
    full_admission: std::sync::Arc<std::sync::Mutex<()>>,
    music_tasks: tokio_util::task::TaskTracker,
    music_shutdown: CancellationToken,
    music_admission: std::sync::Arc<std::sync::Mutex<()>>,
}

#[cfg(test)]
mod formats_report_tests {
    use super::{load_artifact, valid_formats_report};
    use std::io::Read;

    const HEADER: &str =
        "Yoink source formats v1\nID\tEXT\tWIDTH\tHEIGHT\tFPS\tVIDEO\tAUDIO\tPROTOCOL\n";

    #[test]
    fn accepts_fixed_bounded_tsv_and_rejects_untrusted_text() {
        assert!(valid_formats_report(
            format!("{HEADER}137\tmp4\t1920\t1080\t29.970\th264\taac\thttps\n").as_bytes()
        ));
        for bad in [
            format!("{HEADER}137\tmp4\t1920\t1080\t29.970\th264\taac\thttps"),
            format!("{HEADER}137\t../x\t1920\t1080\t30\th264\taac\thttps\n"),
            format!("{HEADER}137\tmp4\t1920\t1080\t1000.001\th264\taac\thttps\n"),
            format!("{HEADER}137\tmp4\t1920\t1080\t30\th264\taac\thttps\textra\n"),
            format!("{HEADER}\tmp4\t1920\t1080\t30\th264\taac\thttps\n"),
            "arbitrary text\n".to_owned(),
        ] {
            assert!(!valid_formats_report(bad.as_bytes()));
        }
        let row = "1\tmp4\t0\t16384\t0\th264\taac\tdash\n";
        assert!(valid_formats_report(
            format!("{HEADER}{}", row.repeat(100)).as_bytes()
        ));
        assert!(!valid_formats_report(
            format!("{HEADER}{}", row.repeat(101)).as_bytes()
        ));
    }

    #[test]
    fn validated_formats_artifact_is_rewound_and_delivers_all_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = format!("{HEADER}137\tmp4\t1920\t1080\t29.970\th264\taac\thttps\n");
        std::fs::write(temp.path().join("formats.txt"), bytes.as_bytes()).unwrap();
        let descriptor = yoink_worker_protocol::ArtifactDescriptor {
            media_info: None,
            metadata: None,
            path: yoink_worker_protocol::ArtifactPath::parse("formats.txt").unwrap(),
            size_bytes: bytes.len() as u64,
            media_type: "text/plain".into(),
        };
        let mut artifact = load_artifact(temp.path(), &descriptor, 64 * 1024, true).unwrap();
        let mut delivered = Vec::new();
        artifact.file.read_to_end(&mut delivered).unwrap();
        assert_eq!(delivered, bytes.as_bytes());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArtifactLimits {
    pub max_artifact_bytes: u64,
    pub max_workspace_bytes: u64,
    pub max_artifacts: usize,
}

#[derive(Clone)]
pub struct ExecutorConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub workspace_root: PathBuf,
    pub timeout: Duration,
    pub processing_timeout: Duration,
    pub socket_timeout: Duration,
    pub max_artifact_bytes: u64,
    pub max_workspace_bytes: u64,
    pub max_output_bytes: usize,
    pub max_artifacts: usize,
    pub allowed_proxy_origins: Vec<String>,
    pub allowed_private_origins: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkerProgress {
    pub percent: f32,
    pub stage: yoink_worker_protocol::ProgressStage,
}

pub struct ValidatedArtifact {
    file: File,
    pub filename: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub metadata: Option<yoink_domain::ContentMetadata>,
    pub media_info: Option<yoink_worker_protocol::MediaInfoReport>,
}

impl ValidatedArtifact {
    /// Returns the already-validated open file. Keep this descriptor open while
    /// streaming: the private executor workspace is removed when execute returns.
    pub fn file(&self) -> &File {
        &self.file
    }

    /// Consumes this artifact and returns its open descriptor. The descriptor
    /// remains usable after the private executor workspace has been removed.
    pub fn into_file(self) -> File {
        self.file
    }
}

impl fmt::Debug for ValidatedArtifact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidatedArtifact")
            .field("file", &"[OPEN FILE]")
            .field("filename", &self.filename)
            .field("media_type", &self.media_type)
            .field("size_bytes", &self.size_bytes)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExecutorError {
    #[error("content network destination denied")]
    ContentDenied,
    #[error("full content fetch failed: {0}")]
    FullContent(yoink_worker_protocol::FullContentError),
    #[error("invalid executor configuration")]
    InvalidConfig,
    #[error("worker could not be started")]
    Spawn,
    #[error("worker exited unsuccessfully")]
    Exited(Option<i32>),
    #[error("invalid worker protocol")]
    Protocol,
    #[error("worker execution timed out")]
    Timeout,
    #[error("worker execution cancelled")]
    Cancelled,
    #[error("worker resource limit exceeded")]
    ResourceLimit,
    #[error("worker artifact is unsafe or invalid")]
    UnsafeArtifact,
    #[error("worker reported a failure")]
    Worker(FailureClass),
    #[error("worker I/O failed")]
    Io,
}

enum ExecutionRequest<'a> {
    Media(&'a WorkerRequest),
    Search(&'a SearchRequest),
    Music(&'a yoink_worker_protocol::MusicRequest),
    Content(&'a yoink_worker_protocol::ContentRequest),
    FullContent(&'a yoink_worker_protocol::FullContentRequest),
}
enum ExecutionOutput {
    Media(Vec<ValidatedArtifact>),
    Search(Vec<SearchEntry>),
    Music(yoink_worker_protocol::MusicResult),
    Content(yoink_worker_protocol::ContentResult),
    FullContent(yoink_worker_protocol::FullContentResult),
}
impl ExecutionRequest<'_> {
    fn identity(&self) -> (u16, uuid::Uuid, u32) {
        match self {
            Self::Media(r) => (r.version, r.operation_id, r.attempt),
            Self::Search(r) => (r.version, r.operation_id, r.attempt),
            Self::Music(r) => (r.version, r.operation_id, r.attempt),
            Self::Content(r) => (r.version, r.operation_id, r.attempt),
            Self::FullContent(r) => (r.version, r.operation_id, r.attempt),
        }
    }
    fn frame(&self) -> Result<Vec<u8>, ExecutorError> {
        match self {
            Self::Media(r) => encode_request(r),
            Self::Search(r) => encode_search_request(r),
            Self::Music(r) => yoink_worker_protocol::encode_music_request(r),
            Self::Content(r) => yoink_worker_protocol::encode_content_request(r),
            Self::FullContent(r) => yoink_worker_protocol::encode_full_content_request(r),
        }
        .map_err(|_| ExecutorError::Protocol)
    }
}

impl Executor {
    /// Returns the immutable limits enforced by this executor instance.
    pub fn artifact_limits(&self) -> ArtifactLimits {
        ArtifactLimits {
            max_artifact_bytes: self.config.max_artifact_bytes,
            max_workspace_bytes: self.config.max_workspace_bytes,
            max_artifacts: self.config.max_artifacts,
        }
    }

    /// Lower per-operation budgets without replacing the shared child lifecycle.
    pub fn with_operation_limits(
        &self,
        timeout: Duration,
        max_artifact_bytes: u64,
        max_artifacts: usize,
    ) -> Result<Self, ExecutorError> {
        if timeout.is_zero()
            || timeout > Duration::from_secs(86400)
            || timeout.subsec_nanos() != 0
            || max_artifact_bytes == 0
            || max_artifact_bytes > 2 * 1024 * 1024 * 1024
            || max_artifacts == 0
            || max_artifacts > 500
        {
            return Err(ExecutorError::InvalidConfig);
        }
        let mut executor = self.clone();
        executor.config.timeout = timeout.min(self.config.timeout);
        executor.config.processing_timeout =
            self.config.processing_timeout.min(executor.config.timeout);
        executor.config.socket_timeout = self.config.socket_timeout.min(executor.config.timeout);
        executor.config.max_artifact_bytes = max_artifact_bytes.min(self.config.max_artifact_bytes);
        executor.config.max_workspace_bytes = self.config.max_workspace_bytes.min(
            executor
                .config
                .max_artifact_bytes
                .saturating_mul(max_artifacts.min(self.config.max_artifacts) as u64),
        );
        executor.config.max_artifacts = max_artifacts.min(self.config.max_artifacts);
        Ok(executor)
    }

    /// Creates an executor. The configured workspace_root must already exist and
    /// be accessible; execute creates only per-request directories beneath it.
    pub fn new(config: ExecutorConfig) -> Result<Self, ExecutorError> {
        if !config.program.is_absolute()
            || !config.workspace_root.is_absolute()
            || config.timeout.is_zero()
            || config.timeout > Duration::from_secs(3600)
            || config.timeout.subsec_nanos() != 0
            || config.processing_timeout.is_zero()
            || config.processing_timeout > config.timeout
            || config.processing_timeout.subsec_nanos() != 0
            || config.socket_timeout.is_zero()
            || config.socket_timeout > Duration::from_secs(300)
            || config.socket_timeout.subsec_nanos() != 0
            || !(1..=MAX_ARTIFACT).contains(&config.max_artifact_bytes)
            || config.max_workspace_bytes < config.max_artifact_bytes
            || config.max_workspace_bytes > MAX_WORKSPACE
            || !(1..=MAX_OUTPUT).contains(&config.max_output_bytes)
            || !(1..=MAX_ARTIFACTS).contains(&config.max_artifacts)
        {
            return Err(ExecutorError::InvalidConfig);
        }
        Ok(Self {
            config,
            youtube_auth_adapter: false,
            full_tasks: tokio_util::task::TaskTracker::new(),
            full_shutdown: CancellationToken::new(),
            full_admission: std::sync::Arc::new(std::sync::Mutex::new(())),
            music_tasks: tokio_util::task::TaskTracker::new(),
            music_shutdown: CancellationToken::new(),
            music_admission: std::sync::Arc::new(std::sync::Mutex::new(())),
        })
    }

    /// Operator-only Unix adapter; never configured from a media request.
    pub fn with_youtube_auth_adapter(mut self) -> Self {
        self.youtube_auth_adapter = true;
        self
    }

    pub async fn execute(
        &self,
        request: &WorkerRequest,
        cancel: CancellationToken,
    ) -> Result<Vec<ValidatedArtifact>, ExecutorError> {
        self.execute_with_progress(request, cancel, None).await
    }

    /// Executes one request and optionally publishes validated progress using a
    /// latest-value watch channel. Artifact descriptors refer to open files that
    /// remain valid after the private workspace has been removed.
    pub async fn execute_with_progress(
        &self,
        request: &WorkerRequest,
        cancel: CancellationToken,
        progress: Option<tokio::sync::watch::Sender<Option<WorkerProgress>>>,
    ) -> Result<Vec<ValidatedArtifact>, ExecutorError> {
        match self
            .execute_inner(ExecutionRequest::Media(request), cancel, progress)
            .await?
        {
            ExecutionOutput::Media(artifacts) => Ok(artifacts),
            _ => Err(ExecutorError::Protocol),
        }
    }

    /// Search shares launch, confinement, bounded IO, deadline and child cleanup with media.
    pub async fn execute_search(
        &self,
        request: &SearchRequest,
        cancel: CancellationToken,
        timeout: Duration,
    ) -> Result<Vec<SearchEntry>, ExecutorError> {
        // Admission budgets include DB/HTTP time, so remaining time can be fractional.
        if timeout.is_zero() || timeout > Duration::from_secs(120) {
            return Err(ExecutorError::InvalidConfig);
        }
        let mut executor = self.clone();
        executor.config.timeout = timeout.min(self.config.timeout);
        match executor
            .execute_inner(ExecutionRequest::Search(request), cancel, None)
            .await?
        {
            ExecutionOutput::Search(entries) => Ok(entries),
            _ => Err(ExecutorError::Protocol),
        }
    }

    /// Shares bounded IO, process-group cleanup and deadlines with media.
    pub async fn execute_content(
        &self,
        request: &yoink_worker_protocol::ContentRequest,
        cancel: CancellationToken,
    ) -> Result<yoink_worker_protocol::ContentResult, ExecutorError> {
        match self
            .execute_inner(ExecutionRequest::Content(request), cancel, None)
            .await?
        {
            ExecutionOutput::Content(result) => Ok(result),
            _ => Err(ExecutorError::Protocol),
        }
    }

    pub async fn execute_music(
        &self,
        request: &yoink_worker_protocol::MusicRequest,
        cancel: CancellationToken,
        timeout: Duration,
    ) -> Result<yoink_worker_protocol::MusicResult, ExecutorError> {
        let mut executor = self.clone();
        if timeout.is_zero() || timeout > Duration::from_secs(120) {
            return Err(ExecutorError::InvalidConfig);
        }
        executor.config.timeout = timeout.min(self.config.timeout);
        match self
            .execute_music_owned(request.clone(), cancel, executor)
            .await?
        {
            ExecutionOutput::Music(result) => Ok(result),
            _ => Err(ExecutorError::Protocol),
        }
    }
    pub async fn execute_music_audio(
        &self,
        request: &yoink_worker_protocol::MusicRequest,
        cancel: CancellationToken,
    ) -> Result<Vec<ValidatedArtifact>, ExecutorError> {
        if !matches!(
            request.task,
            yoink_worker_protocol::MusicTask::MusicAudio { .. }
        ) {
            return Err(ExecutorError::Protocol);
        }
        match self
            .execute_music_owned(request.clone(), cancel, self.clone())
            .await?
        {
            ExecutionOutput::Media(artifacts) => Ok(artifacts),
            _ => Err(ExecutorError::Protocol),
        }
    }

    // Like full-content, the owner outlives a dropped caller until reap/cleanup.
    async fn execute_music_owned(
        &self,
        request: yoink_worker_protocol::MusicRequest,
        cancel: CancellationToken,
        executor: Self,
    ) -> Result<ExecutionOutput, ExecutorError> {
        let owned_cancel = cancel.child_token();
        let drop_cancel = owned_cancel.clone().drop_guard();
        let shutdown = self.music_shutdown.clone();
        let handle = {
            let _admission = self
                .music_admission
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if shutdown.is_cancelled() {
                return Err(ExecutorError::Cancelled);
            }
            self.music_tasks.spawn(async move {
                let shutdown_cancel = owned_cancel.clone();
                let watcher = AbortOnDropHandle::new(tokio::spawn(async move {
                    shutdown.cancelled().await;
                    shutdown_cancel.cancel();
                }));
                let result = executor
                    .execute_inner(ExecutionRequest::Music(&request), owned_cancel, None)
                    .await;
                watcher.0.as_ref().expect("watcher").abort();
                let _ = watcher.join().await;
                result
            })
        };
        let result = handle.await.map_err(|_| ExecutorError::Io)?;
        drop_cancel.disarm();
        result
    }
    /// Permanently close Music admission and await process-group reap and workspace cleanup.
    pub async fn shutdown_music(&self) {
        {
            let _admission = self
                .music_admission
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.music_shutdown.cancel();
            self.music_tasks.close();
        }
        self.music_tasks.wait().await;
    }

    /// Full fetch always uses the immutable default launcher, never configured args.
    /// The independently tracked owner survives dropped caller futures and reaps
    /// its process group before removing the workspace.
    pub async fn execute_full_content(
        &self,
        request: &yoink_worker_protocol::FullContentRequest,
        cancel: CancellationToken,
    ) -> Result<yoink_worker_protocol::FullContentResult, ExecutorError> {
        yoink_worker_protocol::encode_full_content_request(request).map_err(|_| {
            ExecutorError::FullContent(yoink_worker_protocol::FullContentError::InvalidRequest)
        })?;
        if self.full_shutdown.is_cancelled() {
            return Err(ExecutorError::Cancelled);
        }
        let executor = self.full_content_executor(request);
        self.execute_full_content_owned(request.clone(), cancel, executor)
            .await
    }

    fn full_content_executor(&self, request: &yoink_worker_protocol::FullContentRequest) -> Self {
        let mut executor = self.clone();
        executor.config.program = "/usr/local/bin/yoink-worker-launcher".into();
        executor.config.args.clear();
        executor.config.allowed_private_origins.clear();
        executor.youtube_auth_adapter = false;
        executor.config.timeout =
            Duration::from_secs_f64(request.timeout_secs).min(self.config.timeout);
        executor.config.max_output_bytes = MAX_FRAME_BYTES + 1;
        executor
    }

    async fn execute_full_content_owned(
        &self,
        request: yoink_worker_protocol::FullContentRequest,
        cancel: CancellationToken,
        executor: Self,
    ) -> Result<yoink_worker_protocol::FullContentResult, ExecutorError> {
        let owned_cancel = cancel.child_token();
        let drop_cancel = owned_cancel.clone().drop_guard();
        let shutdown = self.full_shutdown.clone();
        let handle = {
            let _admission = self
                .full_admission
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.full_shutdown.is_cancelled() {
                return Err(ExecutorError::Cancelled);
            }
            self.full_tasks.spawn(async move {
                let shutdown_cancel = owned_cancel.clone();
                let watcher = AbortOnDropHandle::new(tokio::spawn(async move {
                    shutdown.cancelled().await;
                    shutdown_cancel.cancel();
                }));
                let result = executor
                    .execute_inner(ExecutionRequest::FullContent(&request), owned_cancel, None)
                    .await;
                drop(watcher);
                match result? {
                    ExecutionOutput::FullContent(result) => Ok(result),
                    _ => Err(ExecutorError::Protocol),
                }
            })
        };
        let result = handle.await.map_err(|_| ExecutorError::Io)?;
        drop_cancel.disarm();
        result
    }

    /// Cancels all full fetch owners and joins their bounded drain/reap/cleanup.
    /// Call during host shutdown before stopping the Tokio runtime.
    pub async fn shutdown_full_content(&self) {
        {
            let _admission = self
                .full_admission
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.full_shutdown.cancel();
            self.full_tasks.close();
        }
        self.full_tasks.wait().await;
    }

    async fn execute_inner(
        &self,
        request: ExecutionRequest<'_>,
        cancel: CancellationToken,
        progress: Option<tokio::sync::watch::Sender<Option<WorkerProgress>>>,
    ) -> Result<ExecutionOutput, ExecutorError> {
        let music_operation = matches!(request, ExecutionRequest::Music(_));
        if cancel.is_cancelled() {
            return Err(ExecutorError::Cancelled);
        }
        let frame = request.frame()?;
        let (version, operation_id, attempt) = request.identity();
        let prefix = format!("{}-{}-", operation_id, attempt);
        let workspace_root = self.config.workspace_root.clone();
        let temp = tokio::task::spawn_blocking(move || {
            tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .prefix(&prefix)
                .tempdir_in(workspace_root)
        })
        .await
        .map_err(|_| ExecutorError::Io)?
        .map_err(|_| ExecutorError::Io)?;
        let temp = std::sync::Arc::new(temp);
        if cancel.is_cancelled() {
            return Err(ExecutorError::Cancelled);
        }
        let workspace = temp.path().to_path_buf();

        let mut command = Command::new(&self.config.program);
        command
            .args(&self.config.args)
            .current_dir(&workspace)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env(
                "YOINK_WORKER_MAX_BYTES",
                self.config.max_artifact_bytes.to_string(),
            )
            .env(
                "YOINK_WORKER_MAX_ITEMS",
                self.config.max_artifacts.to_string(),
            )
            .env(
                "YOINK_WORKER_TOTAL_MAX_BYTES",
                self.config.max_workspace_bytes.to_string(),
            )
            .env(
                "YOINK_WORKER_ALLOWED_PROXY_ORIGINS",
                serde_json::to_string(&self.config.allowed_proxy_origins)
                    .map_err(|_| ExecutorError::InvalidConfig)?,
            )
            .env(
                "YOINK_WORKER_SOCKET_TIMEOUT_SECS",
                self.config.socket_timeout.as_secs().to_string(),
            )
            .env(
                "YOINK_WORKER_PROCESSING_TIMEOUT_SECS",
                self.config.processing_timeout.as_secs().to_string(),
            )
            .env(
                "YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS",
                serde_json::to_string(&self.config.allowed_private_origins)
                    .map_err(|_| ExecutorError::InvalidConfig)?,
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);

        if self.youtube_auth_adapter && !matches!(request, ExecutionRequest::FullContent(_)) {
            command.env(
                "YOINK_YOUTUBE_AUTH_SOCKET",
                "/run/yoink-youtube-auth/adapter.sock",
            );
        }
        let mut child = command.spawn().map_err(|_| ExecutorError::Spawn)?;
        let pid = child.id().ok_or(ExecutorError::Spawn)?;
        let group_pid = i32::try_from(pid)
            .ok()
            .filter(|pid| *pid > 0)
            .ok_or(ExecutorError::Spawn)?;
        let mut guard = ProcessGroupGuard(Pid::from_raw(group_pid));
        let result = async {
        let stdin = child.stdin.take().ok_or(ExecutorError::Spawn)?;
        let stdout = child.stdout.take().ok_or(ExecutorError::Spawn)?;
        let stderr = child.stderr.take().ok_or(ExecutorError::Spawn)?;

        let write_task = AbortOnDropHandle::new(self.spawn_operation_task(music_operation, async move {
            let mut stdin = stdin;
            stdin.write_all(&frame).await.map_err(|_| ())?;
            stdin.write_all(b"\n").await.map_err(|_| ())?;
            stdin.shutdown().await.map_err(|_| ())
        }));
        let max_out = if matches!(request, ExecutionRequest::Search(_)) { self.config.max_output_bytes.min(yoink_worker_protocol::MAX_SEARCH_FRAME_BYTES) } else { self.config.max_output_bytes };
        let request_kind_full = matches!(request, ExecutionRequest::FullContent(_));
        let expected_version = version;
        let expected_operation_id = operation_id;
        let expected_attempt = attempt;
        let progress_sender = progress.clone();
        let out_task = AbortOnDropHandle::new(self.spawn_operation_task(music_operation, async move {
            let result = read_frames_mode(
                stdout,
                max_out,
                expected_version,
                expected_operation_id,
                expected_attempt,
                progress_sender,
                !request_kind_full,
            ).await;
            if result.is_err() {
                terminate_group(pid);
            }
            result
        }));
        let err_task = AbortOnDropHandle::new(self.spawn_operation_task(music_operation, async move {
            let within_limit = drain_stderr(stderr, max_out).await;
            if !within_limit {
                terminate_group(pid);
            }
            within_limit
        }));

        let deadline = Instant::now() + self.config.timeout;
        let mut monitor = tokio::time::interval(Duration::from_millis(100));
        monitor.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let status = loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    terminate_group(pid);
                    return Err(ExecutorError::Cancelled);
                }
                _ = tokio::time::sleep_until(deadline) => {
                    terminate_group(pid);
                    return Err(ExecutorError::Timeout);
                }
                _ = monitor.tick() => {
                    let root = workspace.clone();
                    let budget = self.config.max_workspace_bytes;
                    let keepalive = music_operation.then(||temp.clone());
                    let check_task = self.spawn_operation_blocking(music_operation, move || { let _keepalive = keepalive; workspace_usage(&root, budget) });
                    let check = tokio::select! {
                        _ = cancel.cancelled() => {
                            terminate_group(pid);
                            return Err(ExecutorError::Cancelled);
                        }
                        _ = tokio::time::sleep_until(deadline) => {
                            terminate_group(pid);
                            return Err(ExecutorError::Timeout);
                        }
                        result = check_task => result,
                    };
                    match check {
                        Ok(Ok(())) => {}
                        Ok(Err(WorkspaceError::Unsafe)) => return Err(ExecutorError::UnsafeArtifact),
                        Ok(Err(WorkspaceError::Limit)) => return Err(ExecutorError::ResourceLimit),
                        _ => return Err(ExecutorError::Io),
                    }
                }
                result = child.wait() => {
                    let status = result.map_err(|_| ExecutorError::Io)?;
                    terminate_group(pid);
                    break status;
                }
            }
        };

        let joined = tokio::select! {
            _ = cancel.cancelled() => {
                terminate_group(pid);
                return Err(ExecutorError::Cancelled);
            }
            _ = tokio::time::sleep_until(deadline) => {
                terminate_group(pid);
                return Err(ExecutorError::Timeout);
            }
            result = async {
                let write_result = write_task.join().await.map_err(|_| ExecutorError::Io)?.map_err(|_| ExecutorError::Io);
                let events = out_task.join().await.map_err(|_| ExecutorError::Protocol)??;
                let stderr_ok = err_task.join().await.map_err(|_| ExecutorError::Protocol)?;
                let root = workspace.clone();
                let budget = self.config.max_workspace_bytes;
                let keepalive = music_operation.then(||temp.clone());
                self.spawn_operation_blocking(music_operation, move || { let _keepalive = keepalive; workspace_usage(&root, budget) })
                    .await
                    .map_err(|_| ExecutorError::Io)?
                    .map_err(|error| match error {
                        WorkspaceError::Unsafe => ExecutorError::UnsafeArtifact,
                        WorkspaceError::Limit => ExecutorError::ResourceLimit,
                        WorkspaceError::Io => ExecutorError::Io,
                    })?;
                Ok::<_, ExecutorError>((write_result, events, stderr_ok))
            } => result?,
        };
        let (write_result, events, stderr_ok) = joined;
        write_result?;
        if !stderr_ok {
            return Err(ExecutorError::ResourceLimit);
        }
        if !status.success() {
            return Err(ExecutorError::Exited(status.code()));
        }
        terminate_group(pid);
        let mut terminal = None;
        for event in &events {
            if (event.version, event.operation_id, event.attempt) != (version, operation_id, attempt) { return Err(ExecutorError::Protocol); }
            if event.event.is_terminal() {
                if terminal.is_some() {
                    return Err(ExecutorError::Protocol);
                }
                terminal = Some(event.event.clone());
            } else if terminal.is_some() {
                return Err(ExecutorError::Protocol);
            }
        }
        let event = terminal.ok_or(ExecutorError::Protocol)?;
        if let ExecutionRequest::FullContent(r) = &request {
            if events.len() != 1 { return Err(ExecutorError::Protocol); }
            if std::fs::read_dir(&workspace).map_err(|_| ExecutorError::Io)?.next().is_some() { return Err(ExecutorError::UnsafeArtifact); }
            return match event {
                WorkerEventKind::FullContentCompleted { result } if result.valid_for(r) => Ok(ExecutionOutput::FullContent(result)),
                WorkerEventKind::FullContentFailed { error } => Err(ExecutorError::FullContent(error)),
                _ => Err(ExecutorError::Protocol),
            };
        }
        if matches!(request, ExecutionRequest::Music(r) if !matches!(r.task, yoink_worker_protocol::MusicTask::MusicAudio { .. })) {
            if std::fs::read_dir(&workspace).map_err(|_| ExecutorError::Io)?.next().is_some() { return Err(ExecutorError::UnsafeArtifact); }
            return match event { WorkerEventKind::MusicCompleted { result } => Ok(ExecutionOutput::Music(result)), WorkerEventKind::Failure { class } => Err(ExecutorError::Worker(class)), _ => Err(ExecutorError::Protocol) };
        }
        if matches!(request, ExecutionRequest::Content(_)) {
            if std::fs::read_dir(&workspace).map_err(|_| ExecutorError::Io)?.next().is_some() { return Err(ExecutorError::UnsafeArtifact); }
            return match event {
                WorkerEventKind::ContentCompleted { result } => Ok(ExecutionOutput::Content(result)),
                WorkerEventKind::ContentDenied => Err(ExecutorError::ContentDenied),
                WorkerEventKind::Failure { class } => Err(ExecutorError::Worker(class)),
                _ => Err(ExecutorError::Protocol),
            };
        }
        if matches!(request, ExecutionRequest::Search(_)) {
            return match event {
                WorkerEventKind::SearchCompleted { entries } => {
                    // A metadata-only worker must leave no media or other artifacts.
                    if std::fs::read_dir(&workspace).map_err(|_| ExecutorError::Io)?.next().is_some() { return Err(ExecutorError::UnsafeArtifact); }
                    Ok(ExecutionOutput::Search(entries))
                }
                WorkerEventKind::Failure { class } => Err(ExecutorError::Worker(class)),
                WorkerEventKind::CancellationAcknowledged => Err(ExecutorError::Cancelled),
                _ => Err(ExecutorError::Protocol),
            };
        }
        let WorkerEventKind::Completed { artifacts } = event else {
            return match event {
                WorkerEventKind::Failure { class } => Err(ExecutorError::Worker(class)),
                WorkerEventKind::CancellationAcknowledged => Err(ExecutorError::Cancelled),
                _ => Err(ExecutorError::Protocol),
            };
        };
        if artifacts.is_empty() || artifacts.len() > self.config.max_artifacts {
            return Err(ExecutorError::UnsafeArtifact);
        }
        let mut paths = std::collections::HashSet::with_capacity(artifacts.len());
        let mut total_size = 0u64;
        for descriptor in &artifacts {
            if !paths.insert(descriptor.path.as_str()) {
                return Err(ExecutorError::UnsafeArtifact);
            }
            total_size = total_size.checked_add(descriptor.size_bytes)
                .ok_or(ExecutorError::ResourceLimit)?;
            if total_size > self.config.max_workspace_bytes {
                return Err(ExecutorError::ResourceLimit);
            }
        }
        let limit = self.config.max_artifact_bytes;
        let workspace_limit = self.config.max_workspace_bytes;
        let formats_report = matches!(
            &request,
            ExecutionRequest::Media(WorkerRequest { task: yoink_worker_protocol::TaskKind::Download {
                media_mode: yoink_worker_protocol::MediaMode::Formats
            }, .. })
        );
        let links_report = matches!(&request,
            ExecutionRequest::Media(WorkerRequest { task: yoink_worker_protocol::TaskKind::Download {
                media_mode: yoink_worker_protocol::MediaMode::Links | yoink_worker_protocol::MediaMode::AudioLinks
            }, .. }));
        let allow_media_info = matches!(&request,
            ExecutionRequest::Media(WorkerRequest { task: yoink_worker_protocol::TaskKind::Download {
                media_mode: yoink_worker_protocol::MediaMode::Video | yoink_worker_protocol::MediaMode::Audio | yoink_worker_protocol::MediaMode::Image | yoink_worker_protocol::MediaMode::Best
            }, options: yoink_worker_protocol::DownloadOptions { media_info: true, .. }, .. }));
        let keepalive = music_operation.then(||temp.clone());
        let artifact_task = self.spawn_operation_blocking(music_operation, move || {
            let _keepalive = keepalive;
            let mut validated = Vec::with_capacity(artifacts.len());
            let mut total = 0u64;
            for descriptor in &artifacts {
                if (formats_report || links_report) && artifacts.len() != 1 {
                    return Err(ExecutorError::UnsafeArtifact);
                }
                let mut artifact = load_artifact_kind(&workspace, descriptor, limit, formats_report, links_report)?;
                if !allow_media_info || artifacts.len() != 1 { artifact.media_info = None; }
                total = total.checked_add(artifact.size_bytes).ok_or(ExecutorError::ResourceLimit)?;
                if total > workspace_limit {
                    return Err(ExecutorError::ResourceLimit);
                }
                validated.push(artifact);
            }
            Ok::<_, ExecutorError>(validated)
        });
        let artifacts = tokio::select! {
            _ = cancel.cancelled() => {
                terminate_group(pid);
                return Err(ExecutorError::Cancelled);
            }
            _ = tokio::time::sleep_until(deadline) => {
                terminate_group(pid);
                return Err(ExecutorError::Timeout);
            }
            result = artifact_task => result.map_err(|_| ExecutorError::Io)??,
        };
        Ok(ExecutionOutput::Media(artifacts))
        }.await;
        terminate_group(pid);
        let _ = child.wait().await;
        guard.0 = None;
        result
    }
    fn spawn_operation_task<F>(&self, music: bool, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        if music {
            self.music_tasks.spawn(future)
        } else {
            tokio::spawn(future)
        }
    }
    fn spawn_operation_blocking<F, T>(&self, music: bool, task: F) -> tokio::task::JoinHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        if music {
            self.music_tasks.spawn_blocking(task)
        } else {
            tokio::task::spawn_blocking(task)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn read_frames_mode<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    max_output: usize,
    version: u16,
    operation_id: uuid::Uuid,
    attempt: u32,
    progress: Option<tokio::sync::watch::Sender<Option<WorkerProgress>>>,
    allow_progress: bool,
) -> Result<Vec<WorkerEvent>, ExecutorError> {
    let mut total = 0usize;
    let mut line = Vec::with_capacity(1024);
    let mut frames = Vec::new();
    let mut terminal_seen = false;
    let mut buffer = [0u8; 8192];
    loop {
        let n = reader
            .read(&mut buffer)
            .await
            .map_err(|_| ExecutorError::Io)?;
        if n == 0 {
            if !line.is_empty() {
                if !allow_progress && decode_event_frame(&line).is_ok_and(|e| e.event.is_progress())
                {
                    return Err(ExecutorError::Protocol);
                }
                parse_line(
                    &line,
                    &mut frames,
                    version,
                    operation_id,
                    attempt,
                    &mut terminal_seen,
                    &progress,
                )?;
            }
            return Ok(frames);
        }
        total = total.checked_add(n).ok_or(ExecutorError::ResourceLimit)?;
        if total > max_output {
            return Err(ExecutorError::ResourceLimit);
        }
        for &byte in &buffer[..n] {
            if byte == b'\n' {
                if line.is_empty() {
                    return Err(ExecutorError::Protocol);
                }
                if !allow_progress && decode_event_frame(&line).is_ok_and(|e| e.event.is_progress())
                {
                    return Err(ExecutorError::Protocol);
                }
                parse_line(
                    &line,
                    &mut frames,
                    version,
                    operation_id,
                    attempt,
                    &mut terminal_seen,
                    &progress,
                )?;
                line.clear();
            } else {
                if terminal_seen {
                    return Err(ExecutorError::Protocol);
                }
                if line.len() >= MAX_FRAME_BYTES {
                    return Err(ExecutorError::ResourceLimit);
                }
                line.push(byte);
            }
        }
    }
}

fn parse_line(
    line: &[u8],
    events: &mut Vec<WorkerEvent>,
    version: u16,
    operation_id: uuid::Uuid,
    attempt: u32,
    terminal_seen: &mut bool,
    progress: &Option<tokio::sync::watch::Sender<Option<WorkerProgress>>>,
) -> Result<(), ExecutorError> {
    if line.is_empty() || *terminal_seen {
        return Err(ExecutorError::Protocol);
    }
    let event = decode_event_frame(line).map_err(|_| ExecutorError::Protocol)?;
    if event.version != version || event.operation_id != operation_id || event.attempt != attempt {
        return Err(ExecutorError::Protocol);
    }
    match &event.event {
        WorkerEventKind::Progress { percent, stage } => {
            if let Some(sender) = progress {
                sender.send_replace(Some(WorkerProgress {
                    percent: *percent,
                    stage: *stage,
                }));
            }
        }
        _ => *terminal_seen = true,
    }
    if event.event.is_terminal() {
        events.push(event);
    }
    Ok(())
}

async fn drain_stderr<R: tokio::io::AsyncRead + Unpin>(mut reader: R, max: usize) -> bool {
    let mut total = 0usize;
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => return true,
            Ok(n) => {
                total = match total.checked_add(n) {
                    Some(n) => n,
                    None => return false,
                };
                if total > max {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspaceError {
    Unsafe,
    Limit,
    Io,
}

fn workspace_usage(root: &Path, limit: u64) -> Result<(), WorkspaceError> {
    let mut pending = vec![root.to_path_buf()];
    let mut bytes = 0u64;
    let mut count = 0usize;
    while let Some(dir) = pending.pop() {
        for item in std::fs::read_dir(dir).map_err(|_| WorkspaceError::Io)? {
            let item = item.map_err(|_| WorkspaceError::Io)?;
            count += 1;
            if count > MAX_FILES {
                return Err(WorkspaceError::Limit);
            }
            let meta = item.file_type().map_err(|_| WorkspaceError::Io)?;
            if meta.is_symlink() || (!meta.is_file() && !meta.is_dir()) {
                return Err(WorkspaceError::Unsafe);
            }
            if meta.is_dir() {
                pending.push(item.path());
            } else {
                bytes = bytes
                    .checked_add(item.metadata().map_err(|_| WorkspaceError::Io)?.len())
                    .ok_or(WorkspaceError::Limit)?;
                if bytes > limit {
                    return Err(WorkspaceError::Limit);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn load_artifact(
    root: &Path,
    descriptor: &yoink_worker_protocol::ArtifactDescriptor,
    limit: u64,
    formats_report: bool,
) -> Result<ValidatedArtifact, ExecutorError> {
    load_artifact_kind(root, descriptor, limit, formats_report, false)
}

fn load_artifact_kind(
    root: &Path,
    descriptor: &yoink_worker_protocol::ArtifactDescriptor,
    limit: u64,
    formats_report: bool,
    links_report: bool,
) -> Result<ValidatedArtifact, ExecutorError> {
    let path = Path::new(descriptor.path.as_str());
    let components: Vec<_> = path.components().collect();
    let parts: Vec<_> = components
        .iter()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(*part),
            _ => None,
        })
        .collect();
    if parts.is_empty() || parts.len() != components.len() {
        return Err(ExecutorError::UnsafeArtifact);
    }
    let root_fd = File::open(root).map_err(|_| ExecutorError::Io)?;
    let mut dir_fd: OwnedFd = root_fd.into();
    for part in &parts[..parts.len() - 1] {
        if part.to_str().is_none() {
            return Err(ExecutorError::UnsafeArtifact);
        }
        dir_fd = openat(
            &dir_fd,
            *part,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| ExecutorError::UnsafeArtifact)?;
    }
    let final_name = parts.last().ok_or(ExecutorError::UnsafeArtifact)?;
    let fd = openat(
        &dir_fd,
        *final_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| ExecutorError::UnsafeArtifact)?;
    let stat = fstat(&fd).map_err(|_| ExecutorError::UnsafeArtifact)?;
    if stat.st_mode & libc_mode_type_mask() != libc_mode_regular()
        || stat.st_nlink != 1
        || stat.st_size < 0
        || stat.st_size as u64 != descriptor.size_bytes
        || descriptor.size_bytes > limit
    {
        return Err(ExecutorError::UnsafeArtifact);
    }
    let mut file = File::from(fd);
    if links_report {
        if descriptor.path.as_str() != "links.json"
            || descriptor.media_type != "application/json"
            || descriptor.size_bytes > yoink_worker_protocol::MAX_LINK_REPORT_BYTES as u64
        {
            return Err(ExecutorError::UnsafeArtifact);
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(yoink_worker_protocol::MAX_LINK_REPORT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ExecutorError::Io)?;
        if bytes.len() as u64 != descriptor.size_bytes
            || yoink_worker_protocol::StreamLinks::decode(&bytes).is_err()
        {
            return Err(ExecutorError::UnsafeArtifact);
        }
        file.seek(std::io::SeekFrom::Start(0))
            .map_err(|_| ExecutorError::Io)?;
        return Ok(ValidatedArtifact {
            file,
            filename: "links.json".into(),
            media_type: "application/json".into(),
            size_bytes: descriptor.size_bytes,
            metadata: None,
            media_info: None,
        });
    }
    if formats_report {
        if descriptor.path.as_str() != "formats.txt"
            || descriptor.media_type != "text/plain"
            || descriptor.size_bytes > 64 * 1024
        {
            return Err(ExecutorError::UnsafeArtifact);
        }
        let mut bytes = Vec::with_capacity(descriptor.size_bytes as usize);
        (&mut file)
            .take(64 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ExecutorError::Io)?;
        if bytes.len() as u64 != descriptor.size_bytes
            || !valid_formats_report(&bytes)
            || file.seek(std::io::SeekFrom::Start(0)).is_err()
        {
            return Err(ExecutorError::UnsafeArtifact);
        }
        return Ok(ValidatedArtifact {
            file,
            filename: "formats.txt".into(),
            media_type: "text/plain".into(),
            size_bytes: descriptor.size_bytes,
            metadata: descriptor.metadata.clone(),
            media_info: None,
        });
    }
    let mut header = [0u8; MAX_SNIFF_BYTES];
    let read = file.read(&mut header).map_err(|_| ExecutorError::Io)?;
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|_| ExecutorError::Io)?;
    let header = &header[..read];
    let (filename, media_type, valid_magic) = match descriptor.media_type.as_str() {
        "video/mp4" => (
            "media.mp4",
            "video/mp4",
            header.len() >= 8 && &header[4..8] == b"ftyp",
        ),
        "video/webm" => (
            "media.webm",
            "video/webm",
            header.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]),
        ),
        "video/x-matroska" => (
            "media.mkv",
            "video/x-matroska",
            header.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]),
        ),
        "audio/mpeg" => (
            "audio.mp3",
            "audio/mpeg",
            header.starts_with(b"ID3")
                || (header.len() >= 2 && header[0] == 0xff && header[1] & 0xe0 == 0xe0),
        ),
        "audio/mp4" => (
            "audio.m4a",
            "audio/mp4",
            header.len() >= 8 && &header[4..8] == b"ftyp",
        ),
        "audio/ogg" => ("audio.ogg", "audio/ogg", header.starts_with(b"OggS")),
        "audio/opus" => ("audio.opus", "audio/opus", header.starts_with(b"OggS")),
        "image/jpeg" => (
            "image.jpg",
            "image/jpeg",
            header.starts_with(&[0xff, 0xd8, 0xff]),
        ),
        "image/png" => (
            "image.png",
            "image/png",
            header.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
        ),
        "image/webp" => (
            "image.webp",
            "image/webp",
            header.len() >= 12 && &header[..4] == b"RIFF" && &header[8..12] == b"WEBP",
        ),
        "image/gif" => (
            "image.gif",
            "image/gif",
            header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a"),
        ),
        "text/vtt" => ("subtitles.vtt", "text/vtt", subtitle_magic(header, true)),
        "application/x-subrip" => (
            "subtitles.srt",
            "application/x-subrip",
            subtitle_magic(header, false),
        ),
        "text/x-ssa" => ("subtitles.ass", "text/x-ssa", ssa_magic(header)),
        "application/zip" => (
            "archive.zip",
            "application/zip",
            header.starts_with(b"PK\x03\x04")
                || header.starts_with(b"PK\x05\x06")
                || header.starts_with(b"PK\x07\x08"),
        ),
        _ => return Err(ExecutorError::UnsafeArtifact),
    };
    if !valid_magic {
        return Err(ExecutorError::UnsafeArtifact);
    }
    let filename = if matches!(
        media_type,
        "text/vtt" | "application/x-subrip" | "text/x-ssa"
    ) {
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or(ExecutorError::UnsafeArtifact)?;
        if name.len() > 64
            || !name.starts_with("subtitles-")
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
            || !match media_type {
                "text/vtt" => name.ends_with(".vtt"),
                "application/x-subrip" => name.ends_with(".srt"),
                "text/x-ssa" => name.ends_with(".ass") || name.ends_with(".ssa"),
                _ => false,
            }
        {
            return Err(ExecutorError::UnsafeArtifact);
        }
        name
    } else {
        filename
    };
    Ok(ValidatedArtifact {
        file,
        filename: filename.into(),
        media_type: media_type.into(),
        size_bytes: descriptor.size_bytes,
        metadata: descriptor.metadata.clone(),
        media_info: descriptor.media_info.clone(),
    })
}

fn valid_formats_report(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() > 64 * 1024 || !bytes.ends_with(b"\n") {
        return false;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let mut lines = text.split_terminator('\n');
    if lines.next() != Some("Yoink source formats v1")
        || lines.next() != Some("ID\tEXT\tWIDTH\tHEIGHT\tFPS\tVIDEO\tAUDIO\tPROTOCOL")
    {
        return false;
    }
    let rows: Vec<_> = lines.collect();
    if !(1..=100).contains(&rows.len()) {
        return false;
    }
    rows.into_iter().all(|row| {
        let cells: Vec<_> = row.split('\t').collect();
        cells.len() == 8
            && (cells[0] == "-"
                || (!cells[0].is_empty()
                    && cells[0].len() <= 10
                    && cells[0].bytes().all(|b| b.is_ascii_digit())))
            && matches!(
                cells[1],
                "mp4" | "webm" | "m4a" | "mp3" | "ogg" | "opus" | "flv" | "mkv" | "unknown"
            )
            && bounded_integer(cells[2], 16_384)
            && bounded_integer(cells[3], 16_384)
            && valid_fps(cells[4])
            && matches!(
                cells[5],
                "h264" | "h265" | "av1" | "vp9" | "vp8" | "none" | "unknown"
            )
            && matches!(
                cells[6],
                "aac" | "mp3" | "opus" | "vorbis" | "flac" | "none" | "unknown"
            )
            && matches!(cells[7], "https" | "http" | "hls" | "dash" | "unknown")
    })
}

fn bounded_integer(value: &str, max: u32) -> bool {
    value == "-"
        || (value.parse::<u32>().is_ok_and(|n| n <= max)
            && !value.is_empty()
            && value.bytes().all(|b| b.is_ascii_digit()))
}

fn valid_fps(value: &str) -> bool {
    if value == "-" {
        return true;
    }
    let Some((whole, fraction)) = value.split_once('.') else {
        return value.parse::<u16>().is_ok_and(|n| n <= 1000)
            && !value.is_empty()
            && value.bytes().all(|b| b.is_ascii_digit());
    };
    !whole.is_empty()
        && whole.bytes().all(|b| b.is_ascii_digit())
        && !fraction.is_empty()
        && fraction.len() <= 3
        && fraction.bytes().all(|b| b.is_ascii_digit())
        && value
            .parse::<f32>()
            .is_ok_and(|n| (0.0..=1000.0).contains(&n))
}

// Only sniff the signature here. The worker validates the complete bounded text,
// event fields and cue timing before publishing any subtitle artifact.
fn ssa_magic(header: &[u8]) -> bool {
    let header = header.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(header);
    header.starts_with(b"[Script Info]")
        && header.get(13).is_some_and(|b| matches!(b, b'\r' | b'\n'))
}

fn subtitle_magic(header: &[u8], vtt: bool) -> bool {
    let header = header.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(header);
    if vtt {
        return header.starts_with(b"WEBVTT")
            && header
                .get(6)
                .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'));
    }
    let mut lines = header.split(|b| *b == b'\n');
    let number = lines.next().unwrap_or_default();
    let number = number.strip_suffix(b"\r").unwrap_or(number);
    if number.is_empty() || !number.iter().all(u8::is_ascii_digit) {
        return false;
    }
    let Some(line) = lines.next().and_then(|line| std::str::from_utf8(line).ok()) else {
        return false;
    };
    let mut parts = line.split_ascii_whitespace();
    let timestamp = |value: &str| {
        let mut fields = value.split(':');
        let (Some(hours), Some(minutes), Some(seconds)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return false;
        };
        hours.len() >= 2
            && hours.bytes().all(|b| b.is_ascii_digit())
            && minutes.len() == 2
            && minutes.parse::<u8>().is_ok_and(|v| v < 60)
            && seconds.len() == 6
            && seconds.as_bytes()[2] == b','
            && seconds[..2].parse::<u8>().is_ok_and(|v| v < 60)
            && seconds[3..].bytes().all(|b| b.is_ascii_digit())
            && fields.next().is_none()
    };
    parts.next().is_some_and(timestamp)
        && parts.next() == Some("-->")
        && parts.next().is_some_and(timestamp)
}

fn libc_mode_type_mask() -> u32 {
    0o170000
}
fn libc_mode_regular() -> u32 {
    0o100000
}

fn terminate_group(pid: u32) {
    if let Ok(raw_pid) = i32::try_from(pid)
        && let Some(pid) = Pid::from_raw(raw_pid)
    {
        let _ = kill_process_group(pid, Signal::KILL);
    }
}

struct AbortOnDropHandle<T>(Option<tokio::task::JoinHandle<T>>);

impl<T> AbortOnDropHandle<T> {
    fn new(handle: tokio::task::JoinHandle<T>) -> Self {
        Self(Some(handle))
    }

    async fn join(mut self) -> Result<T, tokio::task::JoinError> {
        let handle = self.0.take().expect("task handle is present");
        handle.await
    }
}

impl<T> Drop for AbortOnDropHandle<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

struct ProcessGroupGuard(Option<Pid>);
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let _ = kill_process_group(pid, Signal::KILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitle_sniff_checks_signatures_without_decoding_truncated_body_text() {
        assert!(subtitle_magic(
            b"WEBVTT\n\n00:00.000 --> 00:02.000\n\xd0",
            true
        ));
        assert!(subtitle_magic(b"\xef\xbb\xbfWEBVTT\r\n", true));
        assert!(subtitle_magic(
            b"1\r\n00:00:00,000 --> 00:00:02,000\r\n\xd0",
            false
        ));
        for bytes in [b"WEBVTTX".as_slice(), b"<html>text</html>", b"WEBVTT"] {
            assert!(!subtitle_magic(bytes, true));
        }
        for bytes in [
            b"1\n00:00:00,000 --> garbage".as_slice(),
            b"x\n00:00:00,000 --> 00:00:02,000",
            b"1\n00:99:00,000 --> 00:00:02,000",
        ] {
            assert!(!subtitle_magic(bytes, false));
        }
    }
    use std::io::Write;

    #[test]
    fn ssa_sniff_requires_header_boundary_and_allows_bom_and_truncated_unicode() {
        for bytes in [
            b"[Script Info]\nScriptType: v4+\n; \xd0".as_slice(),
            b"\xef\xbb\xbf[Script Info]\r\nScriptType: v4\r\n",
        ] {
            assert!(ssa_magic(bytes));
        }
        for bytes in [
            b"[Script Info]".as_slice(),
            b"[Script Info]garbage\n",
            b"[Script InfoX]\n",
            b"[Events]\nDialogue: 0,0:00:00.00,0:00:02.00,text",
            b"<html>[Script Info]</html>",
            b"[Script Info]\0\n",
        ] {
            assert!(!ssa_magic(bytes));
        }
    }

    #[test]
    fn ass_and_ssa_artifact_documents_preserve_bytes_filename_and_mime() {
        for (extension, script_type, bom) in [("ass", "v4+", ""), ("ssa", "v4", "\u{feff}")] {
            let temp = tempfile::tempdir().unwrap();
            let name = format!("subtitles-ru-orig.{extension}");
            let bytes = format!(
                "{bom}[Script Info]\r\nScriptType: {script_type}\r\n[Events]\r\nFormat: Start, End, Text\r\nDialogue: 0:00:00.00,0:00:02.00,Текст, with comma\r\n"
            );
            std::fs::write(temp.path().join(&name), bytes.as_bytes()).unwrap();
            let descriptor = yoink_worker_protocol::ArtifactDescriptor {
                media_info: None,
                metadata: None,
                path: yoink_worker_protocol::ArtifactPath::parse(&name).unwrap(),
                size_bytes: bytes.len() as u64,
                media_type: "text/x-ssa".into(),
            };
            let mut artifact = load_artifact(temp.path(), &descriptor, 4096, false).unwrap();
            assert_eq!(artifact.filename, name);
            assert_eq!(artifact.media_type, "text/x-ssa");
            let mut actual = Vec::new();
            artifact.file.read_to_end(&mut actual).unwrap();
            assert_eq!(actual, bytes.as_bytes());
            assert!(matches!(
                load_artifact(temp.path(), &descriptor, bytes.len() as u64 - 1, false),
                Err(ExecutorError::UnsafeArtifact)
            ));
        }
    }

    #[test]
    fn ssa_artifacts_reject_mime_signature_and_filename_mismatches() {
        let temp = tempfile::tempdir().unwrap();
        for (name, mime, bytes) in [
            (
                "subtitles-en.exe",
                "text/x-ssa",
                b"[Script Info]\n".as_slice(),
            ),
            ("subtitles-en.vtt", "text/x-ssa", b"[Script Info]\n"),
            ("subtitles-en.srt", "text/x-ssa", b"[Script Info]\n"),
            ("subtitles-en.ass", "text/vtt", b"[Script Info]\n"),
            (
                "subtitles-en.ssa",
                "application/x-subrip",
                b"[Script Info]\n",
            ),
            ("subtitles-en.ass", "text/x-ass", b"[Script Info]\n"),
            (
                "subtitles-en.ass",
                "text/x-ssa",
                b"<html>not subtitles</html>",
            ),
            ("subtitles-en.ssa", "text/x-ssa", b"WEBVTT\n"),
            ("other.ass", "text/x-ssa", b"[Script Info]\n"),
            ("subtitles-en secret.ass", "text/x-ssa", b"[Script Info]\n"),
        ] {
            std::fs::write(temp.path().join(name), bytes).unwrap();
            let descriptor = yoink_worker_protocol::ArtifactDescriptor {
                media_info: None,
                metadata: None,
                path: yoink_worker_protocol::ArtifactPath::parse(name).unwrap(),
                size_bytes: bytes.len() as u64,
                media_type: mime.into(),
            };
            assert!(
                matches!(
                    load_artifact(temp.path(), &descriptor, 4096, false),
                    Err(ExecutorError::UnsafeArtifact)
                ),
                "{name} / {mime}"
            );
        }
    }

    #[test]
    fn artifact_debug_does_not_include_payload() {
        let artifact = ValidatedArtifact {
            media_info: None,
            metadata: None,
            file: File::open("/dev/null").unwrap(),
            filename: "media.mp4".into(),
            media_type: "video/mp4".into(),
            size_bytes: 6,
        };
        assert!(!format!("{artifact:?}").contains("secret"));
    }

    #[test]
    fn validates_limits_without_spawning() {
        let config = ExecutorConfig {
            program: PathBuf::from("/bin/true"),
            args: vec![],
            workspace_root: PathBuf::from("/tmp"),
            timeout: Duration::from_secs(1),
            processing_timeout: Duration::from_secs(1),
            socket_timeout: Duration::from_secs(1),
            max_artifact_bytes: 1024,
            max_workspace_bytes: 1024,
            max_output_bytes: 1024,
            max_artifacts: 100,
            allowed_proxy_origins: vec![],
            allowed_private_origins: vec![],
        };
        assert!(Executor::new(config.clone()).is_ok());
        let original = Executor::new(config.clone()).unwrap();
        let lowered = original
            .with_operation_limits(Duration::from_secs(1), 512, 1)
            .unwrap();
        assert_eq!(lowered.config.max_artifact_bytes, 512);
        assert_eq!(lowered.config.max_workspace_bytes, 512);
        assert_eq!(lowered.config.max_artifacts, 1);
        assert!(std::sync::Arc::ptr_eq(
            &lowered.full_admission,
            &original.full_admission
        ));
        let unchanged = original
            .with_operation_limits(Duration::from_secs(86400), 8192, 500)
            .unwrap();
        assert_eq!(unchanged.config.timeout, config.timeout);
        assert_eq!(unchanged.config.max_artifact_bytes, 1024);
        assert_eq!(unchanged.config.max_artifacts, config.max_artifacts);
        for (duration, bytes, count) in [
            (Duration::ZERO, 1, 1),
            (Duration::from_millis(500), 1, 1),
            (Duration::from_secs(86401), 1, 1),
            (Duration::from_secs(1), 0, 1),
            (Duration::from_secs(1), 1, 0),
            (Duration::from_secs(1), 1, 501),
        ] {
            assert!(
                original
                    .with_operation_limits(duration, bytes, count)
                    .is_err()
            );
        }
        let mut fractional = config.clone();
        fractional.processing_timeout = Duration::from_millis(500);
        assert!(matches!(
            Executor::new(fractional),
            Err(ExecutorError::InvalidConfig)
        ));
        let mut over_cap = config;
        over_cap.processing_timeout = Duration::from_secs(2);
        assert!(matches!(
            Executor::new(over_cap),
            Err(ExecutorError::InvalidConfig)
        ));
    }

    fn process_executor(mode: &str, root: &Path) -> Executor {
        process_executor_args(mode, root, Duration::from_secs(3), Vec::new())
    }
    #[tokio::test]
    async fn music_metadata_and_mp3_share_confined_cleanup_and_limits() {
        use yoink_worker_protocol::{
            MusicCredentials, MusicProvider, MusicRequest, MusicTask, PROTOCOL_VERSION,
        };
        let root = tempfile::tempdir().unwrap();
        let mut request = MusicRequest {
            version: PROTOCOL_VERSION,
            operation_id: uuid::Uuid::new_v4(),
            attempt: 1,
            task: MusicTask::MusicParse {
                provider: MusicProvider::Spotify,
                url: yoink_worker_protocol::HttpSourceUrl::parse(
                    "https://open.spotify.com/track/abc",
                )
                .unwrap(),
            },
            proxy_url: None,
            credentials: MusicCredentials::default(),
            request_timeout_ms: 1000,
            response_max_bytes: 1024,
            max_requests: 10,
        };
        assert!(
            process_executor("music-metadata", root.path())
                .execute_music(&request, CancellationToken::new(), Duration::from_secs(3))
                .await
                .is_ok()
        );
        assert!(matches!(
            process_executor("music-artifact", root.path())
                .execute_music(&request, CancellationToken::new(), Duration::from_secs(3))
                .await,
            Err(ExecutorError::UnsafeArtifact)
        ));
        request.task = MusicTask::MusicAudio {
            url: yoink_worker_protocol::HttpSourceUrl::parse(
                "https://music.youtube.com/watch?v=abcdefghijk",
            )
            .unwrap(),
            title: "Song".into(),
            artist: "Artist".into(),
            thumbnail_url: None,
        };
        let artifacts = process_executor("music-audio", root.path())
            .execute_music_audio(&request, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(artifacts[0].media_type, "audio/mpeg");
        assert_eq!(artifacts[0].size_bytes, 15);
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
    }

    fn process_executor_args(
        mode: &str,
        root: &Path,
        timeout: Duration,
        extra: Vec<String>,
    ) -> Executor {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.mjs");
        let mut args = vec![fixture.to_string_lossy().into_owned(), mode.to_owned()];
        args.extend(extra);
        Executor::new(ExecutorConfig {
            program: PathBuf::from("/usr/bin/node"),
            args,
            workspace_root: root.to_path_buf(),
            timeout,
            processing_timeout: timeout.min(Duration::from_secs(300)),
            socket_timeout: Duration::from_secs(2),
            max_artifact_bytes: 1024 * 1024,
            max_workspace_bytes: 2 * 1024 * 1024,
            max_output_bytes: 1024 * 1024,
            max_artifacts: 100,
            allowed_proxy_origins: vec!["https://proxy.example:8443".into()],
            allowed_private_origins: vec!["http://127.0.0.1:8080".into()],
        })
        .unwrap()
    }

    fn request() -> WorkerRequest {
        WorkerRequest {
            version: yoink_worker_protocol::PROTOCOL_VERSION,
            operation_id: uuid::Uuid::new_v4(),
            attempt: 1,
            source_url: yoink_worker_protocol::HttpSourceUrl::parse("https://example.test/media")
                .unwrap(),
            task: yoink_worker_protocol::TaskKind::Download {
                media_mode: yoink_worker_protocol::MediaMode::Video,
            },
            options: yoink_worker_protocol::DownloadOptions::default(),
            network: None,
        }
    }

    #[tokio::test]
    async fn content_typed_results_denial_bounds_cancellation_and_reap() {
        let request = yoink_worker_protocol::ContentRequest {
            version: yoink_worker_protocol::PROTOCOL_VERSION,
            operation_id: uuid::Uuid::new_v4(),
            attempt: 1,
            proxy_url: None,
            task: yoink_worker_protocol::ContentStage::ContentTrafilatura {
                html: "body".into(),
            },
        };
        for (mode, expected) in [
            ("content-denied", ExecutorError::ContentDenied),
            ("content-bound", ExecutorError::Protocol),
            ("content-artifact", ExecutorError::UnsafeArtifact),
            ("hang", ExecutorError::Timeout),
        ] {
            let temp = tempfile::tempdir().unwrap();
            assert_eq!(
                process_executor_args(mode, temp.path(), Duration::from_secs(1), vec![])
                    .execute_content(&request, CancellationToken::new())
                    .await
                    .unwrap_err(),
                expected
            );
            assert!(temp.path().read_dir().unwrap().next().is_none());
        }
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            process_executor("content-ok", temp.path())
                .execute_content(&request, CancellationToken::new())
                .await
                .unwrap()
                .text,
            "body"
        );
        let cancel = CancellationToken::new();
        let executor = process_executor("hang", temp.path());
        let child_cancel = cancel.clone();
        let task =
            tokio::spawn(async move { executor.execute_content(&request, child_cancel).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
        assert_eq!(task.await.unwrap().unwrap_err(), ExecutorError::Cancelled);
        assert!(temp.path().read_dir().unwrap().next().is_none());
    }

    fn full_request() -> yoink_worker_protocol::FullContentRequest {
        use yoink_worker_protocol::*;
        FullContentRequest {
            version: 2,
            operation_id: uuid::Uuid::new_v4(),
            attempt: 1,
            task: FullContentTask::FullContentFetch,
            url: HttpSourceUrl::parse("https://example.com/article").unwrap(),
            options: FullContentOptions {
                transcript_only: false,
                transcript_languages: vec!["fr".into(), "en".into()],
                max_chars: 100,
                github_token: None,
            },
            timeout_secs: 2.0,
            max_response_bytes: 1024,
            proxy_url: None,
        }
    }
    fn full_fixture(mode: &str, root: &Path, marker: Option<&Path>) -> Executor {
        let mut executor = process_executor(mode, root);
        executor.config.args = vec![
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/full_content_fixture.mjs")
                .to_string_lossy()
                .into_owned(),
            mode.into(),
        ];
        if let Some(marker) = marker {
            executor
                .config
                .args
                .push(marker.to_string_lossy().into_owned());
        }
        executor
    }
    #[test]
    fn full_content_fixed_launcher_no_media_permissions_or_credentials() {
        let root = tempfile::tempdir().unwrap();
        let original = full_fixture("ok", root.path(), None).with_youtube_auth_adapter();
        let prepared = original.full_content_executor(&full_request());
        assert_eq!(
            prepared.config.program,
            Path::new("/usr/local/bin/yoink-worker-launcher")
        );
        assert!(prepared.config.args.is_empty());
        assert!(prepared.config.allowed_private_origins.is_empty());
        assert!(!prepared.youtube_auth_adapter);
        assert_eq!(
            prepared.config.allowed_proxy_origins,
            original.config.allowed_proxy_origins
        );
        assert_eq!(prepared.config.max_output_bytes, MAX_FRAME_BYTES + 1);
        // Original anonymous/media instance is not mutated by full fetch preparation.
        assert!(!original.config.args.is_empty());
        assert!(original.youtube_auth_adapter);
    }

    #[tokio::test]
    async fn full_content_correlation_one_terminal_and_result_validation() {
        for (mode, expected) in [
            ("wrong-id", ExecutorError::Protocol),
            ("v1", ExecutorError::Protocol),
            ("duplicate", ExecutorError::Protocol),
            ("progress", ExecutorError::Protocol),
            ("cap", ExecutorError::Protocol),
            ("video", ExecutorError::Protocol),
            ("artifact", ExecutorError::UnsafeArtifact),
            (
                "error",
                ExecutorError::FullContent(
                    yoink_worker_protocol::FullContentError::ContentTooLarge,
                ),
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let executor = full_fixture(mode, root.path(), None);
            assert_eq!(
                executor
                    .execute_full_content_owned(
                        full_request(),
                        CancellationToken::new(),
                        executor.clone()
                    )
                    .await
                    .unwrap_err(),
                expected,
                "{mode}"
            );
            executor.shutdown_full_content().await;
            assert!(root.path().read_dir().unwrap().next().is_none());
        }
        let root = tempfile::tempdir().unwrap();
        let executor = full_fixture("ok", root.path(), None);
        assert!(
            executor
                .execute_full_content_owned(
                    full_request(),
                    CancellationToken::new(),
                    executor.clone()
                )
                .await
                .unwrap()
                .text
                .contains("real")
        );
        executor.shutdown_full_content().await;
    }
    #[tokio::test]
    async fn full_content_dropped_future_cancel_shutdown_reap_group_and_workspace() {
        for mode in ["drop", "cancel", "shutdown", "timeout"] {
            let root = tempfile::tempdir().unwrap();
            let markers = tempfile::tempdir().unwrap();
            let marker = markers.path().join("child");
            let mut executor = full_fixture("hang", root.path(), Some(&marker));
            executor.config.timeout = Duration::from_secs(1);
            let runner = executor.clone();
            let cancel = CancellationToken::new();
            let trigger = cancel.clone();
            let task = tokio::spawn(async move {
                runner
                    .execute_full_content_owned(full_request(), cancel, runner.clone())
                    .await
            });
            tokio::time::timeout(Duration::from_secs(3), async {
                while !marker.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            let parent = std::fs::read_to_string(marker.with_extension("parent")).unwrap();
            let child = std::fs::read_to_string(&marker).unwrap();
            match mode {
                "drop" => {
                    task.abort();
                    let _ = task.await;
                }
                "cancel" => {
                    trigger.cancel();
                    assert_eq!(task.await.unwrap().unwrap_err(), ExecutorError::Cancelled);
                }
                "shutdown" => {
                    executor.shutdown_full_content().await;
                    assert_eq!(task.await.unwrap().unwrap_err(), ExecutorError::Cancelled);
                }
                _ => {
                    assert_eq!(task.await.unwrap().unwrap_err(), ExecutorError::Timeout);
                }
            }
            tokio::time::timeout(Duration::from_secs(3), executor.shutdown_full_content())
                .await
                .unwrap();
            assert!(
                !Path::new(&format!("/proc/{parent}")).exists(),
                "parent not reaped"
            );
            // Group SIGKILL delivery can precede the descendant's scheduler turn.
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    match std::fs::read_to_string(format!("/proc/{child}/stat")) {
                        Ok(stat) if stat.split_whitespace().nth(2) != Some("Z") => {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                        Ok(_) => break,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::NotFound
                                || error.raw_os_error()
                                    == Some(rustix::io::Errno::SRCH.raw_os_error()) =>
                        {
                            break;
                        }
                        Err(error) => panic!("could not inspect descendant: {error}"),
                    }
                }
            })
            .await
            .expect("descendant remained live after group termination");
            let ticks = marker.with_extension("ticks");
            let before = std::fs::read(&ticks).unwrap_or_default();
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(before, std::fs::read(&ticks).unwrap_or_default());
            assert!(root.path().read_dir().unwrap().next().is_none());
        }
    }

    #[tokio::test]
    async fn music_metadata_and_audio_drop_cancel_shutdown_timeout_reap_and_cleanup() {
        use yoink_worker_protocol::*;
        for audio in [false, true] {
            for mode in ["drop", "cancel", "shutdown", "timeout"] {
                let root = tempfile::tempdir().unwrap();
                let markers = tempfile::tempdir().unwrap();
                let marker = markers.path().join("child");
                let mut executor = full_fixture("hang", root.path(), Some(&marker));
                executor.config.timeout = Duration::from_secs(1);
                let url =
                    HttpSourceUrl::parse("https://music.youtube.com/watch?v=abcdefghijk").unwrap();
                let request = MusicRequest {
                    version: PROTOCOL_VERSION,
                    operation_id: uuid::Uuid::new_v4(),
                    attempt: 1,
                    task: if audio {
                        MusicTask::MusicAudio {
                            url,
                            title: "Song".into(),
                            artist: "Artist".into(),
                            thumbnail_url: None,
                        }
                    } else {
                        MusicTask::MusicParse {
                            provider: MusicProvider::Ytmusic,
                            url,
                        }
                    },
                    proxy_url: None,
                    credentials: MusicCredentials::default(),
                    request_timeout_ms: 1000,
                    response_max_bytes: 1024,
                    max_requests: 1,
                };
                let runner = executor.clone();
                let cancel = CancellationToken::new();
                let trigger = cancel.clone();
                let task = tokio::spawn(async move {
                    if audio {
                        runner
                            .execute_music_audio(&request, cancel)
                            .await
                            .map(|_| ())
                    } else {
                        runner
                            .execute_music(&request, cancel, Duration::from_secs(1))
                            .await
                            .map(|_| ())
                    }
                });
                tokio::time::timeout(Duration::from_secs(3), async {
                    while !marker.exists() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .unwrap();
                let parent = std::fs::read_to_string(marker.with_extension("parent")).unwrap();
                let child = std::fs::read_to_string(&marker).unwrap();
                match mode {
                    "drop" => {
                        task.abort();
                        let _ = task.await;
                    }
                    "cancel" => {
                        trigger.cancel();
                        assert_eq!(task.await.unwrap(), Err(ExecutorError::Cancelled));
                    }
                    "shutdown" => {
                        executor.shutdown_music().await;
                        assert_eq!(task.await.unwrap(), Err(ExecutorError::Cancelled));
                    }
                    _ => {
                        assert_eq!(task.await.unwrap(), Err(ExecutorError::Timeout));
                    }
                }
                tokio::time::timeout(Duration::from_secs(3), executor.shutdown_music())
                    .await
                    .unwrap();
                assert!(
                    !Path::new(&format!("/proc/{parent}")).exists(),
                    "Music parent not reaped"
                );
                tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        match std::fs::read_to_string(format!("/proc/{child}/stat")) {
                            Ok(stat) if stat.split_whitespace().nth(2) != Some("Z") => {
                                tokio::time::sleep(Duration::from_millis(10)).await
                            }
                            Ok(_) => break,
                            Err(error)
                                if error.kind() == std::io::ErrorKind::NotFound
                                    || error.raw_os_error()
                                        == Some(rustix::io::Errno::SRCH.raw_os_error()) =>
                            {
                                break;
                            }
                            Err(error) => panic!("Music descendant inspection failed: {error}"),
                        }
                    }
                })
                .await
                .unwrap();
                let ticks = marker.with_extension("ticks");
                let before = std::fs::read(&ticks).unwrap_or_default();
                tokio::time::sleep(Duration::from_millis(100)).await;
                assert_eq!(before, std::fs::read(&ticks).unwrap_or_default());
                assert!(root.path().read_dir().unwrap().next().is_none());
                assert_eq!(executor.music_tasks.len(), 0);
            }
        }
    }
    fn search_request() -> yoink_worker_protocol::SearchRequest {
        yoink_worker_protocol::SearchRequest {
            version: yoink_worker_protocol::PROTOCOL_VERSION,
            operation_id: uuid::Uuid::new_v4(),
            attempt: 1,
            task: yoink_worker_protocol::SearchTask::YoutubeSearch {
                query: "secret-query-marker".into(),
            },
            network: None,
        }
    }

    #[tokio::test]
    async fn typed_search_bounds_failures_and_empty_result_use_same_cleanup() {
        for (mode, expected) in [
            ("search-url", ExecutorError::Protocol),
            ("search-duplicate", ExecutorError::Protocol),
            ("search-artifact", ExecutorError::UnsafeArtifact),
            ("search-overflow", ExecutorError::ResourceLimit),
            ("search-exit", ExecutorError::Exited(Some(7))),
            (
                "search-denial",
                ExecutorError::Worker(yoink_worker_protocol::FailureClass::SourceUnavailable),
            ),
            ("hang", ExecutorError::Timeout),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let error = process_executor_args(mode, temp.path(), Duration::from_secs(1), vec![])
                .execute_search(
                    &search_request(),
                    CancellationToken::new(),
                    Duration::from_secs(1),
                )
                .await
                .unwrap_err();
            assert_eq!(error, expected, "{mode}");
            assert!(!format!("{error:?} {error}").contains("secret-query-marker"));
            assert!(temp.path().read_dir().unwrap().next().is_none());
        }
        let temp = tempfile::tempdir().unwrap();
        let entries = process_executor("search-zero", temp.path())
            .execute_search(
                &search_request(),
                CancellationToken::new(),
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        assert!(entries.is_empty());
        assert!(temp.path().read_dir().unwrap().next().is_none());
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            process_executor("hang", temp.path())
                .execute_search(&search_request(), cancel, Duration::from_secs(1))
                .await
                .unwrap_err(),
            ExecutorError::Cancelled
        );
        assert!(temp.path().read_dir().unwrap().next().is_none());
    }

    #[tokio::test]
    async fn fractional_remaining_search_budget_still_reaps_timed_out_worker() {
        let temp = tempfile::tempdir().unwrap();
        let error = process_executor("hang", temp.path())
            .execute_search(
                &search_request(),
                CancellationToken::new(),
                Duration::from_millis(150),
            )
            .await
            .unwrap_err();
        assert_eq!(error, ExecutorError::Timeout);
        assert!(temp.path().read_dir().unwrap().next().is_none());
    }

    #[tokio::test]
    async fn worker_receives_all_typed_environment_mappings() {
        let temp = tempfile::tempdir().unwrap();
        process_executor("environment", temp.path())
            .execute(&request(), CancellationToken::new())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn node_fixture_process_success_and_progress() {
        let temp = tempfile::tempdir().unwrap();
        let (sender, receiver) = tokio::sync::watch::channel(None);
        let artifact = process_executor("progress", temp.path())
            .execute_with_progress(&request(), CancellationToken::new(), Some(sender))
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(artifact.filename, "media.mp4");
        assert_eq!(artifact.media_type, "video/mp4");
        assert_eq!(artifact.size_bytes, 16);
        assert_eq!(artifact.file().metadata().unwrap().len(), 16);
        let latest = receiver.borrow().unwrap();
        assert_eq!(latest.percent, 50.0);
        assert_eq!(
            latest.stage,
            yoink_worker_protocol::ProgressStage::Processing
        );
    }

    #[test]
    fn large_sparse_artifact_is_returned_as_an_open_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large.mp4");
        let mut source = File::create(&path).unwrap();
        source
            .write_all(&[0, 0, 0, 24, b'f', b't', b'y', b'p'])
            .unwrap();
        source.set_len(1024 * 1024 * 1024).unwrap();
        drop(source);
        let descriptor = yoink_worker_protocol::ArtifactDescriptor {
            media_info: None,
            metadata: None,
            path: yoink_worker_protocol::ArtifactPath::parse("large.mp4").unwrap(),
            size_bytes: 1024 * 1024 * 1024,
            media_type: "video/mp4".into(),
        };
        let artifact =
            load_artifact(temp.path(), &descriptor, 2 * 1024 * 1024 * 1024, false).unwrap();
        assert_eq!(artifact.size_bytes, 1024 * 1024 * 1024);
        assert_eq!(
            artifact.file().metadata().unwrap().len(),
            artifact.size_bytes
        );
        assert_eq!(
            artifact.file().metadata().unwrap().len(),
            1024 * 1024 * 1024
        );
    }

    #[tokio::test]
    async fn accepts_multiple_safe_mime_types_and_rejects_aggregate_bounds() {
        let temp = tempfile::tempdir().unwrap();
        let artifacts = process_executor("mixed-types", temp.path())
            .execute(&request(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            artifacts
                .iter()
                .map(|artifact| artifact.media_type.as_str())
                .collect::<Vec<_>>(),
            vec!["audio/mpeg", "image/png", "application/zip"]
        );
        for (mode, expected) in [
            ("duplicate", ExecutorError::UnsafeArtifact),
            ("many", ExecutorError::UnsafeArtifact),
            ("aggregate-overlimit", ExecutorError::ResourceLimit),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let result = process_executor(mode, temp.path())
                .execute(&request(), CancellationToken::new())
                .await;
            assert_eq!(result.unwrap_err(), expected, "fixture mode {mode}");
            assert!(temp.path().read_dir().unwrap().next().is_none());
        }
    }

    #[tokio::test]
    async fn does_not_publish_progress_from_stale_worker_identity() {
        let temp = tempfile::tempdir().unwrap();
        let (sender, receiver) = tokio::sync::watch::channel(None);
        let result = process_executor("identity-progress", temp.path())
            .execute_with_progress(&request(), CancellationToken::new(), Some(sender))
            .await;
        assert_eq!(result.unwrap_err(), ExecutorError::Protocol);
        assert!(receiver.borrow().is_none());
    }

    #[tokio::test]
    async fn node_fixture_rejects_protocol_resource_and_artifact_failures() {
        let cases = [
            ("bad-json", ExecutorError::Protocol),
            ("version", ExecutorError::Protocol),
            ("attempt", ExecutorError::Protocol),
            ("path", ExecutorError::Protocol),
            ("multiple-terminal", ExecutorError::Protocol),
            ("eof-before-terminal", ExecutorError::Protocol),
            ("stdout-partial", ExecutorError::Protocol),
            ("nonzero", ExecutorError::Exited(Some(7))),
            ("missing-artifact", ExecutorError::UnsafeArtifact),
            ("symlink", ExecutorError::UnsafeArtifact),
            ("parent-symlink", ExecutorError::UnsafeArtifact),
            ("traversal", ExecutorError::Protocol),
            ("hardlink", ExecutorError::UnsafeArtifact),
            ("fifo", ExecutorError::UnsafeArtifact),
            ("magic-mismatch", ExecutorError::UnsafeArtifact),
            ("size-mismatch", ExecutorError::UnsafeArtifact),
            ("oversize", ExecutorError::ResourceLimit),
            ("stderr-overflow", ExecutorError::ResourceLimit),
            ("workspace-filecount", ExecutorError::ResourceLimit),
            ("workspace-bytecap", ExecutorError::ResourceLimit),
            ("duplicate", ExecutorError::UnsafeArtifact),
            ("many", ExecutorError::UnsafeArtifact),
            ("aggregate-overlimit", ExecutorError::ResourceLimit),
        ];
        for (mode, expected) in cases {
            let temp = tempfile::tempdir().unwrap();
            let result = process_executor(mode, temp.path())
                .execute(&request(), CancellationToken::new())
                .await;
            let actual = result.expect_err(&format!("fixture mode {mode} must be rejected"));
            assert_eq!(actual, expected, "fixture mode {mode}");
            assert!(
                temp.path().read_dir().unwrap().next().is_none(),
                "workspace root not empty for {mode}"
            );
        }
    }

    #[tokio::test]
    async fn timeout_covers_hung_protocol_and_stdin_workers() {
        for mode in ["hang", "stdout-partial-hang", "stdin-hang"] {
            let temp = tempfile::tempdir().unwrap();
            let executor =
                process_executor_args(mode, temp.path(), Duration::from_secs(1), Vec::new());
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                executor.execute(&request(), CancellationToken::new()),
            )
            .await;
            assert_eq!(
                result.unwrap().unwrap_err(),
                ExecutorError::Timeout,
                "fixture mode {mode}"
            );
            assert!(temp.path().read_dir().unwrap().next().is_none());
        }
    }

    #[tokio::test]
    async fn cancellation_terminates_worker_and_cleans_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let token = CancellationToken::new();
        let cancel = token.clone();
        let task = tokio::spawn(async move {
            process_executor("cancel", &root)
                .execute(&request(), token)
                .await
        });
        tokio::task::yield_now().await;
        cancel.cancel();
        assert_eq!(task.await.unwrap().unwrap_err(), ExecutorError::Cancelled);
        assert!(temp.path().read_dir().unwrap().next().is_none());
    }

    #[tokio::test]
    async fn descendants_are_killed_on_completion_timeout_cancel_and_future_drop() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("descendant-marker");
        let marker_arg = marker.to_string_lossy().into_owned();
        let pid_file = temp.path().join("worker-pid");
        let result = process_executor_args(
            "descendant",
            temp.path(),
            Duration::from_secs(3),
            vec![marker_arg.clone(), pid_file.to_string_lossy().into_owned()],
        )
        .execute(&request(), CancellationToken::new())
        .await;
        result.unwrap();
        let worker_pid: u32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
        assert!(
            !Path::new(&format!("/proc/{worker_pid}")).exists(),
            "worker process was not reaped before execute returned"
        );
        assert_marker_stops(&marker).await;
        std::fs::remove_file(&marker).unwrap();
        std::fs::remove_file(&pid_file).unwrap();
        assert!(temp.path().read_dir().unwrap().next().is_none());

        for mode in ["cancel", "timeout-descendant"] {
            let marker = temp.path().join(format!("{mode}-marker"));
            let args = vec![marker.to_string_lossy().into_owned()];
            let token = CancellationToken::new();
            let cancel = token.clone();
            let timeout = if mode == "cancel" {
                Duration::from_secs(30)
            } else {
                Duration::from_secs(1)
            };
            let executor = process_executor_args(mode, temp.path(), timeout, args);
            let task = tokio::spawn(async move { executor.execute(&request(), token).await });
            wait_for_marker(&marker).await;
            if mode == "cancel" {
                cancel.cancel();
            }
            assert!(task.await.unwrap().is_err());
            assert_marker_stops(&marker).await;
            std::fs::remove_file(&marker).unwrap();
            assert!(temp.path().read_dir().unwrap().next().is_none());
        }

        let marker = temp.path().join("drop-marker");
        let executor = process_executor_args(
            "timeout-descendant",
            temp.path(),
            Duration::from_secs(30),
            vec![marker.to_string_lossy().into_owned()],
        );
        let task =
            tokio::spawn(
                async move { executor.execute(&request(), CancellationToken::new()).await },
            );
        wait_for_marker(&marker).await;
        task.abort();
        let _ = task.await;
        assert_marker_stops(&marker).await;
        std::fs::remove_file(&marker).unwrap();
        assert!(temp.path().read_dir().unwrap().next().is_none());
    }

    async fn wait_for_marker(marker: &Path) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            if marker.exists() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("descendant did not create marker");
    }

    async fn assert_marker_stops(marker: &Path) {
        wait_for_marker(marker).await;
        let first = std::fs::read(marker).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let second = std::fs::read(marker).unwrap();
        assert_eq!(first, second, "descendant kept writing after cleanup");
    }
}

#[cfg(test)]
mod direct_links_artifact_tests {
    use super::*;
    #[test]
    fn direct_links_are_distinct_bounded_confined_and_cannot_be_format_reports() {
        let temp = tempfile::tempdir().unwrap();
        let bytes =
            br#"{"video_url":"https://cdn.example/video?signature=fixture","audio_url":null}"#;
        std::fs::write(temp.path().join("links.json"), bytes).unwrap();
        let mut descriptor = yoink_worker_protocol::ArtifactDescriptor {
            path: yoink_worker_protocol::ArtifactPath::parse("links.json").unwrap(),
            size_bytes: bytes.len() as u64,
            media_type: "application/json".into(),
            metadata: None,
            media_info: None,
        };
        let a = load_artifact_kind(temp.path(), &descriptor, 4096, false, true).unwrap();
        assert_eq!(a.filename, "links.json");
        assert!(!format!("{a:?}").contains("signature"));
        assert!(load_artifact_kind(temp.path(), &descriptor, 4096, true, false).is_err());
        assert!(load_artifact_kind(temp.path(), &descriptor, 4096, false, false).is_err());
        descriptor.media_type = "text/plain".into();
        assert!(load_artifact_kind(temp.path(), &descriptor, 4096, false, true).is_err());
        descriptor.media_type = "application/json".into();
        std::fs::remove_file(temp.path().join("links.json")).unwrap();
        std::fs::write(temp.path().join("outside"), bytes).unwrap();
        std::os::unix::fs::symlink("outside", temp.path().join("links.json")).unwrap();
        assert!(load_artifact_kind(temp.path(), &descriptor, 4096, false, true).is_err());
    }
}
