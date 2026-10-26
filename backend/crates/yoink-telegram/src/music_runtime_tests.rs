use super::*;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use yoink_db::query;
use yoink_executor::{Executor, ExecutorConfig, ValidatedArtifact};
use yoink_storage::Store;
use yoink_worker_protocol::MusicRequest;

#[derive(Clone, Copy)]
enum WorkerMode {
    Normal,
    Playback,
    LongMetadata,
    NonMusic,
    PendingSearch,
    ActiveAudio,
    PersistenceBudget,
    MetadataVariation,
    InterleavedObservation,
    LargeArtists,
}
struct ObservationPause {
    reached: tokio::sync::Semaphore,
    resume: tokio::sync::Semaphore,
}
struct Worker {
    executor: Executor,
    mode: WorkerMode,
    started: Arc<std::sync::atomic::AtomicUsize>,
    dropped: Arc<std::sync::atomic::AtomicUsize>,
    pause: Arc<ObservationPause>,
}
struct ProviderDrop(Arc<std::sync::atomic::AtomicUsize>);
impl Drop for ProviderDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}
impl MusicWorker for Worker {
    fn shutdown(&self) -> MusicFuture<'_, ()> {
        Box::pin(async move {
            self.executor.shutdown_music().await;
            Ok(())
        })
    }
    fn lookup(
        &self,
        request: MusicRequest,
        _: CancellationToken,
        _: Duration,
    ) -> MusicFuture<'_, MusicResult> {
        Box::pin(async move {
            if matches!(self.mode, WorkerMode::InterleavedObservation)
                && matches!(&request.task,MusicTask::MusicParse {url,..} if url.as_url().path().ends_with("/def"))
            {
                self.pause.reached.add_permits(1);
                self.pause.resume.acquire().await.unwrap().forget();
            }
            if matches!(self.mode, WorkerMode::MetadataVariation)
                && matches!(request.task, MusicTask::MusicParse { .. })
            {
                self.started
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            if matches!(self.mode, WorkerMode::LargeArtists) {
                self.started
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            if matches!(self.mode, WorkerMode::NonMusic) {
                return Err(MusicError::SourceUnavailable);
            }
            if matches!(self.mode, WorkerMode::PendingSearch)
                && matches!(request.task, MusicTask::MusicSearch { .. })
            {
                let _drop = ProviderDrop(self.dropped.clone());
                self.started
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::future::pending::<()>().await;
            }
            Ok(match request.task {
                MusicTask::MusicParse { .. } => MusicResult::Metadata {
                    metadata: MusicMetadata {
                        title: if matches!(self.mode, WorkerMode::LongMetadata) {
                            "x".repeat(1000)
                        } else if matches!(self.mode, WorkerMode::MetadataVariation) {
                            format!(
                                "Song {}",
                                self.started.load(std::sync::atomic::Ordering::SeqCst)
                            )
                        } else {
                            "Song 🎵".into()
                        },
                        artist: Some(if matches!(self.mode, WorkerMode::LongMetadata) {
                            "a".repeat(100)
                        } else {
                            "Artist <&>".into()
                        }),
                        thumbnail_url: Some(
                            yoink_worker_protocol::HttpSourceUrl::parse(
                                "https://example.com/cover.jpg",
                            )
                            .unwrap(),
                        ),
                    },
                },
                MusicTask::MusicSearch {
                    provider: MusicProvider::Ytmusic,
                    ..
                } if matches!(
                    self.mode,
                    WorkerMode::LongMetadata
                        | WorkerMode::Playback
                        | WorkerMode::MetadataVariation
                        | WorkerMode::InterleavedObservation
                ) =>
                {
                    MusicResult::Link {
                        url: Some(
                            yoink_worker_protocol::HttpSourceUrl::parse(
                                if matches!(self.mode, WorkerMode::MetadataVariation)
                                    && self.started.load(std::sync::atomic::Ordering::SeqCst) > 1
                                {
                                    "https://music.youtube.com/watch?v=lmnopqrstuv"
                                } else {
                                    "https://music.youtube.com/watch?v=abcdefghijk"
                                },
                            )
                            .unwrap(),
                        ),
                    }
                }
                MusicTask::MusicSearch { .. } => MusicResult::Link { url: None },
                MusicTask::MusicArtist { url } => MusicResult::Artist {
                    name: if matches!(self.mode, WorkerMode::LargeArtists) {
                        format!(
                            "{:04}{}",
                            self.started.load(std::sync::atomic::Ordering::SeqCst),
                            "a".repeat(1020)
                        )
                    } else {
                        "Artist 🎵 <&>".into()
                    },
                    genres: if matches!(self.mode, WorkerMode::LargeArtists) {
                        vec!["g".repeat(256); 4]
                    } else {
                        vec!["rock".into()]
                    },
                    thumbnail_url: Some(
                        yoink_worker_protocol::HttpSourceUrl::parse(
                            "https://example.com/artist.jpg",
                        )
                        .unwrap(),
                    ),
                    platform_links: vec![(MusicProvider::Spotify, "Spotify".into(), url)],
                    top_tracks: if matches!(self.mode, WorkerMode::LargeArtists) {
                        (0..5)
                            .map(|_| yoink_worker_protocol::MusicArtistTrack {
                                title: "t".repeat(1024),
                                links: vec![],
                            })
                            .collect()
                    } else {
                        vec![]
                    },
                },
                _ => return Err(MusicError::InvalidInput),
            })
        })
    }
    fn audio(
        &self,
        request: MusicRequest,
        cancel: CancellationToken,
        _: Duration,
        _: u64,
    ) -> MusicFuture<'_, Vec<ValidatedArtifact>> {
        Box::pin(async move {
            self.executor
                .execute_music_audio(&request, cancel)
                .await
                .map_err(|_| MusicError::SourceUnavailable)
        })
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    runtime: MusicRuntime,
    started: Arc<std::sync::atomic::AtomicUsize>,
    dropped: Arc<std::sync::atomic::AtomicUsize>,
    pause: Arc<ObservationPause>,
}

