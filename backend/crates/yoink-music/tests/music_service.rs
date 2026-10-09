use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use yoink_executor::ValidatedArtifact;
use yoink_music::*;
use yoink_storage::{Store, TextIdentity};
use yoink_worker_protocol::{HttpSourceUrl, MusicRequest};

fn settings() -> MusicSettings {
    MusicSettings {
        request_timeout: Duration::from_secs(1),
        lookup_timeout: Duration::from_secs(1),
        resolve_timeout: Duration::from_secs(3),
        audio_timeout: Duration::from_secs(3),
        delivery_timeout: Duration::from_secs(1),
        persistence_timeout: Duration::from_secs(2),
        cache_ttl: Duration::from_secs(60),
        cache_entries: 2,
        worker_concurrency: 2,
        response_max_bytes: 4096,
        max_requests: 10,
        audio_max_bytes: 1024,
        max_links_per_command: 10,
        download_enabled: true,
        credentials: MusicCredentials::default(),
        proxies: BTreeMap::new(),
    }
}

#[test]
fn prose_url_spans_keep_strict_validation_and_deduplication() {
    let url = "https://open.spotify.com/track/abc";
    let links = extract_links(
        &format!("Listen: ({url}). <{url}> '[https://www.deezer.com/track/42]'"),
        &[url.into()],
        10,
    );
    assert_eq!(links.len(), 2);
    assert_eq!(links[0], parse_link(url).unwrap());
    assert_eq!(extract_links(&format!("Listen: ({url})"), &[], 10).len(), 1);
    assert!(extract_links("(https://open.spotify.com.evil.test/track/a) https://user:pw@open.spotify.com/track/a http://127.0.0.1/track/a", &[], 10).is_empty());
    assert_eq!(
        extract_links(&format!("({url}) https://www.deezer.com/track/42"), &[], 1).len(),
        1
    );
}

