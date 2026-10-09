use axum::{
    Extension, Json, Router,
    extract::{DefaultBodyLimit, FromRequest, Multipart, Path, Query, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use std::{
    io::Read,
    path::{Component, Path as FilePath},
};
use yoink_stats::{
    Error,
    http::AuthenticatedUser,
    imports::{ImportJobs, ImportStatus},
};

pub(super) fn router(jobs: ImportJobs) -> Router {
    let limit = jobs.config().max_bytes.saturating_add(64 * 1024);
    Router::new()
        .route("/import", post(upload))
        .route(
            "/import/by-path",
            post(by_path).layer(DefaultBodyLimit::max(32 * 1024)),
        )
        .route("/import/{job_id}", get(status))
        .layer(DefaultBodyLimit::max(limit))
        .with_state(jobs)
}
#[derive(Deserialize)]
struct Chat {
    chat_id: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ByPath {
    path: String,
    chat_id: i64,
}
async fn status(
    State(jobs): State<ImportJobs>,
    Extension(actor): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    Ok(match jobs.status(actor.0, &id).await? {
        Some(s) => Json(s).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"detail":"job_not_found"})),
        )
            .into_response(),
    })
}
async fn upload(
    State(jobs): State<ImportJobs>,
    Extension(actor): Extension<AuthenticatedUser>,
    Query(chat): Query<Chat>,
    mut multipart: Multipart,
) -> Result<Json<ImportStatus>, Error> {
    jobs.authorize(actor.0).await?;
    let input = jobs.reserve_input()?;
    if chat.chat_id >= 0 {
        return Err(Error::Invalid);
    }
    let bytes = tokio::time::timeout(jobs.config().timeout, async {
        let mut file = None;
        while let Some(mut field) = multipart.next_field().await.map_err(|_| Error::Invalid)? {
            if field.name() != Some("file")
                || file.is_some()
                || !field.file_name().is_some_and(|s| s.ends_with(".json"))
            {
                return Err(Error::Invalid);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = field.chunk().await.map_err(|_| Error::Invalid)? {
                if chunk.len() > jobs.config().max_bytes.saturating_sub(bytes.len()) {
                    return Err(Error::Invalid);
                }
                bytes.extend_from_slice(&chunk);
            }
            file = Some(bytes);
        }
        file.ok_or(Error::Invalid)
    })
    .await
    .map_err(|_| Error::Unavailable)??;
    jobs.start_reserved(actor.0, chat.chat_id, bytes, input)
        .await
        .map(Json)
}
async fn by_path(
    State(jobs): State<ImportJobs>,
    Extension(actor): Extension<AuthenticatedUser>,
    request: Request,
) -> Result<Json<ImportStatus>, Error> {
    jobs.authorize(actor.0).await?;
    let input = jobs.reserve_input()?;
    let (chat, bytes) = tokio::time::timeout(jobs.config().timeout, async {
        let Json(body) = Json::<ByPath>::from_request(request, &())
            .await
            .map_err(|_| Error::Invalid)?;
        if body.chat_id >= 0 {
            return Err(Error::Invalid);
        }
        let root = jobs.config().root.clone();
        let max = jobs.config().max_bytes;
        let bytes = jobs
            .blocking(input.clone(), move || read_confined(&root, &body.path, max))
            .await?;
        Ok::<_, Error>((body.chat_id, bytes))
    })
    .await
    .map_err(|_| Error::Unavailable)??;
    jobs.start_reserved(actor.0, chat, bytes, input)
        .await
        .map(Json)
}
fn read_confined(root: &FilePath, path: &str, max: usize) -> Result<Vec<u8>, Error> {
    let file = open_confined(root, path)?;
    if !file.metadata().map_err(|_| Error::Invalid)?.is_file() {
        return Err(Error::Invalid);
    }
    let mut bytes = Vec::new();
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Invalid)?;
    if bytes.len() > max {
        return Err(Error::Invalid);
    }
    Ok(bytes)
}
fn open_confined(root: &FilePath, path: &str) -> Result<std::fs::File, Error> {
    use rustix::fs::{CWD, Mode, OFlags, openat};
    if path.len() > 4096 || root.as_os_str().len() > 4096 || !path.ends_with(".json") {
        return Err(Error::Invalid);
    }
    let path = FilePath::new(path);
    let relative = if path.is_absolute() {
        path.strip_prefix(root).map_err(|_| Error::Invalid)?
    } else {
        path
    };
    let mut dir = openat(
        CWD,
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| Error::Invalid)?;
    // Resolve every directory relative to a retained descriptor. No symlink or rename can escape it.
    for c in root.components() {
        match c {
            Component::RootDir => (),
            Component::Normal(name) => {
                dir = openat(
                    &dir,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| Error::Invalid)?;
            }
            _ => return Err(Error::Invalid),
        }
    }
    let components: Vec<_> = relative.components().collect();
    if components.is_empty() {
        return Err(Error::Invalid);
    }
    for (i, c) in components.iter().enumerate() {
        let Component::Normal(name) = c else {
            return Err(Error::Invalid);
        };
        let mut flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        if i + 1 < components.len() {
            flags |= OFlags::DIRECTORY;
        }
        dir = openat(&dir, *name, flags, Mode::empty()).map_err(|_| Error::Invalid)?;
    }
    Ok(std::fs::File::from(dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    use tower::{ServiceExt, service_fn};
    use yoink_identity::{IdentityStore, TrustedProfile};

    struct Members;
    impl yoink_stats::MembershipProvider for Members {
        fn lookup(&self, _: i64, _: i64) -> yoink_stats::MembershipFuture<'_> {
            Box::pin(async { Err(Error::Unavailable) })
        }
        fn lookup_name(&self, _: Option<i64>, _: i64) -> yoink_stats::NameFuture<'_> {
            Box::pin(async { Ok(None) })
        }
    }
    struct Fixture {
        dir: tempfile::TempDir,
        store: yoink_storage::Store,
        stats: yoink_stats::Stats,
        jobs: ImportJobs,
    }
    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
                    .unwrap();
            }
            let store = yoink_storage::Store::open(
                &dir.path().join("stats.db"),
                true,
                Duration::from_secs(2),
            )
            .await
            .unwrap();
            store.migrate().await.unwrap();
            let identities = Arc::new(IdentityStore::new(store.database().clone(), Some(1), false));
            for user_id in [1, 2] {
                identities
                    .register(&TrustedProfile {
                        user_id,
                        username: None,
                        first_name: None,
                        photo_url: None,
                    })
                    .await
                    .unwrap();
            }
            let stats = yoink_stats::Stats::open(
                store.database().clone(),
                identities,
                Arc::new(Members),
                None,
                dir.path().join("search"),
            )
            .await
            .unwrap();
            let jobs = ImportJobs::new(
                stats.clone(),
                yoink_stats::imports::ImportConfig {
                    root: dir.path().to_owned(),
                    max_bytes: 4096,
                    max_messages: 100,
                    max_jobs: 1,
                    timeout: Duration::from_secs(2),
                },
            )
            .unwrap();
            std::fs::write(dir.path().join("result.json"), br#"{"messages":[{"id":1,"type":"message","date":"2026-01-01T00:00:00","text":"hello"}]}"#).unwrap();
            Self {
                dir,
                store,
                stats,
                jobs,
            }
        }
        async fn request(&self, actor: i64, body: Body) -> Response {
            let app = router(self.jobs.clone()).layer(Extension(AuthenticatedUser(actor)));
            tokio::time::timeout(
                Duration::from_secs(1),
                app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/import/by-path")
                        .header("content-type", "application/json")
                        .body(body)
                        .unwrap(),
                ),
            )
            .await
            .unwrap()
            .unwrap()
        }
        async fn close(self) {
            self.jobs.cancel_all();
            tokio::time::timeout(Duration::from_secs(2), self.jobs.drain())
                .await
                .unwrap();
            self.stats.shutdown().await.unwrap();
            self.store.close().await.unwrap();
        }
    }
    fn pending_body(polls: Arc<AtomicUsize>) -> Body {
        let service = service_fn(move |chunk: Result<axum::body::Bytes, axum::Error>| {
            let polls = polls.clone();
            async move {
                let chunk = chunk.map_err(std::io::Error::other)?;
                std::future::poll_fn(|_| {
                    polls.fetch_add(1, Ordering::SeqCst);
                    std::task::Poll::<()>::Pending
                })
                .await;
                Ok::<_, std::io::Error>(chunk)
            }
        });
        Body::from_stream(service.call_all(Body::from("probe").into_data_stream()))
    }
    #[tokio::test]
    async fn by_path_authority_and_capacity_reject_before_polling_body() {
        let f = Fixture::new().await;
        let polls = Arc::new(AtomicUsize::new(0));
        assert_eq!(
            f.request(2, pending_body(polls.clone())).await.status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        let input = f.jobs.reserve_input().unwrap();
        assert_eq!(
            f.request(1, pending_body(polls.clone())).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        drop(input);
        // The probe itself must actually observe a body poll when admission succeeds.
        let app = router(f.jobs.clone()).layer(Extension(AuthenticatedUser(1)));
        let request = Request::builder()
            .method("POST")
            .uri("/import/by-path")
            .header("content-type", "application/json")
            .body(pending_body(polls.clone()))
            .unwrap();
        {
            let mut response = Box::pin(app.oneshot(request));
            tokio::time::timeout(
                Duration::from_secs(1),
                std::future::poll_fn(|cx| {
                    assert!(std::future::Future::poll(response.as_mut(), cx).is_pending());
                    if polls.load(Ordering::SeqCst) > 0 {
                        std::task::Poll::Ready(())
                    } else {
                        std::task::Poll::Pending
                    }
                }),
            )
            .await
            .unwrap();
        }
        assert!(polls.load(Ordering::SeqCst) > 0);
        assert!(f.jobs.reserve_input().is_ok());
        f.close().await;
    }
    #[tokio::test]
    async fn by_path_has_a_dedicated_32_kib_limit() {
        let f = Fixture::new().await;
        let json = r#"{"path":"result.json","chat_id":-100}"#;
        let mut body = json.to_owned();
        body.push_str(&" ".repeat(32 * 1024 + 1 - body.len()));
        assert!(body.len() < f.jobs.config().max_bytes + 64 * 1024);
        assert_eq!(
            f.request(1, Body::from(body)).await.status(),
            StatusCode::BAD_REQUEST
        );
        assert!(f.jobs.status(1, "1").await.unwrap().is_none());
        let mut body = json.to_owned();
        body.push_str(&" ".repeat(32 * 1024 - body.len()));
        assert_eq!(
            f.request(1, Body::from(body)).await.status(),
            StatusCode::OK
        );
        assert!(f.dir.path().join("result.json").exists());
        f.close().await;
    }
    #[test]
    fn by_path_reservation_transfers_to_real_job_before_another_body_is_polled() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap()
            .block_on(async {
                let f = Fixture::new().await;
                let response = f
                    .request(1, Body::from(r#"{"path":"result.json","chat_id":-100}"#))
                    .await;
                assert_eq!(response.status(), StatusCode::OK);
                let bytes = axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap();
                let status: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                let id = status["job_id"].as_str().unwrap();
                let (entered, started) = tokio::sync::oneshot::channel();
                let (release, wait) = std::sync::mpsc::channel();
                // The HTTP caller has returned; hold the sole blocking thread while its job parses.
                let blocker = tokio::task::spawn_blocking(move || {
                    entered.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(2)).unwrap();
                });
                tokio::time::timeout(Duration::from_secs(1), started)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    f.jobs.status(1, id).await.unwrap().unwrap().status,
                    "running"
                );
                let polls = Arc::new(AtomicUsize::new(0));
                assert_eq!(
                    f.request(1, pending_body(polls.clone())).await.status(),
                    StatusCode::SERVICE_UNAVAILABLE
                );
                assert_eq!(polls.load(Ordering::SeqCst), 0);
                release.send(()).unwrap();
                blocker.await.unwrap();
                tokio::time::timeout(Duration::from_secs(1), async {
                    loop {
                        let status = f.jobs.status(1, id).await.unwrap().unwrap();
                        if status.status != "running" {
                            assert_eq!(status.status, "done");
                            assert_eq!(status.inserted, 1);
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                assert!(f.jobs.reserve_input().is_ok());
                f.close().await;
            });
    }
    #[test]
    fn confined_files_are_bounded_and_never_deleted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("result.json"), b"{}{}").unwrap();
        assert_eq!(
            read_confined(dir.path(), "result.json", 4).unwrap(),
            b"{}{}"
        );
        assert!(read_confined(dir.path(), "result.json", 3).is_err());
        assert!(read_confined(dir.path(), "../result.json", 4).is_err());
        assert!(read_confined(dir.path(), "/tmp/result.json", 4).is_err());
        assert!(dir.path().join("result.json").exists());
        #[cfg(unix)]
        {
            rustix::fs::mkfifoat(
                rustix::fs::CWD,
                dir.path().join("fifo.json"),
                rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            )
            .unwrap();
            assert!(read_confined(dir.path(), "fifo.json", 4).is_err());
            std::os::unix::fs::symlink(
                dir.path().join("result.json"),
                dir.path().join("link.json"),
            )
            .unwrap();
            assert!(read_confined(dir.path(), "link.json", 4).is_err());
            std::os::unix::fs::symlink(dir.path(), dir.path().join("nested")).unwrap();
            assert!(read_confined(dir.path(), "nested/result.json", 4).is_err());
        }
    }
}