#[tokio::test]
async fn central_inline_dispatch_preserves_downloader_authority_and_music_capacity() {
    let (listener, bot) = wire().await;
    let f = fixture(bot.clone(), false).await;
    query("DELETE FROM user_permissions WHERE user_id=42 AND plugin='music'")
        .execute(&f.runtime.persistence.database)
        .await
        .unwrap();
    query("INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(42,'dl','inline',42)")
        .execute(&f.runtime.persistence.database).await.unwrap();
    let (sent, mut received) = tokio::sync::mpsc::channel(3);
    let server = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (_, bytes) = request(&mut stream).await;
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            response(&mut stream, json!(true)).await;
            sent.send(body).await.unwrap();
        }
    });
    for (index, link) in [
        "https://youtu.be/video",
        "https://example.com/video",
        "https://open.spotify.com/track/123",
    ]
    .into_iter()
    .enumerate()
    {
        if index == 1 {
            query("INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(42,'music','inline',42)")
                .execute(&f.runtime.persistence.database).await.unwrap();
        }
        let searches = crate::inline_search::Searches::new(
            crate::InlineSearchConfig::default(),
            CancellationToken::new(),
        );
        let _occupied = if index > 0 {
            Some(
                searches
                    .slots
                    .clone()
                    .acquire_many_owned(u32::try_from(searches.slots.available_permits()).unwrap())
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        let dependencies = crate::MessageDependencies {
            music: Some((f.runtime.clone(), Duration::from_secs(120))),
            stats: None,
            lifecycle: crate::TelegramHandlerLifecycle::new(),
            inbox: None,
            insight: None,
            downloads: None,
            identities: Some(f.runtime.identities.clone()),
            upload: crate::CookieUploadDependency {
                config: None,
                base: bot.api_url(),
                client: bot.client().clone(),
            },
            cookie_url: crate::CookieUrlDependency(None),
        };
        let inline = serde_json::from_value(json!({"id":format!("central-{index}"),"from":{"id":42,"is_bot":false,"first_name":"Fixture"},"query":link,"offset":""})).unwrap();
        crate::inline::handle(bot.clone(), inline, dependencies, searches.clone())
            .await
            .unwrap();
        let answer = tokio::time::timeout(Duration::from_secs(3), received.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            answer["results"].as_array().unwrap().len(),
            usize::from(index < 2)
        );
        if index < 2 {
            assert_eq!(
                answer["results"][0]["input_message_content"]["message_text"],
                link
            );
        } else {
            assert_eq!(
                answer["button"]["text"],
                crate::translate(
                    yoink_i18n::Language::English,
                    yoink_i18n::Key::InlineBusy,
                    &[]
                )
            );
        }
        searches.shutdown().await;
    }
    assert_eq!(f.started.load(std::sync::atomic::Ordering::SeqCst), 0);
    server.await.unwrap();
    f.runtime.drain_handlers().await.unwrap();
}
async fn fixture(bot: Bot, download: bool) -> Fixture {
    fixture_with(bot, download, WorkerMode::Normal).await
}

#[tokio::test]
async fn central_music_authority_wait_obeys_supersession_shutdown_and_deadline() {
    let (_listener, bot) = wire().await;
    let f = fixture(bot.clone(), false).await;
    tokio::time::pause();
    for action in ["supersede", "shutdown", "expire"] {
        let held = f.runtime.persistence.database.connection().await.unwrap();
        let searches = crate::inline_search::Searches::new(
            crate::InlineSearchConfig {
                timeout: Duration::from_secs(1),
                ..Default::default()
            },
            CancellationToken::new(),
        );
        let dependencies = crate::MessageDependencies {
            music: Some((f.runtime.clone(), Duration::from_secs(120))),
            stats: None,
            lifecycle: crate::TelegramHandlerLifecycle::new(),
            inbox: None,
            insight: None,
            downloads: None,
            identities: Some(f.runtime.identities.clone()),
            upload: crate::CookieUploadDependency {
                config: None,
                base: bot.api_url(),
                client: bot.client().clone(),
            },
            cookie_url: crate::CookieUrlDependency(None),
        };
        let query = serde_json::from_value(json!({"id":action,"from":{"id":42,"is_bot":false,"first_name":"Fixture"},"query":"https://open.spotify.com/track/123","offset":""})).unwrap();
        crate::inline::handle(bot.clone(), query, dependencies, searches.clone())
            .await
            .unwrap();
        tokio::task::yield_now().await;
        assert_eq!(searches.tasks.len(), 1);
        searches.tasks.close();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), searches.tasks.wait())
                .await
                .is_err()
        );
        if action == "supersede" {
            searches.begin(42).unwrap();
        }
        if action == "expire" {
            tokio::time::advance(Duration::from_secs(1)).await;
        }
        if action != "shutdown" {
            tokio::time::timeout(Duration::from_millis(100), searches.tasks.wait())
                .await
                .unwrap();
        }
        tokio::time::timeout(Duration::from_millis(100), searches.shutdown())
            .await
            .unwrap();
        drop(held);
    }
    tokio::time::resume();
    assert_eq!(f.started.load(std::sync::atomic::Ordering::SeqCst), 0);
    f.runtime.drain_handlers().await.unwrap();
}
async fn fixture_with(bot: Bot, download: bool, mode: WorkerMode) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Arc::new(
        Store::open(&dir.path().join("test.db"), true, Duration::from_secs(60))
            .await
            .unwrap(),
    );
    store.migrate().await.unwrap();
    query("INSERT INTO users(id,role,created_at,updated_at) VALUES(42,'restricted',1,1)")
        .execute(store.database())
        .await
        .unwrap();
    query("INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(42,'music','inline',42)").execute(store.database()).await.unwrap();
    let ids = Arc::new(IdentityStore::new(store.database().clone(), None, true));
    let persistence = Arc::new(NativeMusicPersistence {
        database: store.database().clone(),
        store,
    });
    std::fs::create_dir(dir.path().join("work")).unwrap();
    let executor = Executor::new(ExecutorConfig {
        program: "/usr/bin/env".into(),
        args: vec![
            "node".into(),
            format!(
                "{}/tests/music_runtime_audio.mjs",
                env!("CARGO_MANIFEST_DIR")
            ),
            if matches!(mode, WorkerMode::ActiveAudio) {
                "hang".into()
            } else {
                "audio".into()
            },
            dir.path().join("worker").to_string_lossy().into_owned(),
        ],
        workspace_root: dir.path().join("work"),
        timeout: Duration::from_secs(2),
        processing_timeout: Duration::from_secs(2),
        socket_timeout: Duration::from_secs(2),
        max_artifact_bytes: 1024,
        max_workspace_bytes: 1024,
        max_output_bytes: 16384,
        max_artifacts: 1,
        allowed_proxy_origins: vec![],
        allowed_private_origins: vec![],
    })
    .unwrap();
    let settings = MusicSettings {
        request_timeout: Duration::from_secs(1),
        lookup_timeout: if matches!(mode, WorkerMode::InterleavedObservation) {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(1)
        },
        resolve_timeout: if matches!(mode, WorkerMode::InterleavedObservation) {
            Duration::from_secs(6)
        } else {
            Duration::from_secs(3)
        },
        audio_timeout: Duration::from_secs(3),
        delivery_timeout: Duration::from_secs(2),
        persistence_timeout: if matches!(mode, WorkerMode::PersistenceBudget) {
            Duration::from_millis(50)
        } else {
            Duration::from_secs(2)
        },
        cache_ttl: if matches!(mode, WorkerMode::MetadataVariation) {
            Duration::from_millis(10)
        } else {
            Duration::from_secs(60)
        },
        cache_entries: 8,
        worker_concurrency: 2,
        response_max_bytes: if matches!(mode, WorkerMode::LargeArtists) {
            16384
        } else {
            4096
        },
        max_requests: 10,
        audio_max_bytes: 1024,
        max_links_per_command: if matches!(mode, WorkerMode::LargeArtists) {
            10
        } else {
            3
        },
        download_enabled: download,
        credentials: MusicCredentials::default(),
        proxies: Default::default(),
    };
    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let pause = Arc::new(ObservationPause {
        reached: tokio::sync::Semaphore::new(0),
        resume: tokio::sync::Semaphore::new(0),
    });
    let service = MusicService::new(
        Arc::new(NativeMusicAuthority(ids.clone())),
        Arc::new(Worker {
            executor,
            mode,
            started: started.clone(),
            dropped: dropped.clone(),
            pause: pause.clone(),
        }),
        persistence.clone(),
        settings,
    )
    .unwrap();
    let delivery = Arc::new(
        NativeMusicDelivery::new(
            bot,
            123,
            MusicTransportBudget {
                timeout: Duration::from_secs(2),
                response_bytes: 65536,
            },
        )
        .unwrap(),
    );
    Fixture {
        _dir: dir,
        runtime: MusicRuntime::new(service, persistence, ids, delivery, true).unwrap(),
        started,
        dropped,
        pause,
    }
}
async fn wire() -> (TcpListener, Bot) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bot = Bot::with_client(
        "123:fixture",
        reqwest::Client::builder().no_proxy().build().unwrap(),
    )
    .set_api_url(
        format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    );
    (listener, bot)
}
async fn request(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut bytes = Vec::new();
    let split = loop {
        let mut buf = [0; 4096];
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buf[..n]);
        if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let header = String::from_utf8(bytes[..split].to_vec()).unwrap();
    let length: usize = header
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|s| s.trim().parse().unwrap())
        })
        .unwrap();
    while bytes.len() < split + length {
        let mut buf = [0; 4096];
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buf[..n]);
    }
    (header, bytes[split..split + length].to_vec())
}
async fn response(stream: &mut TcpStream, result: Value) {
    let body = json!({"ok":true,"result":result}).to_string();
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
}
fn multipart_field<'a>(body: &'a str, name: &str) -> &'a str {
    body.split(&format!("name=\"{name}\"\r\n\r\n"))
        .nth(1)
        .unwrap()
        .split("\r\n--")
        .next()
        .unwrap()
}
fn receipt(id: i32, chat: i64, topic: Option<i32>) -> Value {
    let mut v = json!({"message_id":id,"date":0,"chat":{"id":chat,"type":if chat>0 {"private"} else {"supergroup"},"title":"fixture"},"from":{"id":123,"is_bot":true,"first_name":"Bot"}});
    if let Some(t) = topic {
        v["message_thread_id"] = t.into();
    }
    v
}
fn message(id: i32, text: &str) -> Message {
    serde_json::from_value(json!({"message_id":id,"date":0,"chat":{"id":42,"type":"private","first_name":"Requester"},"from":{"id":42,"is_bot":false,"first_name":"Requester 🎵"},"text":text})).unwrap()
}
fn track() -> Track {
    let source = parse_link("https://music.youtube.com/watch?v=abcdefghijk").unwrap();
    Track {
        metadata: MusicMetadata {
            title: "Song 🎵".into(),
            artist: Some("Artist <&>".into()),
            thumbnail_url: None,
        },
        links: vec![source.clone()],
        source,
    }
}

