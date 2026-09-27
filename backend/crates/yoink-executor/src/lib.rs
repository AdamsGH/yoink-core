use std::{
    fmt,
    fs::File,
    io::Read,
    os::fd::OwnedFd,
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

const MAX_ARTIFACT: u64 = 50 * 1024 * 1024;
const MAX_WORKSPACE: u64 = 200 * 1024 * 1024;
const MAX_OUTPUT: usize = 4 * 1024 * 1024;
const MAX_FILES: usize = 32;

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
    pub socket_timeout: Duration,
    pub max_artifact_bytes: u64,
    pub max_workspace_bytes: u64,
    pub max_output_bytes: usize,
    pub allowed_private_origins: Vec<String>,
}

pub struct ValidatedArtifact {
    pub bytes: Vec<u8>,
    pub filename: String,
    pub media_type: String,
}

impl fmt::Debug for ValidatedArtifact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidatedArtifact")
            .field("bytes", &"[REDACTED]")
            .field("filename", &self.filename)
            .field("media_type", &self.media_type)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExecutorError {
    #[error("invalid executor configuration")]
    InvalidConfig,
    #[error("worker could not be started")]
    Spawn,
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
            || config.socket_timeout.is_zero()
            || config.socket_timeout > Duration::from_secs(3600)
            || !(1..=MAX_ARTIFACT).contains(&config.max_artifact_bytes)
            || config.max_workspace_bytes < config.max_artifact_bytes
            || config.max_workspace_bytes > MAX_WORKSPACE
            || !(1..=MAX_OUTPUT).contains(&config.max_output_bytes)
        {
            return Err(ExecutorError::InvalidConfig);
        }
        Ok(Self { config })
    }

    pub async fn execute(
        &self,
        request: &WorkerRequest,
        cancel: CancellationToken,
    ) -> Result<ValidatedArtifact, ExecutorError> {
        request.validate().map_err(|_| ExecutorError::Protocol)?;
        let prefix = format!("{}-{}-", request.operation_id, request.attempt);
        let workspace_root = self.config.workspace_root.clone();
        let temp = tokio::task::spawn_blocking(move || {
            tempfile::Builder::new()
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
                "YOINK_WORKER_SOCKET_TIMEOUT_SECS",
                self.config.socket_timeout.as_secs().to_string(),
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
        let out_task = AbortOnDropHandle::new(tokio::spawn(async move {
            let result = read_frames(stdout, max_out).await;
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
            return Err(ExecutorError::Spawn);
        }
        terminate_group(pid);
        let mut terminal = None;
        for event in events {
            check_event_identity(&event, request)?;
            if event.event.is_terminal() && terminal.replace(event.event).is_some() {
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
        if artifacts.len() != 1 {
            return Err(ExecutorError::UnsafeArtifact);
        }
        let descriptor = artifacts[0].clone();
        let limit = self.config.max_artifact_bytes;
        let artifact_task =
            tokio::task::spawn_blocking(move || load_artifact(&workspace, &descriptor, limit));
        let artifact = tokio::select! {
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
        Ok(artifact)
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
) -> Result<Vec<WorkerEvent>, ExecutorError> {
    let mut total = 0usize;
    let mut line = Vec::with_capacity(1024);
    let mut frames = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = reader
            .read(&mut byte)
            .await
            .map_err(|_| ExecutorError::Io)?;
        if n == 0 {
            if !line.is_empty() {
                parse_line(&line, &mut frames)?;
            }
            return Ok(frames);
        }
        total = total.checked_add(1).ok_or(ExecutorError::ResourceLimit)?;
        if total > max_output || line.len() >= MAX_FRAME_BYTES {
            return Err(ExecutorError::ResourceLimit);
        }
        if byte[0] == b'\n' {
            parse_line(&line, &mut frames)?;
            line.clear();
        } else {
            line.push(byte[0]);
        }
    }
}

fn parse_line(line: &[u8], events: &mut Vec<WorkerEvent>) -> Result<(), ExecutorError> {
    if line.is_empty() {
        return Err(ExecutorError::Protocol);
    }
    events.push(decode_event_frame(line).map_err(|_| ExecutorError::Protocol)?);
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
    let file = File::from(fd);
    let mut bytes = Vec::with_capacity((descriptor.size_bytes as usize).min(limit as usize));
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ExecutorError::Io)?;
    if bytes.len() as u64 != descriptor.size_bytes || bytes.len() as u64 > limit {
        return Err(ExecutorError::UnsafeArtifact);
    }
    let (filename, media_type, valid_magic) = match descriptor.media_type.as_str() {
        "video/mp4" => (
            "media.mp4",
            "video/mp4",
            bytes.len() >= 8 && &bytes[4..8] == b"ftyp",
        ),
        "video/webm" => (
            "media.webm",
            "video/webm",
            bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]),
        ),
        _ => return Err(ExecutorError::UnsafeArtifact),
    };
    if !valid_magic {
        return Err(ExecutorError::UnsafeArtifact);
    }
    Ok(ValidatedArtifact {
        bytes,
        filename: filename.into(),
        media_type: media_type.into(),
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

    #[test]
    fn artifact_debug_does_not_include_payload() {
        let artifact = ValidatedArtifact {
            bytes: b"secret".to_vec(),
            filename: "media.mp4".into(),
            media_type: "video/mp4".into(),
        };
        assert!(!format!("{artifact:?}").contains("secret"));
    }

    #[test]
    fn validates_limits_without_spawning() {
        let result = Executor::new(ExecutorConfig {
            program: PathBuf::from("/bin/true"),
            args: vec![],
            workspace_root: PathBuf::from("/tmp"),
            timeout: Duration::from_secs(1),
            socket_timeout: Duration::from_secs(1),
            max_artifact_bytes: 1024,
            max_workspace_bytes: 1024,
            max_output_bytes: 1024,
            allowed_private_origins: vec![],
        });
        assert!(result.is_ok());
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
            socket_timeout: Duration::from_secs(2),
            max_artifact_bytes: 1024 * 1024,
            max_workspace_bytes: 2 * 1024 * 1024,
            max_output_bytes: 1024 * 1024,
            allowed_private_origins: vec![],
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
        }
    }

    #[tokio::test]
    async fn node_fixture_process_success_and_progress() {
        let temp = tempfile::tempdir().unwrap();
        let artifact = process_executor("progress", temp.path())
            .execute(&request(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(artifact.filename, "media.mp4");
        assert_eq!(artifact.media_type, "video/mp4");
        assert_eq!(&artifact.bytes[4..8], b"ftyp");
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
            ("nonzero", ExecutorError::Spawn),
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
        ];
        for (mode, expected) in cases {
            let temp = tempfile::tempdir().unwrap();
            let result = process_executor(mode, temp.path())
                .execute(&request(), CancellationToken::new())
                .await;
            assert_eq!(result.unwrap_err(), expected, "fixture mode {mode}");
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
                process_executor_args(mode, temp.path(), Duration::from_millis(250), Vec::new());
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
                Duration::from_millis(800)
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