struct OrderedAudioWorker {
    executor: yoink_executor::Executor,
    calls: std::sync::Mutex<Vec<&'static str>>,
    fail_music: bool,
}
impl MusicWorker for OrderedAudioWorker {
    fn lookup(
        &self,
        r: MusicRequest,
        _: CancellationToken,
        _: Duration,
    ) -> MusicFuture<'_, MusicResult> {
        Box::pin(async move {
            match r.task {
                MusicTask::MusicSearch {
                    provider: MusicProvider::Ytmusic,
                    ..
                } => {
                    self.calls.lock().unwrap().push("music-search");
                    Ok(MusicResult::Link {
                        url: Some(
                            HttpSourceUrl::parse("https://music.youtube.com/watch?v=abcdefghijk")
                                .unwrap(),
                        ),
                    })
                }
                MusicTask::MusicSearch {
                    provider: MusicProvider::Youtube,
                    ..
                } => {
                    self.calls.lock().unwrap().push("youtube-search");
                    if !self.fail_music {
                        std::future::pending::<()>().await;
                    }
                    Ok(MusicResult::Link {
                        url: Some(
                            HttpSourceUrl::parse("https://www.youtube.com/watch?v=bcdefghijkl")
                                .unwrap(),
                        ),
                    })
                }
                _ => Err(MusicError::SourceUnavailable),
            }
        })
    }
    fn audio(
        &self,
        r: MusicRequest,
        c: CancellationToken,
        _: Duration,
        _: u64,
    ) -> MusicFuture<'_, Vec<ValidatedArtifact>> {
        Box::pin(async move {
            let MusicTask::MusicAudio { ref url, .. } = r.task else {
                return Err(MusicError::InvalidInput);
            };
            let music = url.as_url().host_str() == Some("music.youtube.com");
            self.calls.lock().unwrap().push(if music {
                "music-audio"
            } else {
                "youtube-audio"
            });
            if music && self.fail_music {
                return Err(MusicError::SourceUnavailable);
            }
            self.executor
                .execute_music_audio(&r, c)
                .await
                .map_err(|_| MusicError::SourceUnavailable)
        })
    }
}
struct UploadDelivery;
impl MusicAudioDelivery for UploadDelivery {
    fn send(
        &self,
        _: AudioTarget,
        _: String,
        _: Option<String>,
        _: MusicCard,
        a: MusicAudioArtifact,
    ) -> MusicFuture<'_, AudioReceipt> {
        Box::pin(async move {
            assert!(matches!(a, MusicAudioArtifact::Upload(_)));
            Ok(AudioReceipt {
                message_id: 123,
                file_id: None,
            })
        })
    }
}
#[tokio::test]
async fn music_search_audio_precedes_slow_youtube_and_failure_falls_back() {
    for fail_music in [false, true] {
        let (_dir, native) = persistence().await;
        let workspace = tempfile::tempdir().unwrap();
        let executor = yoink_executor::Executor::new(yoink_executor::ExecutorConfig {
            program: "/usr/bin/node".into(),
            args: vec![format!(
                "{}/tests/music_audio_fixture.mjs",
                env!("CARGO_MANIFEST_DIR")
            )],
            workspace_root: workspace.path().into(),
            timeout: Duration::from_secs(1),
            processing_timeout: Duration::from_secs(1),
            socket_timeout: Duration::from_secs(1),
            max_artifact_bytes: 1024,
            max_workspace_bytes: 1024,
            max_output_bytes: 16384,
            max_artifacts: 1,
            allowed_proxy_origins: vec![],
            allowed_private_origins: vec![],
        })
        .unwrap();
        let worker = Arc::new(OrderedAudioWorker {
            executor,
            calls: std::sync::Mutex::new(vec![]),
            fail_music,
        });
        let mut config = settings();
        config.audio_timeout = Duration::from_secs(1);
        config.lookup_timeout = Duration::from_secs(1);
        let service = MusicService::new(
            Arc::new(Authority(AtomicBool::new(true))),
            worker.clone(),
            Arc::new(Analytics(AtomicUsize::new(0))),
            config,
        )
        .unwrap();
        let t = track();
        assert_eq!(
            service
                .send_audio(
                    audio_command(&t),
                    Arc::new(native),
                    Arc::new(UploadDelivery),
                    CancellationToken::new()
                )
                .await
                .unwrap(),
            AudioOutcome::Confirmed(123)
        );
        assert_eq!(
            *worker.calls.lock().unwrap(),
            if fail_music {
                vec![
                    "music-search",
                    "music-audio",
                    "youtube-search",
                    "youtube-audio",
                ]
            } else {
                vec!["music-search", "music-audio"]
            }
        );
    }
}
struct Authority(AtomicBool);
impl MusicAuthority for Authority {
    fn allowed(&self, _: MusicActor) -> MusicFuture<'_, bool> {
        Box::pin(async { Ok(self.0.load(Ordering::SeqCst)) })
    }
}
struct Worker {
    calls: AtomicUsize,
    fail: bool,
}
impl MusicWorker for Worker {
    fn lookup(
        &self,
        r: MusicRequest,
        _: CancellationToken,
        _: Duration,
    ) -> MusicFuture<'_, MusicResult> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(MusicError::SourceUnavailable);
            }
            match r.task {
                MusicTask::MusicParse { provider, .. } => Ok(MusicResult::Metadata {
                    metadata: MusicMetadata {
                        title: "Песня 🎵".into(),
                        artist: if provider == MusicProvider::Spotify {
                            None
                        } else {
                            Some("Артист".into())
                        },
                        thumbnail_url: None,
                    },
                }),
                MusicTask::MusicSearch { provider, .. } => Ok(MusicResult::Link {
                    url: if provider == MusicProvider::Deezer {
                        Some(HttpSourceUrl::parse("https://www.deezer.com/track/42").unwrap())
                    } else {
                        None
                    },
                }),
                _ => Err(MusicError::SourceUnavailable),
            }
        })
    }
    fn audio(
        &self,
        _: MusicRequest,
        _: CancellationToken,
        _: Duration,
        _: u64,
    ) -> MusicFuture<'_, Vec<ValidatedArtifact>> {
        Box::pin(async { Err(MusicError::ResourceLimit) })
    }
}
struct Analytics(AtomicUsize);
impl ResolveAnalytics for Analytics {
    fn record(&self, _: i64, _: Track) -> MusicFuture<'_, ()> {
        Box::pin(async {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}
fn actor() -> MusicActor {
    MusicActor {
        user_id: 7,
        group: None,
    }
}
fn track() -> Track {
    let source = parse_link("https://open.spotify.com/track/abc").unwrap();
    Track {
        metadata: MusicMetadata {
            title: "Song".into(),
            artist: Some("Artist".into()),
            thumbnail_url: None,
        },
        links: vec![source.clone()],
        source,
    }
}
fn service(
    a: Arc<dyn MusicAuthority>,
    w: Arc<dyn MusicWorker>,
    log: Arc<dyn ResolveAnalytics>,
) -> MusicService {
    MusicService::new(a, w, log, settings()).unwrap()
}
const PROVIDER_FIXTURES: [(&str, MusicProvider); 10] = [
    (
        "https://open.spotify.com/intl-en/track/abc?si=tracking",
        MusicProvider::Spotify,
    ),
    ("https://www.deezer.com/en/track/42", MusicProvider::Deezer),
    (
        "https://music.yandex.ru/album/1/track/2",
        MusicProvider::Yandex,
    ),
    (
        "https://music.youtube.com/watch?v=abcdefghijk",
        MusicProvider::Ytmusic,
    ),
    (
        "https://soundcloud.com/artist/song",
        MusicProvider::Soundcloud,
    ),
    (
        "https://music.apple.com/us/album/song/1?i=2",
        MusicProvider::AppleMusic,
    ),
    (
        "https://artist.bandcamp.com/track/song",
        MusicProvider::Bandcamp,
    ),
    (
        "https://www.qobuz.com/us-en/album/song/abc",
        MusicProvider::Qobuz,
    ),
    ("https://tidal.com/browse/track/42", MusicProvider::Tidal),
    ("https://youtu.be/abcdefghijk", MusicProvider::Youtube),
];
#[test]
fn provider_links_invalid_private_inputs_and_normalization() {
    for (url, p) in PROVIDER_FIXTURES {
        assert_eq!(parse_link(url).unwrap().provider, p)
    }
    for url in [
        "http://127.0.0.1/track/1",
        "https://music.youtube.com.evil.test/watch?v=abcdefghijk",
        "https://user:password@open.spotify.com/track/1",
        "file:///tmp/music",
        "https://open.spotify.com:444/track/a",
        "https://open.spotify.com/playlist/a",
        "https://soundcloud.com/a/sets",
        "https://youtu.be/a",
    ] {
        assert_eq!(parse_link(url), Err(MusicError::InvalidInput));
    }
    assert!(
        !parse_link(PROVIDER_FIXTURES[0].0)
            .unwrap()
            .url
            .as_url()
            .as_str()
            .contains("si=")
    );
    assert_eq!(
        extract_links(
            "https://www.deezer.com/track/42",
            &["https://www.deezer.com/track/42".into()],
            10
        )
        .len(),
        1
    );
}
#[tokio::test]
async fn supported_provider_orchestration_source_first_fallback_and_artist_recovery() {
    let a = Arc::new(Authority(AtomicBool::new(true)));
    let w = Arc::new(Worker {
        calls: AtomicUsize::new(0),
        fail: false,
    });
    let log = Arc::new(Analytics(AtomicUsize::new(0)));
    let s = service(a, w, log.clone());
    for (url, p) in PROVIDER_FIXTURES {
        let Resolution::Track(t) = s
            .resolve(actor(), url, CancellationToken::new())
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(t.source.provider, p);
        assert_eq!(t.links[0], t.source);
        assert_eq!(t.metadata.artist.as_deref(), Some("Артист"));
        if p != MusicProvider::Youtube {
            assert!(
                !t.links
                    .iter()
                    .skip(1)
                    .any(|l| l.provider == MusicProvider::Youtube)
            );
        }
    }
    assert_eq!(log.0.load(Ordering::SeqCst), 10);
}
#[tokio::test]
async fn fresh_grant_denial_and_revocation_cover_cached_inline() {
    let a = Arc::new(Authority(AtomicBool::new(false)));
    let w = Arc::new(Worker {
        calls: AtomicUsize::new(0),
        fail: false,
    });
    let log = Arc::new(Analytics(AtomicUsize::new(0)));
    let s = service(a.clone(), w.clone(), log.clone());
    assert_eq!(
        s.inline(7, PROVIDER_FIXTURES[0].0, CancellationToken::new())
            .await
            .unwrap_err(),
        MusicError::Denied
    );
    assert_eq!(w.calls.load(Ordering::SeqCst), 0);
    a.0.store(true, Ordering::SeqCst);
    let answer = s
        .inline(7, PROVIDER_FIXTURES[0].0, CancellationToken::new())
        .await
        .unwrap();
    assert!(answer.is_personal);
    assert_eq!(answer.cache_time, 0);
    let calls = w.calls.load(Ordering::SeqCst);
    s.inline(7, PROVIDER_FIXTURES[0].0, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(w.calls.load(Ordering::SeqCst), calls);
    assert_eq!(log.0.load(Ordering::SeqCst), 2);
    a.0.store(false, Ordering::SeqCst);
    assert_eq!(
        s.inline(7, PROVIDER_FIXTURES[0].0, CancellationToken::new())
            .await
            .unwrap_err(),
        MusicError::Denied
    );
}
#[tokio::test]
async fn operational_failure_cancel_and_settings_bounds() {
    let a = Arc::new(Authority(AtomicBool::new(true)));
    let s = service(
        a,
        Arc::new(Worker {
            calls: AtomicUsize::new(0),
            fail: true,
        }),
        Arc::new(Analytics(AtomicUsize::new(0))),
    );
    assert_eq!(
        s.resolve(actor(), PROVIDER_FIXTURES[0].0, CancellationToken::new())
            .await
            .unwrap_err(),
        MusicError::SourceUnavailable
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        s.resolve(actor(), PROVIDER_FIXTURES[0].0, cancel)
            .await
            .unwrap_err(),
        MusicError::Cancelled
    );
    let mut config = settings();
    config.worker_concurrency = 0;
    assert_eq!(config.validate(), Err(MusicError::InvalidInput));
}
#[test]
fn utf16_cards_and_locales() {
    let mut t = track();
    t.metadata.title = "🎵 Музыка".into();
    let mut card = MusicCard::from_resolution(&Resolution::Track(t));
    let offset = card.text.split('\n').next().unwrap().encode_utf16().count() + 1;
    assert_eq!(card.entities[0].offset, offset);
    card.add_requester(7, "😀 Adams");
    assert_eq!(card.entities.last().unwrap().length, 8);
    assert_ne!(
        error_message(MusicError::Denied, "en"),
        error_message(MusicError::Denied, "ru")
    );
    assert_eq!(
        error_message(MusicError::Denied, "ru-RU"),
        error_message(MusicError::Denied, "ru")
    );
}

struct CachedPersistence(NativeMusicPersistence, Option<Arc<Authority>>);
impl MusicPersistence for CachedPersistence {
    fn audio_log(&self, log: AudioDownloadLog) -> MusicFuture<'_, ()> {
        self.0.audio_log(log)
    }
    fn effect(&self, t: String) -> MusicFuture<'_, Option<AudioEffect>> {
        self.0.effect(t)
    }
    fn begin(&self, t: String, i: TextIdentity) -> MusicFuture<'_, bool> {
        Box::pin(async move {
            let admitted = self.0.begin(t, i).await?;
            if let Some(authority) = &self.1 {
                authority.0.store(false, Ordering::SeqCst);
            }
            Ok(admitted)
        })
    }
    fn confirm(&self, t: String, m: i32) -> MusicFuture<'_, ()> {
        self.0.confirm(t, m)
    }
    fn unknown(&self, t: String) -> MusicFuture<'_, ()> {
        self.0.unknown(t)
    }
    fn cached(&self, _: String) -> MusicFuture<'_, Option<String>> {
        Box::pin(async { Ok(Some("fixture-file-id".into())) })
    }
    fn cache(&self, k: String, f: String, t: String, d: Duration) -> MusicFuture<'_, ()> {
        self.0.cache(k, f, t, d)
    }
}
struct Delivery {
    calls: AtomicUsize,
    unknown: bool,
}
impl MusicAudioDelivery for Delivery {
    fn send(
        &self,
        _: AudioTarget,
        _: String,
        _: Option<String>,
        _: MusicCard,
        artifact: MusicAudioArtifact,
    ) -> MusicFuture<'_, AudioReceipt> {
        Box::pin(async move {
            assert!(matches!(artifact, MusicAudioArtifact::Cached(_)));
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.unknown {
                Err(MusicError::DeliveryUnknown)
            } else {
                Ok(AudioReceipt {
                    message_id: 123,
                    file_id: Some("fixture-receipt-id".into()),
                })
            }
        })
    }
}
async fn persistence() -> (tempfile::TempDir, NativeMusicPersistence) {
    let dir = tempfile::tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Arc::new(
        Store::open(&dir.path().join("db"), true, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    store.migrate().await.unwrap();
    let database = store.database().clone();
    (dir, NativeMusicPersistence { database, store })
}
fn target() -> AudioTarget {
    AudioTarget {
        bot_id: 100,
        chat_id: 7,
        topic_id: None,
        incoming_message_id: 50,
    }
}
fn audio_command(t: &Track) -> AudioCommand<'_> {
    AudioCommand {
        actor: actor(),
        target: target(),
        track: t,
        caption: MusicCard::from_resolution(&Resolution::Track(t.clone())),
    }
}
#[tokio::test]
async fn concurrent_audio_receipt_replay_and_restart_do_not_resend() {
    let (_dir, native) = persistence().await;
    let store = native.store.clone();
    let p = Arc::new(CachedPersistence(native, None));
    let a = Arc::new(Authority(AtomicBool::new(true)));
    let w = Arc::new(Worker {
        calls: AtomicUsize::new(0),
        fail: false,
    });
    let log = Arc::new(Analytics(AtomicUsize::new(0)));
    let s = service(a.clone(), w.clone(), log.clone());
    let d = Arc::new(Delivery {
        calls: AtomicUsize::new(0),
        unknown: false,
    });
    let t = track();
    let (r1, r2) = tokio::join!(
        s.send_audio(
            audio_command(&t),
            p.clone(),
            d.clone(),
            CancellationToken::new()
        ),
        s.send_audio(
            audio_command(&t),
            p.clone(),
            d.clone(),
            CancellationToken::new()
        )
    );
    assert!([r1.unwrap(), r2.unwrap()].contains(&AudioOutcome::Confirmed(123)));
    assert_eq!(d.calls.load(Ordering::SeqCst), 1);
    let restarted = service(a.clone(), w, log);
    assert_eq!(
        restarted
            .send_audio(
                audio_command(&t),
                p.clone(),
                d.clone(),
                CancellationToken::new()
            )
            .await
            .unwrap(),
        AudioOutcome::Confirmed(123)
    );
    assert!(
        store
            .unbound_text_receipts_page(100, 256)
            .await
            .unwrap()
            .is_empty()
    );
    a.0.store(false, Ordering::SeqCst);
    assert_eq!(
        restarted
            .send_audio(audio_command(&t), p, d.clone(), CancellationToken::new())
            .await
            .unwrap_err(),
        MusicError::Denied
    );
    assert_eq!(d.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn uncertain_audio_delivery_is_terminal_for_replay() {
    let (_dir, native) = persistence().await;
    let p = Arc::new(CachedPersistence(native, None));
    let s = service(
        Arc::new(Authority(AtomicBool::new(true))),
        Arc::new(Worker {
            calls: AtomicUsize::new(0),
            fail: false,
        }),
        Arc::new(Analytics(AtomicUsize::new(0))),
    );
    let d = Arc::new(Delivery {
        calls: AtomicUsize::new(0),
        unknown: true,
    });
    let t = track();
    for _ in 0..2 {
        assert_eq!(
            s.send_audio(
                audio_command(&t),
                p.clone(),
                d.clone(),
                CancellationToken::new()
            )
            .await
            .unwrap(),
            AudioOutcome::Unknown
        );
    }
    assert_eq!(d.calls.load(Ordering::SeqCst), 1);
    let mut altered = t.clone();
    altered.metadata.title = "Other".into();
    assert_eq!(
        s.send_audio(audio_command(&altered), p, d, CancellationToken::new())
            .await
            .unwrap_err(),
        MusicError::Conflict
    );
}
#[tokio::test]
async fn native_identity_uses_fresh_music_grants_and_native_cache_storage() {
    let (_dir, native) = persistence().await;
    yoink_db::query("INSERT INTO users(id,role,created_at,updated_at) VALUES(7,'user',0,0)")
        .execute(&native.database)
        .await
        .unwrap();
    let authority = NativeMusicAuthority(Arc::new(yoink_identity::IdentityStore::new(
        native.database.clone(),
        None,
        false,
    )));
    assert!(authority.allowed(actor()).await.unwrap());
    yoink_db::query("UPDATE users SET role='restricted' WHERE id=7")
        .execute(&native.database)
        .await
        .unwrap();
    assert!(!authority.allowed(actor()).await.unwrap());
    native
        .cache(
            "music:native:test".into(),
            "file-id".into(),
            "title".into(),
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert_eq!(
        native.cached("music:native:test".into()).await.unwrap(),
        Some("file-id".into())
    );
    assert_eq!(native.cached("other-owner".into()).await.unwrap(), None);
}

#[tokio::test]
async fn authority_revoked_while_arming_audio_prevents_send_and_replay() {
    let (_dir, native) = persistence().await;
    let authority = Arc::new(Authority(AtomicBool::new(true)));
    let persistence = Arc::new(CachedPersistence(native, Some(authority.clone())));
    let service = service(
        authority.clone(),
        Arc::new(Worker {
            calls: AtomicUsize::new(0),
            fail: false,
        }),
        Arc::new(Analytics(AtomicUsize::new(0))),
    );
    let delivery = Arc::new(Delivery {
        calls: AtomicUsize::new(0),
        unknown: false,
    });
    let track = track();
    assert_eq!(
        service
            .send_audio(
                audio_command(&track),
                persistence.clone(),
                delivery.clone(),
                CancellationToken::new()
            )
            .await,
        Err(MusicError::Denied)
    );
    authority.0.store(true, Ordering::SeqCst);
    assert_eq!(
        service
            .send_audio(
                audio_command(&track),
                persistence,
                delivery.clone(),
                CancellationToken::new()
            )
            .await,
        Ok(AudioOutcome::Unknown)
    );
    assert_eq!(delivery.calls.load(Ordering::SeqCst), 0);
}