#[tokio::test]
async fn dispatcher_music_song_and_deep_help_do_not_require_downloader() {
    let (listener, bot) = wire().await;
    let f = fixture(bot.clone(), false).await;
    let lifecycle = crate::TelegramHandlerLifecycle::new();
    let deps = crate::MessageDependencies {
        downloads: None,
        upload: crate::CookieUploadDependency {
            config: None,
            base: bot.api_url(),
            client: bot.client().clone(),
        },
        cookie_url: crate::CookieUrlDependency(None),
        identities: Some(f.runtime.identities.clone()),
        inbox: None,
        insight: None,
        music: Some((f.runtime.clone(), Duration::from_secs(30))),
        stats: None,
        lifecycle: lifecycle.clone(),
    };
    let identity = crate::TelegramIdentity {
        owner_id: 99,
        bot_id: 123,
        groups_enabled: true,
        identity_enabled: true,
    };
    let system = yoink_application::SystemService::new("fixture");
    system.set_ready(true);
    let server = tokio::spawn(async move {
        for id in [21, 22, 23] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (_, body) = request(&mut stream).await;
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["chat_id"], 42);
            if id < 23 {
                assert_eq!(body["reply_parameters"]["message_id"], id);
            } else {
                assert!(body["text"].as_str().unwrap().contains("/song"));
            }
            let mut echo = receipt(100 + id, 42, None);
            echo["text"] = body["text"].clone();
            response(&mut stream, echo).await;
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    for (id, text) in [
        (21, "/music@fixture https://open.spotify.com/track/abc"),
        (22, "/song https://open.spotify.com/track/abc"),
        (23, "/start music_help"),
    ] {
        crate::handle_message(
            bot.clone(),
            message(id, text),
            identity,
            "fixture".into(),
            system.clone(),
            deps.clone(),
        )
        .await
        .unwrap();
    }
    lifecycle.drain_handlers().await;
    server.await.unwrap();
    f.runtime.drain_handlers().await.unwrap();
}
fn audio_command(t: &Track, id: i32) -> AudioCommand<'_> {
    let mut caption = MusicCard::from_resolution(&Resolution::Track(t.clone()));
    caption.add_requester(42, "Requester 🎵");
    AudioCommand {
        actor: MusicActor {
            user_id: 42,
            group: None,
        },
        target: AudioTarget {
            bot_id: 123,
            chat_id: 42,
            topic_id: None,
            incoming_message_id: id,
        },
        track: t,
        caption,
    }
}
#[tokio::test]
async fn card_reply_hidden_links_and_inline_flags_entities() {
    let (listener, bot) = wire().await;
    let f = fixture(bot, false).await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (head, body) = request(&mut stream).await;
        assert!(head.contains("sendMessage"));
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["reply_parameters"]["message_id"], 11);
        assert_eq!(v["text"], "Artist <&> - Song 🎵\nSpotify\nRequester 🎵");
        assert_eq!(v["entities"][0]["offset"], 21);
        assert_eq!(v["entities"][0]["length"], 7);
        assert_eq!(
            v["link_preview_options"],
            json!({"url":"https://example.com/cover.jpg","show_above_text":true,"prefer_large_media":true})
        );
        let mut r = receipt(77, 42, None);
        r["text"] = v["text"].clone();
        response(&mut stream, r).await;
        let (mut stream, _) = listener.accept().await.unwrap();
        let (head, body) = request(&mut stream).await;
        assert!(head.contains("answerInlineQuery"));
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["cache_time"], 0);
        assert_eq!(v["is_personal"], true);
        assert_eq!(v["button"]["start_parameter"], "music_help");
        assert_eq!(
            v["results"][0]["input_message_content"]["link_preview_options"]["url"],
            "https://example.com/cover.jpg"
        );
        assert_eq!(
            v["results"][0]["reply_markup"]["inline_keyboard"][0][0]["text"],
            "Share"
        );
        assert_eq!(
            v["results"][0]["input_message_content"]["message_text"],
            "Artist <&> - Song 🎵\nSpotify"
        );
        assert_eq!(
            v["results"][0]["input_message_content"]["entities"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            v["results"][0]["input_message_content"]["entities"][0]["offset"],
            21
        );
        response(&mut stream, json!(true)).await;
    });
    let mut m = serde_json::to_value(message(11, "/song@fixture")).unwrap();
    m["reply_to_message"] = json!({"message_id":10,"date":0,"chat":{"id":42,"type":"private"},"text":"hidden","entities":[{"type":"text_link","offset":0,"length":6,"url":"https://open.spotify.com/track/abc"}]});
    assert!(
        f.runtime
            .handle_command(&serde_json::from_value(m).unwrap(), 42, "fixture")
            .await
            .unwrap()
    );
    let q:InlineQuery=serde_json::from_value(json!({"id":"query","from":{"id":42,"is_bot":false,"first_name":"Requester"},"query":"listen https://open.spotify.com/track/abc now","offset":""})).unwrap();
    assert_eq!(
        f.runtime.handle_inline(&q).await.unwrap(),
        MusicInlineOutcome::Answered
    );
    let mut q = q;
    q.query = "not music".into();
    assert_eq!(
        f.runtime.handle_inline(&q).await.unwrap(),
        MusicInlineOutcome::Unsupported
    );
    server.await.unwrap();
}
#[tokio::test]
async fn audio_upload_open_descriptor_cached_and_authoritative_replay() {
    let (listener, bot) = wire().await;
    let f = fixture(bot, true).await;
    let server = tokio::spawn(async move {
        for upload in [true, false] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (head, body) = request(&mut stream).await;
            assert!(head.contains("sendAudio"));
            let body = String::from_utf8(body).unwrap();
            assert!(body.contains("Song 🎵"));
            assert!(body.contains("Artist <&>"));
            assert!(body.contains("tg://user?id=42"));
            assert!(body.contains("caption_entities"));
            assert!(body.contains("Requester 🎵"));
            assert_eq!(multipart_field(&body, "title"), "Song 🎵");
            assert_eq!(multipart_field(&body, "performer"), "Artist <&>");
            let expected = audio_command(&track(), 11).caption;
            assert_eq!(multipart_field(&body, "caption"), expected.text);
            assert_eq!(
                serde_json::from_str::<Value>(multipart_field(&body, "caption_entities")).unwrap(),
                json!(entities(&expected))
            );
            assert_eq!(
                serde_json::from_str::<Value>(multipart_field(&body, "reply_parameters")).unwrap(),
                json!({"message_id":if upload {11}else{12}})
            );
            assert_eq!(body.contains("ID3fixtureaudio"), upload);
            if !upload {
                assert!(body.contains("native-file"));
            }
            let mut r = receipt(if upload { 77 } else { 78 }, 42, None);
            r["audio"] = json!({"file_id":"native-file","file_unique_id":"unique","duration":1,"file_size":15,"mime_type":"audio/mpeg"});
            let decoded: Message = serde_json::from_value(r.clone()).unwrap();
            assert!(decoded.audio().is_some());
            response(&mut stream, r).await;
        }
    });
    let t = track();
    assert_eq!(
        f.runtime.send_audio(audio_command(&t, 11)).await.unwrap(),
        AudioOutcome::Confirmed(77)
    );
    assert_eq!(
        f.runtime.send_audio(audio_command(&t, 11)).await.unwrap(),
        AudioOutcome::Confirmed(77)
    );
    assert_eq!(
        f.runtime.send_audio(audio_command(&t, 12)).await.unwrap(),
        AudioOutcome::Confirmed(78)
    );
    server.await.unwrap();
    assert_eq!(
        std::fs::read_dir(f._dir.path().join("work"))
            .unwrap()
            .count(),
        0
    );
}
#[tokio::test]
async fn lost_audio_receipt_is_unknown_and_never_retried() {
    let (listener, bot) = wire().await;
    let f = fixture(bot, true).await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        drop(stream);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), listener.accept())
                .await
                .is_err()
        );
    });
    let t = track();
    assert_eq!(
        f.runtime.send_audio(audio_command(&t, 11)).await.unwrap(),
        AudioOutcome::Unknown
    );
    assert_eq!(
        f.runtime.send_audio(audio_command(&t, 11)).await.unwrap(),
        AudioOutcome::Unknown
    );
    server.await.unwrap();
}
#[tokio::test]
async fn topic_reply_and_sender_proof_rejection() {
    let (listener, bot) = wire().await;
    let delivery = NativeMusicDelivery::new(
        bot,
        123,
        MusicTransportBudget {
            timeout: Duration::from_secs(2),
            response_bytes: 65536,
        },
    )
    .unwrap();
    let target = AudioTarget {
        bot_id: 123,
        chat_id: -100,
        topic_id: Some(8),
        incoming_message_id: 11,
    };
    let card = MusicCard::from_resolution(&Resolution::Track(track()));
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut s).await;
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["message_thread_id"], 8);
        assert_eq!(v["reply_parameters"]["message_id"], 11);
        let mut r = receipt(77, -100, Some(8));
        r["from"]["id"] = 999.into();
        r["text"] = v["text"].clone();
        response(&mut s, r).await;
    });
    assert_eq!(
        delivery.card(target, &card).await,
        Err(MusicError::DeliveryUnknown)
    );
    server.await.unwrap();
}
#[tokio::test]
async fn fresh_denial_cancel_dropped_handler_and_drain() {
    let (listener, bot) = wire().await;
    let f = fixture(bot, false).await;
    query("DELETE FROM user_permissions WHERE user_id=42")
        .execute(&f.runtime.persistence.database)
        .await
        .unwrap();
    assert_eq!(
        f.runtime
            .handle_command(
                &message(11, "/music https://open.spotify.com/track/abc"),
                42,
                "fixture"
            )
            .await,
        Err(MusicError::Denied)
    );
    query("INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(42,'music','inline',42)").execute(&f.runtime.persistence.database).await.unwrap();
    let runtime = f.runtime.clone();
    let task = tokio::spawn(async move {
        runtime
            .handle_command(
                &message(12, "/music https://open.spotify.com/track/abc"),
                42,
                "fixture",
            )
            .await
    });
    let (mut s, _) = listener.accept().await.unwrap();
    request(&mut s).await;
    f.runtime.cancel_all();
    assert_eq!(task.await.unwrap(), Err(MusicError::DeliveryUnknown));
    f.runtime.drain_handlers().await.unwrap();
    assert_eq!(
        f.runtime
            .handle_command(&message(13, "/music x"), 42, "fixture")
            .await,
        Err(MusicError::Cancelled)
    );
}
#[tokio::test]
async fn dropped_audio_and_inline_handlers_release_drain() {
    for audio in [true, false] {
        let (listener, bot) = wire().await;
        let f = fixture(bot, true).await;
        let runtime = f.runtime.clone();
        let task = tokio::spawn(async move {
            if audio {
                let t = track();
                let _ = runtime.send_audio(audio_command(&t, 11)).await;
            } else {
                let q:InlineQuery=serde_json::from_value(json!({"id":"query","from":{"id":42,"is_bot":false,"first_name":"Requester"},"query":"https://open.spotify.com/track/abc","offset":""})).unwrap();
                let _ = runtime.handle_inline(&q).await;
            }
        });
        let (mut s, _) = listener.accept().await.unwrap();
        request(&mut s).await;
        task.abort();
        let _ = task.await;
        tokio::time::timeout(Duration::from_secs(2), f.runtime.drain_handlers())
            .await
            .unwrap()
            .unwrap();
        assert!(f.runtime.handlers.admit().is_none());
    }
}
#[tokio::test]
async fn revocation_after_card_prevents_audio() {
    let (listener, bot) = wire().await;
    let f = fixture(bot, true).await;
    let database = f.runtime.persistence.database.clone();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut s).await;
        let v: Value = serde_json::from_slice(&body).unwrap();
        query("DELETE FROM user_permissions WHERE user_id=42")
            .execute(&database)
            .await
            .unwrap();
        let mut r = receipt(77, 42, None);
        r["text"] = v["text"].clone();
        response(&mut s, r).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(200), listener.accept())
                .await
                .is_err()
        );
    });
    assert_eq!(
        f.runtime
            .handle_command(
                &message(11, "/music https://music.youtube.com/watch?v=abcdefghijk"),
                42,
                "fixture"
            )
            .await,
        Err(MusicError::Denied)
    );
    server.await.unwrap();
}
#[tokio::test]
async fn bounded_transport_only_explicit_nonacceptance_is_rejected() {
    for (body, expected) in [
        (
            json!({"ok":false,"error_code":400,"description":"bad request"}).to_string(),
            MusicError::DeliveryRejected,
        ),
        (
            json!({"ok":false,"error_code":500,"description":"uncertain"}).to_string(),
            MusicError::DeliveryUnknown,
        ),
        ("malformed".into(), MusicError::DeliveryUnknown),
        ("x".repeat(2048), MusicError::DeliveryUnknown),
    ] {
        let (listener, bot) = wire().await;
        let delivery = NativeMusicDelivery::new(
            bot,
            123,
            MusicTransportBudget {
                timeout: Duration::from_secs(2),
                response_bytes: 1024,
            },
        )
        .unwrap();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            request(&mut s).await;
            s.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        });
        assert_eq!(
            delivery
                .receive(delivery.request("sendAudio").json(&json!({})))
                .await,
            Err(expected)
        );
        server.await.unwrap();
    }
}
#[tokio::test]
async fn artist_cards_use_supplied_entities_and_never_send_audio() {
    let (listener, bot) = wire().await;
    let f = fixture(bot, true).await;
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut s).await;
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["text"], "Artist 🎵 <&>\nrock\nSpotify\nRequester 🎵");
        assert_eq!(
            v["link_preview_options"]["url"],
            "https://example.com/artist.jpg"
        );
        assert_eq!(
            v["entities"][0],
            json!({"type":"text_link","offset":19,"length":7,"url":"https://open.spotify.com/artist/abc"})
        );
        let mut r = receipt(77, 42, None);
        r["text"] = v["text"].clone();
        response(&mut s, r).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(200), listener.accept())
                .await
                .is_err()
        );
    });
    assert!(
        f.runtime
            .handle_command(
                &message(11, "/music https://open.spotify.com/artist/abc"),
                42,
                "fixture"
            )
            .await
            .unwrap()
    );
    server.await.unwrap();
}
#[tokio::test]
async fn stalled_body_deadline_and_audio_cancellation_are_unknown() {
    let (listener, bot) = wire().await;
    let delivery = NativeMusicDelivery::new(
        bot,
        123,
        MusicTransportBudget {
            timeout: Duration::from_millis(100),
            response_bytes: 1024,
        },
    )
    .unwrap();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        request(&mut s).await;
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"ok\":true,")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
    });
    assert_eq!(
        delivery
            .receive(delivery.request("sendAudio").json(&json!({})))
            .await,
        Err(MusicError::DeliveryUnknown)
    );
    server.await.unwrap();
    let (listener, bot) = wire().await;
    let f = fixture(bot, true).await;
    let runtime = f.runtime.clone();
    let task = tokio::spawn(async move {
        let t = track();
        runtime.send_audio(audio_command(&t, 11)).await
    });
    let (mut s, _) = listener.accept().await.unwrap();
    request(&mut s).await;
    f.runtime.cancel_all();
    assert_eq!(task.await.unwrap().unwrap(), AudioOutcome::Unknown);
    f.runtime.drain_handlers().await.unwrap();
}

