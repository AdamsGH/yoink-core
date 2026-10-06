//! Opt-in pinned Python transport fixture, with Rust encoding and typed decoding.
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};
use yoink_worker_protocol::{
    PROTOCOL_VERSION, SearchRequest, SearchTask, WorkerEventKind, decode_event_frame,
    encode_search_request,
};

#[tokio::test]
#[ignore = "requires the existing pinned worker Docker image"]
async fn rust_encode_pinned_python_parser_extractor_rust_decode() {
    if std::env::var_os("YOINK_SEARCH_WIRE_DOCKER").is_none() {
        return;
    }
    let worker = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../worker")
        .canonicalize()
        .unwrap();
    for (query, count) in [
        ("cats", 8),
        ("zero", 0),
        ("duplicates", 1),
        ("oversized", 8),
        ("strings", 1),
    ] {
        let request = SearchRequest {
            version: PROTOCOL_VERSION,
            operation_id: uuid::Uuid::new_v4(),
            attempt: 1,
            task: SearchTask::YoutubeSearch {
                query: query.into(),
            },
            network: None,
        };
        let name = format!("yoink-inline-wire-{}", request.operation_id);
        let mut child = Command::new("docker")
            .args([
                "run",
                "-i",
                "--rm",
                "--pull",
                "never",
                "--name",
                &name,
                "--network",
                "none",
                "--read-only",
                "--memory",
                "512m",
                "--cpus",
                "2",
                "--pids-limit",
                "128",
                "--tmpfs",
                "/workspace:rw,noexec,nosuid,size=8m",
                "--workdir",
                "/workspace",
                "--mount",
                &format!(
                    "type=bind,source={},target=/worker,readonly",
                    worker.display()
                ),
                "--entrypoint",
                "python",
                "yoink/rust-backend:experimental",
                "-I",
                "-B",
                "/worker/search_fixture.py",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(&encode_search_request(&request).unwrap())
            .await
            .unwrap();
        stdin.shutdown().await.unwrap();
        drop(stdin);
        let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output()).await;
        // Recovery even on timeout/assertion failure: never leave a named container.
        let _ = Command::new("docker")
            .args(["rm", "--force", &name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        let output = output.expect("local extractor fixture timed out").unwrap();
        assert!(
            output.status.success(),
            "pinned parser/engine exited unsuccessfully"
        );
        assert!(
            output.stderr.is_empty(),
            "worker diagnostics must be secret-free and silent on success"
        );
        assert!(output.stdout.len() < yoink_worker_protocol::MAX_SEARCH_FRAME_BYTES);
        let event = decode_event_frame(output.stdout.strip_suffix(b"\n").unwrap()).unwrap();
        assert_eq!(
            (event.version, event.operation_id, event.attempt),
            (request.version, request.operation_id, request.attempt)
        );
        let WorkerEventKind::SearchCompleted { entries } = event.event else {
            panic!("expected metadata terminal event");
        };
        assert_eq!(entries.len(), count);
        if let Some(first) = entries.first() {
            assert_eq!(first.url, "https://www.youtube.com/watch?v=fixture0000");
            assert_eq!(first.duration_seconds, Some(62));
            assert_eq!(first.view_count, Some(1234));
        }
    }
}
