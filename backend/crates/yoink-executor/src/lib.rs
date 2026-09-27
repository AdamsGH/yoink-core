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
    FailureClass, MAX_FRAME_BYTES, WorkerEvent, WorkerEventKind, WorkerRequest, decode_event_frame,
    encode_request,
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

impl Executor {
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
        Ok(Self { config })
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
        request.validate().map_err(|_| ExecutorError::Protocol)?;
        let prefix = format!("{}-{}-", request.operation_id, request.attempt);
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
        let frame = encode_request(request).map_err(|_| ExecutorError::Protocol)?;
        let write_task = AbortOnDropHandle::new(tokio::spawn(async move {
            let mut stdin = stdin;
            stdin.write_all(&frame).await.map_err(|_| ())?;
            stdin.write_all(b"\n").await.map_err(|_| ())?;
            stdin.shutdown().await.map_err(|_| ())
        }));
        let max_out = self.config.max_output_bytes;
        let expected_version = request.version;
        let expected_operation_id = request.operation_id;
        let expected_attempt = request.attempt;
        let progress_sender = progress.clone();
        let out_task = AbortOnDropHandle::new(tokio::spawn(async move {
            let result = read_frames(
                stdout,
                max_out,
                expected_version,
                expected_operation_id,
                expected_attempt,
                progress_sender,
            ).await;
            if result.is_err() {
                terminate_group(pid);
            }
            result
        }));
        let err_task = AbortOnDropHandle::new(tokio::spawn(async move {
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
                    let check_task = tokio::task::spawn_blocking(move || workspace_usage(&root, budget));
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
                tokio::task::spawn_blocking(move || workspace_usage(&root, budget))
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
            check_event_identity(event, request)?;
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
        let artifact_task = tokio::task::spawn_blocking(move || {
            let mut validated = Vec::with_capacity(artifacts.len());
            let mut total = 0u64;
            for descriptor in &artifacts {
                let artifact = load_artifact(&workspace, descriptor, limit)?;
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
        Ok(artifacts)
        }.await;
        terminate_group(pid);
        let _ = child.wait().await;
        guard.0 = None;
        result
    }
}

fn check_event_identity(event: &WorkerEvent, request: &WorkerRequest) -> Result<(), ExecutorError> {
    if event.version != request.version
        || event.operation_id != request.operation_id
        || event.attempt != request.attempt
    {
        return Err(ExecutorError::Protocol);
    }
    Ok(())
}

async fn read_frames<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    max_output: usize,
    version: u16,
    operation_id: uuid::Uuid,
    attempt: u32,
    progress: Option<tokio::sync::watch::Sender<Option<WorkerProgress>>>,
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

fn load_artifact(
    root: &Path,
    descriptor: &yoink_worker_protocol::ArtifactDescriptor,
    limit: u64,
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
    Ok(ValidatedArtifact {
        file,
        filename: filename.into(),
        media_type: media_type.into(),
        size_bytes: descriptor.size_bytes,
    })
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
    use std::io::Write;

    #[test]
    fn artifact_debug_does_not_include_payload() {
        let artifact = ValidatedArtifact {
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
            path: yoink_worker_protocol::ArtifactPath::parse("large.mp4").unwrap(),
            size_bytes: 1024 * 1024 * 1024,
            media_type: "video/mp4".into(),
        };
        let artifact = load_artifact(temp.path(), &descriptor, 2 * 1024 * 1024 * 1024).unwrap();
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