#[tokio::test]
async fn long_command_metadata_uses_requester_only_audio_caption() {
    let (listener, bot) = wire().await;
    let f = fixture_with(bot, true, WorkerMode::LongMetadata).await;
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut s).await;
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert!(value["text"].as_str().unwrap().encode_utf16().count() > 1024);
        let mut r = receipt(77, 42, None);
        r["text"] = value["text"].clone();
        response(&mut s, r).await;
        let (mut s, _) = listener.accept().await.unwrap();
        let (head, body) = request(&mut s).await;
        assert!(head.contains("sendAudio"));
        let body = String::from_utf8(body).unwrap();
        assert_eq!(multipart_field(&body, "title"), "x".repeat(1000));
        assert_eq!(multipart_field(&body, "performer"), "a".repeat(100));
        let caption = MusicCard::audio_caption(42, "Requester 🎵");
        assert_eq!(caption.text, "Requester 🎵");
        assert_eq!(caption.entities[0].offset, 0);
        assert_eq!(caption.entities[0].url, "tg://user?id=42");
        assert_eq!(multipart_field(&body, "caption"), caption.text);
        assert!(caption.text.encode_utf16().count() <= 1024);
        assert_eq!(
            serde_json::from_str::<Value>(multipart_field(&body, "caption_entities")).unwrap(),
            json!(entities(&caption))
        );
        let mut r = receipt(78, 42, None);
        r["audio"] = json!({"file_id":"native-file","file_unique_id":"unique","duration":1,"mime_type":"audio/mpeg"});
        response(&mut s, r).await;
    });
    assert!(
        f.runtime
            .handle_command(
                &message(11, "/music https://open.spotify.com/track/abc"),
                42,
                "fixture"
            )
            .await
            .unwrap()
    );
    server.await.unwrap();
}

#[tokio::test]
async fn nonmusic_youtube_inline_keeps_downloader_fallback_unanswered() {
    let (listener, bot) = wire().await;
    let f = fixture_with(bot, false, WorkerMode::NonMusic).await;
    let q:InlineQuery=serde_json::from_value(json!({"id":"query","from":{"id":42,"is_bot":false,"first_name":"Requester"},"query":"watch https://www.youtube.com/watch?v=abcdefghijk","offset":""})).unwrap();
    assert_eq!(
        f.runtime.handle_inline(&q).await.unwrap(),
        MusicInlineOutcome::Unsupported
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn passive_spotify_failure_is_reported_by_the_shared_dispatcher() {
    let (listener, bot) = wire().await;
    let f = fixture_with(bot.clone(), true, WorkerMode::NonMusic).await;
    let deps = crate::MessageDependencies {
        downloads: None,
        upload: crate::CookieUploadDependency {
            config: None,
            base: bot.api_url(),
            client: bot.client().clone(),
        },
        cookie_url: crate::CookieUrlDependency(None),
        identities: Some(f.runtime.identities.clone()),
        inbox: None,
        insight: None,
        music: Some((f.runtime.clone(), Duration::from_secs(30))),
        stats: None,
        lifecycle: crate::TelegramHandlerLifecycle::new(),
    };
    let system = yoink_application::SystemService::new("fixture");
    system.set_ready(true);
    let mut incoming = message(20, "https://open.spotify.com/track/abc");
    incoming.date = chrono::Utc::now();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut socket).await;
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["chat_id"], 42);
        let text = body["text"].as_str().unwrap();
        assert!(
            ["en", "ru"]
                .iter()
                .any(|lang| text == error_message(MusicError::SourceUnavailable, lang))
        );
        let mut echo = receipt(90, 42, None);
        echo["text"] = body["text"].clone();
        response(&mut socket, echo).await;
    });
    crate::handle_message(
        bot,
        incoming,
        crate::TelegramIdentity {
            owner_id: 99,
            bot_id: 123,
            groups_enabled: true,
            identity_enabled: true,
        },
        "fixture".into(),
        system,
        deps.clone(),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
    deps.lifecycle.drain_handlers().await;
    f.runtime.drain_handlers().await.unwrap();
}

#[tokio::test]
async fn passive_links_and_via_bot_cards_keep_provenance_and_replay_fences() {
    for (via, foreign) in [(false, false), (false, true), (true, false)] {
        let (listener, bot) = wire().await;
        let f = fixture_with(bot, true, WorkerMode::Playback).await;
        let mut raw =
            serde_json::to_value(message(11, "https://open.spotify.com/track/abc")).unwrap();
        if foreign {
            raw["via_bot"] = json!({"id":999,"is_bot":true,"first_name":"Other Bot"});
        }
        if via {
            raw["text"] = "Spotify".into();
            raw["entities"] = json!([{"type":"text_link","offset":0,"length":7,"url":"https://open.spotify.com/track/abc"}]);
            raw["via_bot"] = json!({"id":123,"is_bot":true,"first_name":"Bot"});
        }
        let m: Message = serde_json::from_value(raw).unwrap();
        let now = chrono::DateTime::from_timestamp(1, 0).unwrap();
        assert!(
            !f.runtime
                .handle_observed_message(
                    &m,
                    42,
                    chrono::DateTime::from_timestamp(31, 0).unwrap(),
                    Duration::from_secs(30)
                )
                .await
                .unwrap()
        );
        assert_eq!(
            f.runtime
                .handle_observed_message(&m, 99, now, Duration::from_secs(30))
                .await,
            Err(MusicError::Denied)
        );
        let server = tokio::spawn(async move {
            if !via {
                let (mut s, _) = listener.accept().await.unwrap();
                let (head, body) = request(&mut s).await;
                assert!(head.contains("sendMessage"));
                let value: Value = serde_json::from_slice(&body).unwrap();
                let mut r = receipt(77, 42, None);
                r["text"] = value["text"].clone();
                response(&mut s, r).await;
            }
            let (mut s, _) = listener.accept().await.unwrap();
            let (head, body) = request(&mut s).await;
            assert!(head.contains("sendAudio"));
            let body = String::from_utf8(body).unwrap();
            assert_eq!(
                multipart_field(&body, "caption"),
                MusicCard::audio_caption(42, "Requester 🎵").text
            );
            let mut r = receipt(78, 42, None);
            r["audio"] = json!({"file_id":"native-file","file_unique_id":"unique","duration":1,"mime_type":"audio/mpeg"});
            response(&mut s, r).await;
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        for _ in 0..2 {
            assert!(
                f.runtime
                    .handle_observed_message(&m, 42, now, Duration::from_secs(30))
                    .await
                    .unwrap()
            );
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn changed_card_requester_returns_typed_conflict_without_resend() {
    let (listener, bot) = wire().await;
    let f = fixture(bot, false).await;
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut s).await;
        let value: Value = serde_json::from_slice(&body).unwrap();
        let mut r = receipt(77, 42, None);
        r["text"] = value["text"].clone();
        response(&mut s, r).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let m = message(11, "/music https://open.spotify.com/track/abc");
    f.runtime.handle_command(&m, 42, "fixture").await.unwrap();
    let mut raw = serde_json::to_value(m).unwrap();
    raw["from"]["first_name"] = "Changed requester".into();
    assert_eq!(
        f.runtime
            .handle_command(&serde_json::from_value(raw).unwrap(), 42, "fixture")
            .await,
        Err(MusicError::Conflict)
    );
    server.await.unwrap();
}

#[tokio::test]
async fn unsuccessful_http_status_cannot_confirm_card_audio_or_inline() {
    for status in [400, 503] {
        for kind in ["card", "audio", "inline"] {
            let (listener, bot) = wire().await;
            let f = fixture(bot, true).await;
            let server = tokio::spawn(async move {
                let (mut s, _) = listener.accept().await.unwrap();
                request(&mut s).await;
                let result = if kind == "inline" {
                    json!(true)
                } else {
                    let mut r = receipt(77, 42, None);
                    if kind == "audio" {
                        r["audio"] =
                            json!({"file_id":"native-file","file_unique_id":"unique","duration":1});
                    } else {
                        r["text"] = "Artist <&> - Song 🎵\nYouTube Music".into();
                    }
                    r
                };
                let body = json!({"ok":true,"result":result}).to_string();
                s.write_all(format!("HTTP/1.1 {status} Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            });
            if kind == "inline" {
                let q:InlineQuery=serde_json::from_value(json!({"id":"query","from":{"id":42,"is_bot":false,"first_name":"Requester"},"query":"https://open.spotify.com/track/abc","offset":""})).unwrap();
                assert_eq!(
                    f.runtime.handle_inline(&q).await,
                    Err(MusicError::DeliveryUnknown)
                );
            } else if kind == "audio" {
                assert_eq!(
                    f.runtime
                        .send_audio(audio_command(&track(), 11))
                        .await
                        .unwrap(),
                    AudioOutcome::Unknown
                );
                assert_eq!(
                    f.runtime
                        .send_audio(audio_command(&track(), 11))
                        .await
                        .unwrap(),
                    AudioOutcome::Unknown
                );
            } else {
                let target = audio_command(&track(), 11).target;
                assert_eq!(
                    f.runtime
                        .delivery
                        .card(
                            target,
                            &MusicCard::from_resolution(&Resolution::Track(track()))
                        )
                        .await,
                    Err(MusicError::DeliveryUnknown)
                );
            }
            server.await.unwrap();
        }
    }
}

#[tokio::test]
async fn provider_tasks_are_dropped_before_runtime_drain_returns() {
    for abort in [false, true] {
        for inline in [false, true] {
            let (_, bot) = wire().await;
            let f = fixture_with(bot, false, WorkerMode::PendingSearch).await;
            let runtime = f.runtime.clone();
            let task = tokio::spawn(async move {
                if inline {
                    let q:InlineQuery=serde_json::from_value(json!({"id":"query","from":{"id":42,"is_bot":false,"first_name":"Requester"},"query":"https://open.spotify.com/track/abc","offset":""})).unwrap();
                    let _ = runtime.handle_inline(&q).await;
                } else {
                    let _ = runtime
                        .handle_command(
                            &message(11, "/music https://open.spotify.com/track/abc"),
                            42,
                            "fixture",
                        )
                        .await;
                }
            });
            tokio::time::timeout(Duration::from_secs(2), async {
                while f.started.load(std::sync::atomic::Ordering::SeqCst) < 2 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            if abort {
                task.abort();
                let _ = task.await;
            } else {
                f.runtime.cancel_all();
                task.await.unwrap();
            }
            tokio::time::timeout(Duration::from_secs(2), f.runtime.drain_handlers())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                f.started.load(std::sync::atomic::Ordering::SeqCst),
                f.dropped.load(std::sync::atomic::Ordering::SeqCst)
            );
        }
    }
}

#[tokio::test]
async fn active_music_worker_drop_and_cancel_reap_before_runtime_drain() {
    for abort in [false, true] {
        let (_, bot) = wire().await;
        let f = fixture_with(bot, true, WorkerMode::ActiveAudio).await;
        let runtime = f.runtime.clone();
        let task =
            tokio::spawn(async move { runtime.send_audio(audio_command(&track(), 11)).await });
        let marker = f._dir.path().join("worker");
        tokio::time::timeout(Duration::from_secs(2), async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let parent = std::fs::read_to_string(marker.with_extension("parent")).unwrap();
        let child = std::fs::read_to_string(&marker).unwrap();
        if abort {
            task.abort();
            let _ = task.await;
        } else {
            f.runtime.cancel_all();
            let _ = task.await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(2), f.runtime.drain_handlers())
            .await
            .unwrap()
            .unwrap();
        assert!(
            !std::path::Path::new(&format!("/proc/{parent}")).exists(),
            "parent not reaped"
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match std::fs::read_to_string(format!("/proc/{child}/stat")) {
                    Ok(stat) if stat.split_whitespace().nth(2) != Some("Z") => {
                        tokio::time::sleep(Duration::from_millis(5)).await
                    }
                    Ok(_) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                    Err(e) => panic!("descendant inspection failed: {e}"),
                }
            }
        })
        .await
        .unwrap();
        assert!(
            f._dir
                .path()
                .join("work")
                .read_dir()
                .unwrap()
                .next()
                .is_none()
        );
    }
}

#[tokio::test]
async fn runtime_database_admission_cancel_is_bounded_by_service_policy() {
    for inline in [false, true] {
        let (_, bot) = wire().await;
        let f = fixture_with(bot, false, WorkerMode::PersistenceBudget).await;
        let db = f.runtime.persistence.database.clone();
        let held = db.connection().await.unwrap();
        let runtime = f.runtime.clone();
        let task = tokio::spawn(async move {
            if inline {
                let q:InlineQuery=serde_json::from_value(json!({"id":"query","from":{"id":42,"is_bot":false,"first_name":"Requester"},"query":"https://open.spotify.com/track/abc","offset":""})).unwrap();
                runtime.handle_inline(&q).await.map(|_| ())
            } else {
                runtime
                    .handle_command(
                        &message(11, "/music https://open.spotify.com/track/abc"),
                        42,
                        "fixture",
                    )
                    .await
                    .map(|_| ())
            }
        });
        tokio::task::yield_now().await;
        f.runtime.cancel_all();
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(200), task)
                .await
                .unwrap()
                .unwrap(),
            Err(MusicError::Cancelled)
        );
        tokio::time::timeout(Duration::from_millis(200), f.runtime.drain_handlers())
            .await
            .unwrap()
            .unwrap();
        drop(held);
    }
}

#[tokio::test]
async fn runtime_identity_language_and_card_journal_use_persistence_timeout() {
    let (_, bot) = wire().await;
    let f = fixture_with(bot, false, WorkerMode::PersistenceBudget).await;
    let db = f.runtime.persistence.database.clone();
    let held = db.connection().await.unwrap();
    let actor = MusicActor {
        user_id: 42,
        group: None,
    };
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(250), f.runtime.require(actor))
            .await
            .unwrap(),
        Err(MusicError::Persistence)
    );
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_millis(250),
            f.runtime.admission(async {
                f.runtime
                    .identities
                    .language_preference(42)
                    .await
                    .map_err(|_| MusicError::Persistence)
            })
        )
        .await
        .unwrap(),
        Err(MusicError::Persistence)
    ));
    let identity = TextIdentity {
        bot: 123,
        actor: 42,
        chat: 42,
        topic: None,
        message: 11,
        item: None,
        edit: false,
        text: "card".into(),
        payload: json!({"kind":"music_card","bind_item":false}),
    };
    assert_eq!(
        tokio::time::timeout(
            Duration::from_millis(250),
            f.runtime.admission(async {
                f.runtime
                    .persistence
                    .store
                    .begin_text("music:blocked", &identity)
                    .await
                    .map_err(store_error)
            })
        )
        .await
        .unwrap(),
        Err(MusicError::Persistence)
    );
    f.runtime.cancel_all();
    tokio::time::timeout(Duration::from_millis(250), f.runtime.drain_handlers())
        .await
        .unwrap()
        .unwrap();
    drop(held);
}

#[tokio::test]
async fn supplied_artwork_previews_cover_artist_inline_and_no_artwork_cards() {
    let (listener, bot) = wire().await;
    let f = fixture(bot, false).await;
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut s).await;
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["results"][0]["input_message_content"]["link_preview_options"],
            json!({"url":"https://example.com/artist.jpg","show_above_text":true,"prefer_large_media":true})
        );
        response(&mut s, json!(true)).await;
        let (mut s, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut s).await;
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["link_preview_options"], json!({"is_disabled":true}));
        let mut r = receipt(77, 42, None);
        r["text"] = value["text"].clone();
        response(&mut s, r).await;
    });
    let q:InlineQuery=serde_json::from_value(json!({"id":"query","from":{"id":42,"is_bot":false,"first_name":"Requester"},"query":"https://open.spotify.com/artist/abc","offset":""})).unwrap();
    assert_eq!(
        f.runtime.handle_inline(&q).await.unwrap(),
        MusicInlineOutcome::Answered
    );
    f.runtime
        .delivery
        .card(
            audio_command(&track(), 11).target,
            &MusicCard::from_resolution(&Resolution::Track(track())),
        )
        .await
        .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn positive_audio_receipt_survives_bounded_recording_failure_without_retry() {
    let (listener, bot) = wire().await;
    let f = fixture_with(bot, true, WorkerMode::PersistenceBudget).await;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (receipt_tx, receipt_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        request(&mut s).await;
        ready_tx.send(()).unwrap();
        receipt_rx.await.unwrap();
        let mut r = receipt(77, 42, None);
        r["audio"] = json!({"file_id":"native-file","file_unique_id":"unique","duration":1,"mime_type":"audio/mpeg"});
        response(&mut s, r).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(300), listener.accept())
                .await
                .is_err()
        );
    });
    let runtime = f.runtime.clone();
    let task = tokio::spawn(async move { runtime.send_audio(audio_command(&track(), 11)).await });
    ready_rx.await.unwrap();
    let db = f.runtime.persistence.database.clone();
    let held = db.connection().await.unwrap();
    receipt_tx.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        AudioOutcome::Confirmed(77)
    );
    drop(held);
    assert_eq!(
        f.runtime
            .send_audio(audio_command(&track(), 11))
            .await
            .unwrap(),
        AudioOutcome::Unknown
    );
    server.await.unwrap();
}

fn own_observed_card(source: &str) -> Message {
    let mut raw = serde_json::to_value(message(11, "Spotify")).unwrap();
    raw["entities"] = json!([{"type":"text_link","offset":0,"length":7,"url":source}]);
    raw["via_bot"] = json!({"id":123,"is_bot":true,"first_name":"Bot"});
    serde_json::from_value(raw).unwrap()
}
async fn observe(runtime: &MusicRuntime, message: &Message) -> Result<bool, MusicError> {
    runtime
        .handle_observed_message(
            message,
            42,
            chrono::DateTime::from_timestamp(1, 0).unwrap(),
            Duration::from_secs(30),
        )
        .await
}
async fn observed_audio_response(listener: TcpListener, positive: bool) {
    let (mut s, _) = listener.accept().await.unwrap();
    let (head, _) = request(&mut s).await;
    assert!(
        head.contains("sendAudio"),
        "own card must not produce a second card"
    );
    if positive {
        let mut r = receipt(77, 42, None);
        r["audio"] = json!({"file_id":"native-file","file_unique_id":"unique","duration":1,"mime_type":"audio/mpeg"});
        response(&mut s, r).await;
    } else {
        drop(s);
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(300), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn own_observation_changed_source_provenance_and_identity_are_conflicts() {
    let (listener, bot) = wire().await;
    let f = fixture_with(bot, true, WorkerMode::Playback).await;
    let server = tokio::spawn(observed_audio_response(listener, true));
    let original = own_observed_card("https://open.spotify.com/track/abc");
    assert!(observe(&f.runtime, &original).await.unwrap());
    let changed_source = own_observed_card("https://open.spotify.com/track/def");
    assert_eq!(
        observe(&f.runtime, &changed_source).await,
        Err(MusicError::Conflict)
    );
    for (field, value) in [
        (
            "via_bot",
            json!({"id":999,"is_bot":true,"first_name":"Other Bot"}),
        ),
        (
            "from",
            json!({"id":42,"is_bot":false,"first_name":"Changed requester"}),
        ),
        ("text", json!("Changed card")),
        ("entities", json!([])),
    ] {
        let mut raw = serde_json::to_value(&original).unwrap();
        raw[field] = value;
        assert_eq!(
            observe(&f.runtime, &serde_json::from_value(raw).unwrap()).await,
            Err(MusicError::Conflict)
        );
    }
    let identity = observation_identity(&original, 42, 123);
    let effect = f
        .runtime
        .persistence
        .store
        .text_effect(&observation_token(&identity))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(effect.outcome, TextOutcome::Prepared);
    assert!(!effect.bound);
    assert_eq!(
        effect.identity.as_ref().unwrap().payload["kind"],
        "music_observed_card"
    );
    let card = f
        .runtime
        .persistence
        .store
        .text_effect("music:card:123:42:None:11:0")
        .await
        .unwrap();
    assert!(
        card.is_none(),
        "observation must not masquerade as a card send"
    );
    server.await.unwrap();
}

#[tokio::test]
async fn concurrent_conflicting_own_observations_admit_only_one_audio_snapshot() {
    let (listener, bot) = wire().await;
    let f = fixture_with(bot, true, WorkerMode::Playback).await;
    let server = tokio::spawn(observed_audio_response(listener, true));
    let a = own_observed_card("https://open.spotify.com/track/abc");
    let b = own_observed_card("https://open.spotify.com/track/def");
    let (a_result, b_result) = tokio::join!(observe(&f.runtime, &a), observe(&f.runtime, &b));
    assert!(matches!(
        (a_result, b_result),
        (Ok(true), Err(MusicError::Conflict)) | (Err(MusicError::Conflict), Ok(true))
    ));
    let stored = f
        .runtime
        .persistence
        .store
        .text_effect(&observation_token(&observation_identity(&a, 42, 123)))
        .await
        .unwrap()
        .unwrap();
    let winner = if stored.identity.as_ref().unwrap().payload["observation"]["entities"][0]["url"]
        == "https://open.spotify.com/track/abc"
    {
        a
    } else {
        b
    };
    assert!(observe(&f.runtime, &winner).await.unwrap());
    server.await.unwrap();
}

#[tokio::test]
async fn concurrent_same_observation_preparations_reuse_first_resolution_snapshot() {
    let (listener, bot) = wire().await;
    let f = fixture_with(bot, true, WorkerMode::Playback).await;
    let m = own_observed_card("https://open.spotify.com/track/abc");
    let identity = observation_identity(&m, 42, 123);
    let first = Resolution::Track(track());
    let mut different = track();
    different.metadata.title = "New metadata".into();
    different.source = parse_link("https://music.youtube.com/watch?v=lmnopqrstuv").unwrap();
    different.links = vec![different.source.clone()];
    let (a, b) = tokio::join!(
        f.runtime.prepare_observation(&identity, vec![first]),
        f.runtime
            .prepare_observation(&identity, vec![Resolution::Track(different)])
    );
    assert_eq!(a.unwrap(), b.unwrap());
    let server = tokio::spawn(observed_audio_response(listener, true));
    for _ in 0..2 {
        assert!(observe(&f.runtime, &m).await.unwrap());
    }
    server.await.unwrap();
}

#[tokio::test]
async fn own_observation_replay_uses_snapshot_after_metadata_and_audio_link_drift() {
    for positive in [false, true] {
        let (listener, bot) = wire().await;
        let f = fixture_with(bot, true, WorkerMode::MetadataVariation).await;
        let server = tokio::spawn(observed_audio_response(listener, positive));
        let m = own_observed_card("https://open.spotify.com/track/abc");
        let expected = if positive {
            Ok(true)
        } else {
            Err(MusicError::DeliveryUnknown)
        };
        assert_eq!(observe(&f.runtime, &m).await, expected);
        let token = observation_token(&observation_identity(&m, 42, 123));
        let before = f
            .runtime
            .persistence
            .store
            .text_effect(&token)
            .await
            .unwrap()
            .unwrap()
            .identity
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let changed = f
            .runtime
            .service
            .resolve(
                MusicActor {
                    user_id: 42,
                    group: None,
                },
                "https://open.spotify.com/track/abc",
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_ne!(
            serde_json::to_value(vec![&changed]).unwrap(),
            before.payload["resolution"]
        );
        assert_eq!(f.started.load(std::sync::atomic::Ordering::SeqCst), 2);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(observe(&f.runtime, &m).await, expected);
        assert_eq!(
            f.started.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "replay must not resolve new metadata"
        );
        let after = f
            .runtime
            .persistence
            .store
            .text_effect(&token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.identity.as_ref(), Some(&before));
        assert_eq!(after.outcome, TextOutcome::Prepared);
        let rows = query(
            "SELECT count(*) AS n FROM runtime_text_effects WHERE token LIKE 'music:audio:%'",
        )
        .fetch_one(&f.runtime.persistence.database)
        .await
        .unwrap();
        assert_eq!(rows.try_get::<i64, _>("n").unwrap(), 1);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn own_observation_authority_and_cancellation_prevent_audio_admission() {
    for cancel in [false, true] {
        let (listener, bot) = wire().await;
        let f = fixture_with(bot, true, WorkerMode::PendingSearch).await;
        let runtime = f.runtime.clone();
        let m = own_observed_card("https://open.spotify.com/track/abc");
        let identity = observation_identity(&m, 42, 123);
        let task = tokio::spawn(async move { observe(&runtime, &m).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while f.started.load(std::sync::atomic::Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if cancel {
            f.runtime.cancel_all();
            assert_eq!(task.await.unwrap(), Err(MusicError::Cancelled));
            f.runtime.drain_handlers().await.unwrap();
        } else {
            query("DELETE FROM user_permissions WHERE user_id=42")
                .execute(&f.runtime.persistence.database)
                .await
                .unwrap();
            assert!(matches!(
                task.await.unwrap(),
                Ok(false) | Err(MusicError::Denied)
            ));
        }
        assert!(
            f.runtime
                .persistence
                .store
                .text_effect(&observation_token(&identity))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
    }
    let (listener, bot) = wire().await;
    let f = fixture_with(bot, true, WorkerMode::Playback).await;
    let m = own_observed_card("https://open.spotify.com/track/abc");
    let identity = observation_identity(&m, 42, 123);
    f.runtime
        .prepare_observation(&identity, vec![Resolution::Track(track())])
        .await
        .unwrap();
    query("DELETE FROM user_permissions WHERE user_id=42")
        .execute(&f.runtime.persistence.database)
        .await
        .unwrap();
    assert_eq!(observe(&f.runtime, &m).await, Err(MusicError::Denied));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
    assert_eq!(
        f.runtime
            .persistence
            .store
            .text_effect(&observation_token(&identity))
            .await
            .unwrap()
            .unwrap()
            .outcome,
        TextOutcome::Prepared
    );
}

#[tokio::test]
async fn observation_prepared_transaction_cancel_drop_timeout_roll_back_snapshot() {
    for mode in ["cancel", "drop", "timeout"] {
        let (_, bot) = wire().await;
        let f = fixture_with(bot, false, WorkerMode::PersistenceBudget).await;
        let runtime = f.runtime.clone();
        let identity = observation_identity(
            &own_observed_card("https://open.spotify.com/track/abc"),
            42,
            123,
        );
        let proposed = identity.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            runtime
                .admission(async {
                    let mut c = runtime
                        .persistence
                        .database
                        .connection()
                        .await
                        .map_err(|_| MusicError::Persistence)?;
                    let tx = yoink_db::Transaction::new(
                        &mut c,
                        turso::transaction::TransactionBehavior::Immediate,
                    )
                    .await
                    .map_err(|_| MusicError::Persistence)?;
                    prepare_observation_on(&tx, &proposed, vec![Resolution::Track(track())])
                        .await?;
                    ready_tx.send(()).unwrap();
                    std::future::pending::<Result<(), MusicError>>().await
                })
                .await
        });
        ready_rx.await.unwrap();
        match mode {
            "cancel" => {
                f.runtime.cancel_all();
                assert_eq!(task.await.unwrap(), Err(MusicError::Cancelled));
            }
            "drop" => {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            }
            _ => {
                assert_eq!(task.await.unwrap(), Err(MusicError::Persistence));
            }
        }
        assert!(
            f.runtime
                .persistence
                .store
                .text_effect(&observation_token(&identity))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            NativeMusicAuthority(f.runtime.identities.clone())
                .allowed(MusicActor {
                    user_id: 42,
                    group: None
                })
                .await
                .unwrap()
        );
    }
}

fn observed_card_branch(source: &str, own: bool, foreign: bool) -> Message {
    let mut raw = serde_json::to_value(own_observed_card(source)).unwrap();
    if !own {
        if foreign {
            raw["via_bot"] = json!({"id":999,"is_bot":true,"first_name":"Other Bot"});
        } else {
            raw.as_object_mut().unwrap().remove("via_bot");
        }
    }
    serde_json::from_value(raw).unwrap()
}
async fn winning_observation_response(listener: TcpListener) -> bool {
    let (mut s, _) = listener.accept().await.unwrap();
    let (head, body) = request(&mut s).await;
    let card = head.contains("sendMessage");
    if card {
        let value: Value = serde_json::from_slice(&body).unwrap();
        let mut r = receipt(76, 42, None);
        r["text"] = value["text"].clone();
        response(&mut s, r).await;
        let (mut audio, _) = listener.accept().await.unwrap();
        let (head, _) = request(&mut audio).await;
        assert!(head.contains("sendAudio"));
        s = audio;
    } else {
        assert!(head.contains("sendAudio"));
    }
    let mut r = receipt(77, 42, None);
    r["audio"] = json!({"file_id":"native-file","file_unique_id":"unique","duration":1,"mime_type":"audio/mpeg"});
    response(&mut s, r).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(300), listener.accept())
            .await
            .is_err(),
        "losing observer must not send a card or audio"
    );
    card
}
async fn assert_observation_effect_counts(runtime: &MusicRuntime, cards: i64) {
    for (prefix, expected) in [
        ("music:observation:%", 1),
        ("music:card:%", cards),
        ("music:audio:%", 1),
    ] {
        let row = query("SELECT count(*) AS n FROM runtime_text_effects WHERE token LIKE ?")
            .bind(prefix)
            .fetch_one(&runtime.persistence.database)
            .await
            .unwrap();
        assert_eq!(row.try_get::<i64, _>("n").unwrap(), expected, "{prefix}");
    }
}

#[tokio::test]
async fn cross_branch_paused_resolution_conflicts_after_other_branch_commits() {
    for paused_own in [false, true] {
        for foreign in [false, true] {
            let (listener, bot) = wire().await;
            let f = fixture_with(bot, true, WorkerMode::InterleavedObservation).await;
            let paused =
                observed_card_branch("https://open.spotify.com/track/def", paused_own, foreign);
            let winner =
                observed_card_branch("https://open.spotify.com/track/abc", !paused_own, foreign);
            let runtime = f.runtime.clone();
            let losing_task = tokio::spawn(async move { observe(&runtime, &paused).await });
            tokio::time::timeout(Duration::from_secs(2), f.pause.reached.acquire())
                .await
                .unwrap()
                .unwrap()
                .forget();
            let token = observation_token(&observation_identity(&winner, 42, 123));
            assert!(
                f.runtime
                    .persistence
                    .store
                    .text_effect(&token)
                    .await
                    .unwrap()
                    .is_none()
            );
            let server = tokio::spawn(winning_observation_response(listener));
            assert!(observe(&f.runtime, &winner).await.unwrap());
            let effect = f
                .runtime
                .persistence
                .store
                .text_effect(&token)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(effect.outcome, TextOutcome::Prepared);
            assert_eq!(
                effect.identity.as_ref().unwrap().payload["resolution"][0]["Track"]["source"]["url"],
                "https://open.spotify.com/track/abc"
            );
            f.pause.resume.add_permits(1);
            assert_eq!(losing_task.await.unwrap(), Err(MusicError::Conflict));
            assert!(observe(&f.runtime, &winner).await.unwrap());
            assert_eq!(server.await.unwrap(), paused_own);
            assert_observation_effect_counts(&f.runtime, i64::from(paused_own)).await;
        }
    }
}

#[tokio::test]
async fn concurrent_cross_branch_observations_choose_one_durable_message_intent() {
    for foreign in [false, true] {
        let (listener, bot) = wire().await;
        let f = fixture_with(bot, true, WorkerMode::Playback).await;
        let own = observed_card_branch("https://open.spotify.com/track/abc", true, false);
        let passive = observed_card_branch("https://open.spotify.com/track/def", false, foreign);
        let server = tokio::spawn(winning_observation_response(listener));
        let (a, b) = tokio::join!(observe(&f.runtime, &own), observe(&f.runtime, &passive));
        let own_won = match (a, b) {
            (Ok(true), Err(MusicError::Conflict)) => true,
            (Err(MusicError::Conflict), Ok(true)) => false,
            other => panic!("unexpected cross-branch outcomes: {other:?}"),
        };
        assert_eq!(server.await.unwrap(), !own_won);
        assert_observation_effect_counts(&f.runtime, i64::from(!own_won)).await;
    }
}

#[tokio::test]
async fn passive_multi_link_observation_replays_complete_ordered_snapshot_after_drift() {
    let (listener, bot) = wire().await;
    let f = fixture_with(bot, false, WorkerMode::MetadataVariation).await;
    let m = message(
        11,
        "listen https://open.spotify.com/track/abc https://open.spotify.com/track/def",
    );
    let server = tokio::spawn(async move {
        for (index, source) in [(1, "abc"), (2, "def")] {
            let (mut s, _) = listener.accept().await.unwrap();
            let (head, body) = request(&mut s).await;
            assert!(head.contains("sendMessage"));
            let value: Value = serde_json::from_slice(&body).unwrap();
            assert!(
                value["text"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("Song {index}"))
            );
            assert_eq!(
                value["entities"][0]["url"],
                format!("https://open.spotify.com/track/{source}")
            );
            let mut r = receipt(76 + index, 42, None);
            r["text"] = value["text"].clone();
            response(&mut s, r).await;
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(300), listener.accept())
                .await
                .is_err()
        );
    });
    assert!(observe(&f.runtime, &m).await.unwrap());
    let token = observation_token(&observation_identity(&m, 42, 123));
    let before = f
        .runtime
        .persistence
        .store
        .text_effect(&token)
        .await
        .unwrap()
        .unwrap()
        .identity
        .unwrap();
    let results: Vec<Resolution> =
        serde_json::from_value(before.payload["resolution"].clone()).unwrap();
    assert_eq!(results.len(), 2);
    for (resolution, source) in results.iter().zip(["abc", "def"]) {
        let Resolution::Track(track) = resolution else {
            panic!("expected track")
        };
        assert_eq!(
            track.source.url.as_url().as_str(),
            format!("https://open.spotify.com/track/{source}")
        );
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    let drift = f
        .runtime
        .service
        .resolve(
            MusicActor {
                user_id: 42,
                group: None,
            },
            "https://open.spotify.com/track/abc",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_ne!(drift, results[0]);
    assert!(observe(&f.runtime, &m).await.unwrap());
    assert_eq!(f.started.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert_eq!(
        f.runtime
            .persistence
            .store
            .text_effect(&token)
            .await
            .unwrap()
            .unwrap()
            .identity
            .as_ref(),
        Some(&before)
    );
    server.await.unwrap();
}

#[tokio::test]
async fn legacy_single_result_observation_intent_retains_audio_replay_identity() {
    let (listener, bot) = wire().await;
    let f = fixture_with(bot, true, WorkerMode::Playback).await;
    let m = own_observed_card("https://open.spotify.com/track/abc");
    let mut identity = observation_identity(&m, 42, 123);
    identity.payload["resolution"] = serde_json::to_value(Resolution::Track(track())).unwrap();
    {
        let mut c = f.runtime.persistence.database.connection().await.unwrap();
        let tx =
            yoink_db::Transaction::new(&mut c, turso::transaction::TransactionBehavior::Immediate)
                .await
                .unwrap();
        Store::prepare_text_on(&*tx, &observation_token(&identity), &identity)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let server = tokio::spawn(observed_audio_response(listener, true));
    for _ in 0..2 {
        assert!(observe(&f.runtime, &m).await.unwrap());
    }
    assert_eq!(
        f.runtime
            .persistence
            .store
            .text_effect(&observation_token(&identity))
            .await
            .unwrap()
            .unwrap()
            .identity
            .as_ref(),
        Some(&identity)
    );
    assert_observation_effect_counts(&f.runtime, 0).await;
    server.await.unwrap();
}

#[tokio::test]
async fn large_observation_results_commit_atomically_and_replay_without_extra_sends() {
    let (listener, bot) = wire().await;
    let f = fixture_with(bot, false, WorkerMode::LargeArtists).await;
    let urls: Vec<_> = (0..10)
        .map(|index| format!("https://open.spotify.com/artist/artist{index}"))
        .collect();
    let m = message(11, &urls.join(" "));
    let expected_urls = urls.clone();
    let server = tokio::spawn(async move {
        for (index, url) in expected_urls.iter().enumerate() {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (head, body) = request(&mut stream).await;
            assert!(head.contains("sendMessage"));
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["entities"][0]["url"], *url);
            assert!(body["text"].as_str().unwrap().encode_utf16().count() <= 4096);
            let mut result = receipt(100 + index as i32, 42, None);
            result["text"] = body["text"].clone();
            response(&mut stream, result).await;
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(500), listener.accept())
                .await
                .is_err()
        );
    });
    assert!(observe(&f.runtime, &m).await.unwrap());
    let identity = observation_identity(&m, 42, 123);
    let token = observation_token(&identity);
    let snapshot = f
        .runtime
        .observed_snapshot(&identity)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.len(), 10);
    assert!(serde_json::to_vec(&snapshot).unwrap().len() > 65536);
    for (result, url) in snapshot.iter().zip(&urls) {
        let Resolution::Artist { source, result } = result else {
            panic!("expected artist")
        };
        assert!(result.valid());
        assert_eq!(source.as_url().as_str(), url);
    }
    for index in 0..10 {
        let effect = f
            .runtime
            .persistence
            .store
            .text_effect(&format!("{token}:result:{index}"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(effect.outcome, TextOutcome::Prepared);
        assert!(effect.identity.unwrap().valid());
    }
    let drift = f
        .runtime
        .service
        .resolve(
            MusicActor {
                user_id: 42,
                group: None,
            },
            &urls[0],
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_ne!(snapshot[0], drift);
    assert!(observe(&f.runtime, &m).await.unwrap());
    assert_eq!(f.started.load(std::sync::atomic::Ordering::SeqCst), 11);
    assert_eq!(
        f.runtime
            .observed_snapshot(&identity)
            .await
            .unwrap()
            .unwrap(),
        snapshot
    );

    let other = observation_identity(&message(12, &urls.join(" ")), 42, 123);
    let mut oversized = snapshot.clone();
    let Resolution::Artist {
        result: MusicResult::Artist { name, .. },
        ..
    } = oversized.last_mut().unwrap()
    else {
        panic!("expected artist")
    };
    *name = "x".repeat(65537);
    assert_eq!(
        f.runtime
            .prepare_observation(&other, oversized)
            .await
            .unwrap_err(),
        MusicError::Persistence
    );
    let other_token = observation_token(&other);
    assert_eq!(
        query("SELECT count(*) FROM runtime_text_effects WHERE token=? OR token LIKE ?")
            .bind(&other_token)
            .bind(format!("{other_token}:result:%"))
            .fetch_one(f.runtime.persistence.store.database())
            .await
            .unwrap()
            .try_get::<i64, _>(0)
            .unwrap(),
        0
    );

    query("DELETE FROM runtime_text_effects WHERE token=?")
        .bind(format!("{token}:result:9"))
        .execute(f.runtime.persistence.store.database())
        .await
        .unwrap();
    assert_eq!(
        observe(&f.runtime, &m).await.unwrap_err(),
        MusicError::Persistence
    );
    assert_eq!(f.started.load(std::sync::atomic::Ordering::SeqCst), 11);
    server.await.unwrap();
}
