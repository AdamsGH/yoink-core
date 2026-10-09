use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use yoink_db::{Executor, query};
use yoink_storage::{Store, StoreError};

async fn wire() -> (TcpListener, Bot) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bot = Bot::with_client(
        "123:fixture",
        reqwest::Client::builder().no_proxy().build().unwrap(),
    )
    .set_api_url(url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap());
    (listener, bot)
}
async fn request(stream: &mut TcpStream) -> (String, Value) {
    let mut bytes = Vec::new();
    let mut buf = [0; 4096];
    let end = loop {
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buf[..n]);
        if let Some(n) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
            break n + 4;
        }
    };
    let header = String::from_utf8_lossy(&bytes[..end]).into_owned();
    let length = header
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap();
    while bytes.len() < end + length {
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buf[..n]);
    }
    (
        header,
        serde_json::from_slice(&bytes[end..end + length]).unwrap(),
    )
}
async fn reply(stream: &mut TcpStream, body: Value) {
    let body = body.to_string();
    stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
}
fn receipt(body: &Value, sender: i64) -> Value {
    let mut result = json!({"message_id":99,"date":1,"chat":{"id":body["chat_id"],"type":"supergroup","title":"fixture"},"from":{"id":sender,"is_bot":true,"first_name":"fixture"},"text":body["text"]});
    if body["chat_id"].as_i64().is_some_and(|chat| chat > 0) {
        result["chat"] = json!({"id":body["chat_id"],"type":"private","first_name":"fixture"});
    }
    if let Some(topic) = body.get("message_thread_id") {
        result["message_thread_id"] = topic.clone();
    }
    json!({"ok":true,"result":result})
}
fn budget() -> LookupBudget {
    LookupBudget {
        timeout: Duration::from_secs(2),
        response_bytes: 4096,
    }
}

#[tokio::test]
async fn fresh_membership_mapping_and_names() {
    let (listener, bot) = wire().await;
    let provider = BotMembershipProvider::new(bot, budget()).unwrap();
    let server = tokio::spawn(async move {
        for (status, member) in [
            ("member", false),
            ("administrator", false),
            ("creator", false),
            ("restricted", true),
            ("restricted", false),
            ("left", false),
            ("kicked", false),
        ] {
            let (mut s, _) = listener.accept().await.unwrap();
            let (header, body) = request(&mut s).await;
            assert!(header.contains("/bot123:fixture/getChatMember"));
            assert_eq!(body, json!({"chat_id":-100,"user_id":42}));
            reply(&mut s, json!({"ok":true,"result":{"status":status,"is_member":member,"user":{"id":42,"first_name":"Ada"}}})).await;
        }
        for body in [
            json!({"ok":false,"error_code":400}),
            json!({"ok":false,"error_code":403}),
            json!({"ok":true,"result":{"user":{"id":42,"first_name":" ","username":""}}}),
            json!({"ok":true,"result":{"user":{"id":42,"first_name":"Deleted Account"}}}),
            json!({"ok":true,"result":{"user":{"id":42,"is_deleted":true,"first_name":"Old"}}}),
            json!({"ok":true,"result":{"user":{"id":99,"first_name":"Wrong"}}}),
            json!({"ok":true,"result":{"id":42,"type":"private","first_name":"Ada","last_name":"Lovelace","username":"ada"}}),
        ] {
            let (mut s, _) = listener.accept().await.unwrap();
            let _ = request(&mut s).await;
            reply(&mut s, body).await;
        }
    });
    for (member, administrator) in [
        (true, false),
        (true, true),
        (true, true),
        (true, false),
        (false, false),
        (false, false),
        (false, false),
    ] {
        let access = provider.lookup(-100, 42).await.unwrap();
        assert_eq!(
            (access.member, access.administrator),
            (member, administrator)
        );
    }
    let unavailable = provider.lookup(-100, 42).await.unwrap();
    assert!(!unavailable.member && !unavailable.administrator);
    assert!(
        provider
            .lookup_name(Some(-100), 42)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        provider
            .lookup_name(Some(-100), 42)
            .await
            .unwrap()
            .is_none()
    );
    for _ in 0..3 {
        assert!(
            provider
                .lookup_name(Some(-100), 42)
                .await
                .unwrap()
                .is_none()
        );
    }
    let name = provider.lookup_name(None, 42).await.unwrap().unwrap();
    assert_eq!(name.display_name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(name.username.as_deref(), Some("ada"));
    server.await.unwrap();
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Arc<Store>,
    runtime: StatsRuntime,
}
async fn fixture(bot: Bot) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Arc::new(
        Store::open(&dir.path().join("test.db"), true, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    store.migrate().await.unwrap();
    query("INSERT INTO users(id,role,created_at,updated_at) VALUES(42,'user',1,1)")
        .execute(store.database())
        .await
        .unwrap();
    let identity = Arc::new(yoink_identity::IdentityStore::new(
        store.database().clone(),
        None,
        true,
    ));
    let provider = Arc::new(BotMembershipProvider::new(bot.clone(), budget()).unwrap());
    let stats = Stats::open(
        store.database().clone(),
        identity,
        provider,
        None,
        dir.path().join("index"),
    )
    .await
    .unwrap();
    let delivery = TextDelivery::new(bot, 123, store.clone(), Duration::from_secs(2)).unwrap();
    Fixture {
        _dir: dir,
        store,
        runtime: StatsRuntime::new(stats, delivery),
    }
}
fn identity(markup: &str) -> TextIdentity {
    TextIdentity {
        bot: 123,
        chat: -100,
        topic: Some(8),
        message: 7,
        actor: 42,
        item: None,
        edit: false,
        text: markup.into(),
        payload: json!({"command":"stats"}),
    }
}

#[tokio::test]
async fn html_entities_replay_conflict_topic_and_sender_proof() {
    let (listener, bot) = wire().await;
    let f = fixture(bot).await;
    let server = tokio::spawn(async move {
        for sender in [123, 999] {
            let (mut s, _) = listener.accept().await.unwrap();
            let (_, body) = request(&mut s).await;
            assert_eq!(body["text"], "😀<&\"nested");
            assert_eq!(
                body["entities"],
                json!([{"type":"bold","offset":0,"length":5},{"type":"pre","offset":5,"length":6}])
            );
            assert_eq!(body["message_thread_id"], 8);
            assert_eq!(body["reply_parameters"]["message_id"], 7);
            reply(&mut s, receipt(&body, sender)).await;
        }
    });
    let i = identity("<b>😀&lt;&amp;&quot;</b><pre>nested</pre>");
    let delivery = &f.runtime.delivery;
    assert_eq!(
        delivery.dispatch_html("html", &i).await.unwrap(),
        TextOutcome::Confirmed(99)
    );
    assert_eq!(
        delivery.dispatch_html("html", &i).await.unwrap(),
        TextOutcome::Confirmed(99)
    );
    for markup in ["<b>different</b>", "<pre>😀&lt;&amp;&quot;nested</pre>"] {
        assert!(matches!(
            delivery.dispatch_html("html", &identity(markup)).await,
            Err(StoreError::Conflict)
        ));
    }
    assert!(matches!(
        delivery.dispatch("html", &i).await,
        Err(StoreError::Conflict)
    ));
    assert_eq!(
        delivery.dispatch_html("wrong-sender", &i).await.unwrap(),
        TextOutcome::Unknown
    );
    assert_eq!(
        delivery.dispatch_html("wrong-sender", &i).await.unwrap(),
        TextOutcome::Unknown
    );
    server.await.unwrap();
    for markup in [
        "<b>bad</pre>",
        "<a href='x'>bad</a>",
        "&bad;",
        "<b>unclosed",
        "<b><pre>nested</pre></b>",
        "",
    ] {
        assert!(matches!(
            delivery.dispatch_html("invalid", &identity(markup)).await,
            Err(StoreError::InvalidInput)
        ));
        assert!(f.store.text_effect("invalid").await.unwrap().is_none());
    }
    f.runtime.shutdown().await.unwrap();
}

fn raw() -> Value {
    json!({"update_id":20,"message":{"message_id":7,"date":1700000000,"chat":{"id":-100,"type":"supergroup","title":"<group>"},"from":{"id":500,"is_bot":false,"first_name":"Observed"},"text":"😀 <& ordinary"}})
}

#[tokio::test]
async fn dispatcher_stats_without_downloader_preserves_trusted_tokens_and_replay() {
    let (listener, bot) = wire().await;
    let f = fixture(bot.clone()).await;
    let identities = Arc::new(yoink_identity::IdentityStore::new(
        f.store.database().clone(),
        None,
        true,
    ));
    let lifecycle = crate::TelegramHandlerLifecycle::new();
    let deps = crate::MessageDependencies {
        downloads: None,
        upload: crate::CookieUploadDependency {
            config: None,
            base: bot.api_url(),
            client: bot.client().clone(),
        },
        cookie_url: crate::CookieUrlDependency(None),
        identities: Some(identities),
        inbox: None,
        insight: None,
        music: None,
        stats: Some(f.runtime.clone()),
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
    let message: teloxide::types::Message = serde_json::from_value(json!({"message_id":17,"date":1700000000,"chat":{"id":42,"type":"private","first_name":"Ada"},"from":{"id":42,"is_bot":false,"first_name":"Ada"},"text":"/stats@fixture"})).unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut stream).await;
        assert_eq!(body["chat_id"], 42);
        assert!(body.get("message_thread_id").is_none());
        reply(&mut stream, receipt(&body, 123)).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "replay sent twice"
        );
    });
    for _ in 0..2 {
        crate::handle_message(
            bot.clone(),
            message.clone(),
            identity,
            "fixture".into(),
            system.clone(),
            deps.clone(),
        )
        .await
        .unwrap();
    }
    let effect = f
        .store
        .text_effect("stats:123:42:0:17:42:0")
        .await
        .unwrap()
        .unwrap();
    let bound = effect.identity.unwrap();
    assert_eq!(
        (
            bound.bot,
            bound.chat,
            bound.topic,
            bound.message,
            bound.actor
        ),
        (123, 42, None, 17, 42)
    );
    assert_eq!(effect.outcome, TextOutcome::Confirmed(99));
    lifecycle.drain_handlers().await;
    server.await.unwrap();
    let (listener, help_bot) = wire().await;
    let server = tokio::spawn(async move {
        for owner in [true, false] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (_, body) = request(&mut stream).await;
            let text = body["text"].as_str().unwrap();
            assert_eq!(text.contains("/status"), owner);
            assert_eq!(text.contains("/lang"), owner);
            assert!(text.contains("/stats"));
            reply(&mut stream, receipt(&body, 123)).await;
        }
    });
    let mut help_deps = deps;
    help_deps.lifecycle = crate::TelegramHandlerLifecycle::new();
    let help: teloxide::types::Message = serde_json::from_value(json!({"message_id":18,"date":1700000000,"chat":{"id":42,"type":"private","first_name":"Ada"},"from":{"id":42,"is_bot":false,"first_name":"Ada"},"text":"/help"})).unwrap();
    for role in ["owner", "user"] {
        query("UPDATE users SET role=? WHERE id=42")
            .bind(role)
            .execute(f.store.database())
            .await
            .unwrap();
        crate::handle_message(
            help_bot.clone(),
            help.clone(),
            crate::TelegramIdentity {
                owner_id: 42,
                ..identity
            },
            "fixture".into(),
            system.clone(),
            help_deps.clone(),
        )
        .await
        .unwrap();
    }
    help_deps.lifecycle.drain_handlers().await;
    server.await.unwrap();
    f.runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn original_raw_replay_failure_and_no_core_registration() {
    let (_listener, bot) = wire().await;
    let f = fixture(bot).await;
    let observe = f.runtime.observer();
    observe(raw()).await.unwrap();
    observe(raw()).await.unwrap();
    let count: i64 = query("SELECT COUNT(*) FROM stats_messages")
        .fetch_one(f.store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(count, 1);
    let count: i64 = query("SELECT COUNT(*) FROM users WHERE id=500")
        .fetch_one(f.store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(count, 0);
    let mut invalid = raw();
    invalid["message"]["date"] = "bad".into();
    assert!(observe(invalid).await.is_err());
    f.runtime.shutdown().await.unwrap();
    assert!(observe(raw()).await.is_err());
}

#[tokio::test]
async fn native_polling_replays_failed_batch_before_acknowledgment() {
    use futures::StreamExt;
    use teloxide::update_listeners::AsUpdateStream;
    let (listener, bot) = wire().await;
    let f = fixture(bot.clone()).await;
    let server = tokio::spawn(async move {
        for (offset, fail) in [(0, true), (0, false), (21, true)] {
            let (mut s, _) = listener.accept().await.unwrap();
            let (_, body) = request(&mut s).await;
            assert_eq!(body["offset"], offset);
            let mut bad = raw();
            bad["update_id"] = 21.into();
            bad["message"]["message_id"] = 0.into();
            let updates = if fail { vec![raw(), bad] } else { vec![raw()] };
            reply(&mut s, json!({"ok":true,"result":updates})).await;
        }
    });
    let mut endpoint = bot.api_url();
    endpoint.set_path("/bot123:fixture/getUpdates");
    let polling = || {
        crate::raw_polling::RawPolling::new(
            bot.client().clone(),
            endpoint.clone(),
            Duration::from_secs(2),
            123,
            None,
        )
        .with_update_observer(Some(f.runtime.observer()))
    };
    let mut first = polling();
    let mut stream = first.as_stream();
    assert!(matches!(
        stream.next().await,
        Some(Err(crate::raw_polling::PollingError::ProductCommit))
    ));
    assert!(stream.next().await.is_none());
    let mut restarted = polling();
    let mut stream = restarted.as_stream();
    assert!(stream.next().await.unwrap().is_ok());
    assert!(matches!(
        stream.next().await,
        Some(Err(crate::raw_polling::PollingError::ProductCommit))
    ));
    let count: i64 = query("SELECT COUNT(*) FROM stats_messages")
        .fetch_one(f.store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(count, 1);
    let bot_id: i64 = query("SELECT bot_id FROM stats_ingestion_watermarks LIMIT 1")
        .fetch_one(f.store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(bot_id, 123);
    server.await.unwrap();
    f.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn bounded_provider_rejects_large_and_stalled_body() {
    let (listener, bot) = wire().await;
    let provider = BotMembershipProvider::new(
        bot,
        LookupBudget {
            timeout: Duration::from_millis(40),
            response_bytes: 128,
        },
    )
    .unwrap();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        request(&mut s).await;
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 99999\r\n\r\n")
            .await
            .unwrap();
        drop(s);
        let (mut s, _) = listener.accept().await.unwrap();
        request(&mut s).await;
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    });
    assert!(matches!(
        provider.lookup(-100, 42).await,
        Err(Error::Unavailable)
    ));
    assert!(matches!(
        provider.lookup_name(None, 42).await,
        Err(Error::Unavailable)
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn missing_and_failed_name_refresh_never_erase_observed_metadata() {
    let (listener, bot) = wire().await;
    let f = fixture(bot).await;
    f.runtime.observe(raw()).await.unwrap();
    let server = tokio::spawn(async move {
        for body in [
            json!({"ok":false,"error_code":403}),
            json!({"ok":true,"result":{"id":500,"type":"private","first_name":" "}}),
            json!({"ok":false,"error_code":500}),
            json!({"ok":false,"error_code":403}),
        ] {
            let (mut s, _) = listener.accept().await.unwrap();
            request(&mut s).await;
            reply(&mut s, body).await;
        }
    });
    for _ in 0..2 {
        let report = f
            .runtime
            .stats
            .refresh_usernames(Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(
            report,
            yoink_stats::NameRefreshReport {
                refreshed: 0,
                skipped: 1
            }
        );
        let name: String =
            query("SELECT display_name FROM stats_user_latest_name WHERE user_id=500")
                .fetch_one(f.store.database())
                .await
                .unwrap()
                .try_get(0)
                .unwrap();
        assert_eq!(name, "Observed");
    }
    server.await.unwrap();
    f.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_command_html_topic_and_unknown_nonretry() {
    let (listener, bot) = wire().await;
    let f = fixture(bot).await;
    let target = CommandTarget {
        actor: 42,
        chat: -100,
        bot: 123,
        message: 7,
        topic: Some(8),
    };
    let server = tokio::spawn(async move {
        for confirm in [true, false] {
            let (mut s, _) = listener.accept().await.unwrap();
            let (_, body) = request(&mut s).await;
            assert!(
                body["text"]
                    .as_str()
                    .unwrap()
                    .contains("Run /stats in a group")
            );
            assert!(!body["text"].as_str().unwrap().contains("<pre>"));
            assert_eq!(body["entities"][0]["type"], "bold");
            assert_eq!(body["message_thread_id"], 8);
            if confirm {
                reply(&mut s, receipt(&body, 123)).await;
            }
        }
    });
    assert_eq!(
        f.runtime
            .command("command", target, &[], Language::English)
            .await
            .unwrap(),
        vec![TextOutcome::Confirmed(99)]
    );
    assert_eq!(
        f.runtime
            .command("command", target, &[], Language::English)
            .await
            .unwrap(),
        vec![TextOutcome::Confirmed(99)]
    );
    assert_eq!(
        f.runtime
            .command("lost", target, &[], Language::English)
            .await
            .unwrap(),
        vec![TextOutcome::Unknown]
    );
    assert_eq!(
        f.runtime
            .command("lost", target, &[], Language::English)
            .await
            .unwrap(),
        vec![TextOutcome::Unknown]
    );
    server.await.unwrap();
    f.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn owned_schedule_cancellation_and_drain() {
    let (listener, bot) = wire().await;
    let f = fixture(bot).await;
    f.runtime.observe(raw()).await.unwrap();
    let schedule = UsernameRefreshSchedule {
        first_delay: Duration::from_millis(30),
        interval: Duration::from_secs(3600),
        lookup_timeout: Duration::from_secs(2),
    };
    let (started, wait_started) = tokio::sync::oneshot::channel();
    let (release, wait_release) = tokio::sync::oneshot::channel();
    let started_at = tokio::time::Instant::now();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        assert!(started_at.elapsed() >= Duration::from_millis(30));
        let (_, body) = request(&mut s).await;
        assert_eq!(body["user_id"], 500);
        started.send(()).unwrap();
        wait_release.await.unwrap();
        reply(
            &mut s,
            json!({"ok":true,"result":{"user":{"id":500,"first_name":"Refreshed"}}}),
        )
        .await;
    });
    let runtime = f.runtime.clone();
    let job = tokio::spawn(async move {
        runtime
            .background(schedule, Duration::from_secs(3600))
            .await
    });
    wait_started.await.unwrap();
    f.runtime.stop_admission();
    assert!(matches!(
        f.runtime.observe(raw()).await,
        Err(RuntimeError::Closing)
    ));
    let shutdown = f.runtime.shutdown();
    tokio::pin!(shutdown);
    assert!(futures::poll!(&mut shutdown).is_pending());
    release.send(()).unwrap();
    job.await.unwrap().unwrap();
    shutdown.await.unwrap();
    server.await.unwrap();
    assert!(f.runtime.stats.refresh().await.is_err());
}

#[tokio::test]
async fn observer_and_command_work_remain_admitted_until_drain() {
    let (listener, bot) = wire().await;
    let f = fixture(bot).await;
    let connection = f.store.database().connection().await.unwrap();
    let observer = f.runtime.observe(raw());
    tokio::pin!(observer);
    assert!(futures::poll!(&mut observer).is_pending());
    f.runtime.stop_admission();
    let shutdown = f.runtime.shutdown();
    tokio::pin!(shutdown);
    assert!(futures::poll!(&mut shutdown).is_pending());
    drop(connection);
    observer.await.unwrap();
    shutdown.await.unwrap();

    // A separately owned runtime demonstrates drain across an in-flight send.
    let (_, bot) = wire().await;
    let mut bot_url = bot.api_url();
    bot_url
        .set_port(Some(listener.local_addr().unwrap().port()))
        .unwrap();
    let f = fixture(bot.set_api_url(bot_url)).await;
    let (started, wait_started) = tokio::sync::oneshot::channel();
    let (release, wait_release) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut s).await;
        started.send(()).unwrap();
        wait_release.await.unwrap();
        reply(&mut s, receipt(&body, 123)).await;
    });
    let runtime = f.runtime.clone();
    let command = tokio::spawn(async move {
        runtime
            .command(
                "drain-command",
                CommandTarget {
                    actor: 42,
                    chat: -100,
                    bot: 123,
                    message: 7,
                    topic: Some(8),
                },
                &[],
                Language::English,
            )
            .await
    });
    wait_started.await.unwrap();
    f.runtime.stop_admission();
    let shutdown = f.runtime.shutdown();
    tokio::pin!(shutdown);
    assert!(futures::poll!(&mut shutdown).is_pending());
    release.send(()).unwrap();
    assert_eq!(
        command.await.unwrap().unwrap(),
        vec![TextOutcome::Confirmed(99)]
    );
    shutdown.await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn commands_deliver_each_independent_escaped_chunk() {
    let (listener, bot) = wire().await;
    let f = fixture(bot).await;
    f.runtime.observe(raw()).await.unwrap();
    query("UPDATE users SET role='admin' WHERE id=42")
        .execute(f.store.database())
        .await
        .unwrap();
    for i in 0..60 {
        query("INSERT INTO stats_messages(chat_id,message_id,date,new_chat_title) VALUES(-100,?,1700000000000000,?)")
            .bind(1700000000000000_i64 + i)
            .bind(format!("{i}: {}", "<&😀>".repeat(20)))
            .execute(f.store.database())
            .await
            .unwrap();
    }
    let args = vec!["titles".to_owned()];
    let expected = f
        .runtime
        .stats
        .command(42, -100, &args, Language::English)
        .await
        .unwrap();
    assert!(expected.len() > 1);
    let count = expected.len();
    let server = tokio::spawn(async move {
        for markup in expected {
            let (mut s, _) = listener.accept().await.unwrap();
            let (_, body) = request(&mut s).await;
            assert!(markup.starts_with("<b>Statistics: titles</b>\n<pre>"));
            assert!(markup.ends_with("</pre>"));
            assert!(markup.contains("&lt;&amp;😀&gt;"));
            assert!(body["text"].as_str().unwrap().contains("<&😀>"));
            assert_eq!(body["message_thread_id"], 8);
            reply(&mut s, receipt(&body, 123)).await;
        }
    });
    let outcomes = f
        .runtime
        .command(
            "chunks",
            CommandTarget {
                actor: 42,
                chat: -100,
                bot: 123,
                message: 7,
                topic: Some(8),
            },
            &args,
            Language::English,
        )
        .await
        .unwrap();
    assert_eq!(outcomes, vec![TextOutcome::Confirmed(99); count]);
    server.await.unwrap();
    f.runtime.shutdown().await.unwrap();
}

fn target() -> CommandTarget {
    CommandTarget {
        actor: 42,
        chat: -100,
        bot: 123,
        message: 7,
        topic: Some(8),
    }
}
#[tokio::test]
async fn command_transaction_cancellation_rolls_back_snapshot_and_admission() {
    for phase in [TransactionPhase::Snapshot, TransactionPhase::Admission] {
        for timeout in [false, true] {
            let (listener, bot) = wire().await;
            let mut f = fixture(bot).await;
            titles(&f, 60).await;
            let pause = Arc::new(TransactionPause {
                phase,
                entered: tokio::sync::Notify::new(),
                release: tokio::sync::Semaphore::new(0),
            });
            f.runtime.transaction_pause = Some(pause.clone());
            let runtime = f.runtime.clone();
            let command = tokio::spawn(async move {
                let args = vec!["titles".to_owned()];
                let future = runtime.command("cancelled", target(), &args, Language::English);
                if timeout {
                    assert!(
                        tokio::time::timeout(Duration::from_secs(1), future)
                            .await
                            .is_err()
                    );
                } else {
                    future.await.unwrap();
                }
            });
            tokio::time::timeout(Duration::from_secs(5), pause.entered.notified())
                .await
                .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(10), f.store.database().connection())
                    .await
                    .is_err()
            );
            if timeout {
                tokio::time::timeout(Duration::from_secs(5), command)
                    .await
                    .unwrap()
                    .unwrap();
            } else {
                command.abort();
                assert!(command.await.unwrap_err().is_cancelled());
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(20), listener.accept())
                    .await
                    .is_err()
            );
            let manifest = f.store.text_effect("cancelled").await.unwrap();
            let rows = query("SELECT state FROM runtime_text_effects")
                .fetch_all(f.store.database())
                .await
                .unwrap();
            if phase == TransactionPhase::Snapshot {
                assert!(manifest.is_none());
                assert!(rows.is_empty());
            } else {
                let args = vec!["titles".to_owned()];
                let result = read_command_snapshot(
                    manifest.unwrap(),
                    &command_request(target(), &args, Language::English),
                )
                .unwrap();
                assert!(result.chunks.len() > 1);
                assert_eq!(rows.len(), result.chunks.len() + 1);
                for row in rows {
                    assert_eq!(row.try_get::<String, _>("state").unwrap(), "prepared");
                }
            }
            let identity_store =
                yoink_identity::IdentityStore::new(f.store.database().clone(), None, true);
            assert!(
                identity_store
                    .snapshot(42, Some((-100, Some(8))))
                    .await
                    .unwrap()
                    .is_some()
            );
            // Retry the same immutable command, proving a fresh transaction can commit.
            f.runtime.transaction_pause = None;
            let expected = f
                .runtime
                .stats
                .command_result(42, -100, &["titles".to_owned()], Language::English)
                .await
                .unwrap();
            let count = expected.chunks.len();
            let server = tokio::spawn(async move {
                for _ in 0..count {
                    let (mut s, _) = listener.accept().await.unwrap();
                    let (_, body) = request(&mut s).await;
                    reply(&mut s, receipt(&body, 123)).await;
                }
            });
            assert_eq!(
                f.runtime
                    .command(
                        "cancelled",
                        target(),
                        &["titles".to_owned()],
                        Language::English
                    )
                    .await
                    .unwrap()
                    .len(),
                count
            );
            server.await.unwrap();
            for part in 0..count {
                assert_eq!(
                    f.store
                        .text_effect(&format!("cancelled:{part}"))
                        .await
                        .unwrap()
                        .unwrap()
                        .outcome,
                    TextOutcome::Confirmed(99)
                );
            }
            f.runtime.stats.shutdown().await.unwrap();
        }
    }
}
#[tokio::test]
async fn command_transaction_operation_and_commit_failures_leave_no_partial_attempt() {
    for phase in [TransactionPhase::Snapshot, TransactionPhase::Admission] {
        for commit_failure in [false, true] {
            let (listener, bot) = wire().await;
            let f = fixture(bot).await;
            titles(&f, 60).await;
            {
                let c = f.store.database().connection().await.unwrap();
                c.batch("PRAGMA foreign_keys=ON; CREATE TABLE tx_parent(id INTEGER PRIMARY KEY); CREATE TABLE tx_child(parent INTEGER REFERENCES tx_parent(id) DEFERRABLE INITIALLY DEFERRED);").await.unwrap();
                let condition = if phase == TransactionPhase::Snapshot {
                    // The first chunk and manifest have already been inserted.
                    "INSERT ON runtime_text_effects WHEN NEW.token='failure:1'"
                } else {
                    "UPDATE ON runtime_text_effects WHEN NEW.state='sending'"
                };
                let statement = if commit_failure {
                    "INSERT INTO tx_child VALUES(1)"
                } else {
                    "SELECT RAISE(ABORT,'test operation failure')"
                };
                c.batch(&format!(
                    "CREATE TRIGGER tx_failure AFTER {condition} BEGIN {statement}; END;"
                ))
                .await
                .unwrap();
            }
            let args = vec!["titles".to_owned()];
            assert!(
                f.runtime
                    .command("failure", target(), &args, Language::English)
                    .await
                    .is_err()
            );
            assert!(
                tokio::time::timeout(Duration::from_millis(20), listener.accept())
                    .await
                    .is_err()
            );
            let rows = query("SELECT state FROM runtime_text_effects")
                .fetch_all(f.store.database())
                .await
                .unwrap();
            if phase == TransactionPhase::Snapshot {
                assert!(rows.is_empty());
            } else {
                assert!(rows.len() > 2);
                for row in rows {
                    assert_eq!(row.try_get::<String, _>("state").unwrap(), "prepared");
                }
            }
            assert_eq!(
                query("SELECT COUNT(*) AS n FROM tx_child")
                    .fetch_one(f.store.database())
                    .await
                    .unwrap()
                    .try_get::<i64, _>("n")
                    .unwrap(),
                0
            );
            {
                let c = f.store.database().connection().await.unwrap();
                c.batch("DROP TRIGGER tx_failure").await.unwrap();
            }
            let identity_store =
                yoink_identity::IdentityStore::new(f.store.database().clone(), None, true);
            assert!(
                identity_store
                    .snapshot(42, Some((-100, Some(8))))
                    .await
                    .unwrap()
                    .is_some()
            );
            let count = f
                .runtime
                .stats
                .command_result(42, -100, &args, Language::English)
                .await
                .unwrap()
                .chunks
                .len();
            let server = tokio::spawn(async move {
                for _ in 0..count {
                    let (mut s, _) = listener.accept().await.unwrap();
                    let (_, body) = request(&mut s).await;
                    reply(&mut s, receipt(&body, 123)).await;
                }
            });
            assert_eq!(
                f.runtime
                    .command("failure", target(), &args, Language::English)
                    .await
                    .unwrap()
                    .len(),
                count
            );
            server.await.unwrap();
            f.runtime.stats.shutdown().await.unwrap();
        }
    }
}
async fn titles(f: &Fixture, count: i64) {
    f.runtime.observe(raw()).await.unwrap();
    query("UPDATE users SET role='admin' WHERE id=42")
        .execute(f.store.database())
        .await
        .unwrap();
    let c = f.store.database().connection().await.unwrap();
    c.batch("BEGIN IMMEDIATE").await.unwrap();
    for i in 0..count {
        query("INSERT INTO stats_messages(chat_id,message_id,date,new_chat_title) VALUES(-100,?,1700000000000000,?)")
            .bind(100+i).bind(format!("{i}: {}", "<&😀>".repeat(20))).execute(&*c).await.unwrap();
    }
    c.batch("COMMIT").await.unwrap();
}

#[tokio::test]
async fn first_receipt_revocation_stops_next_chunk_and_replay_uses_whole_old_result() {
    let (listener, bot) = wire().await;
    let f = fixture(bot).await;
    titles(&f, 60).await;
    let args = vec!["titles".to_owned()];
    let (started, wait_started) = tokio::sync::oneshot::channel();
    let (release, wait_release) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut s).await;
        started.send(()).unwrap();
        wait_release.await.unwrap();
        reply(&mut s, receipt(&body, 123)).await;
        listener
    });
    let runtime = f.runtime.clone();
    let a = args.clone();
    let command = tokio::spawn(async move {
        runtime
            .command("partial", target(), &a, Language::English)
            .await
    });
    wait_started.await.unwrap();
    let manifest = f.store.text_effect("partial").await.unwrap().unwrap();
    assert_eq!(manifest.outcome, TextOutcome::Prepared);
    let result = read_command_snapshot(
        manifest,
        &command_request(target(), &args, Language::English),
    )
    .unwrap();
    assert!(result.chunks.len() > 1);
    for part in 1..result.chunks.len() {
        assert_eq!(
            f.store
                .text_effect(&format!("partial:{part}"))
                .await
                .unwrap()
                .unwrap()
                .outcome,
            TextOutcome::Prepared
        );
    }
    query("UPDATE users SET role='restricted' WHERE id=42")
        .execute(f.store.database())
        .await
        .unwrap();
    release.send(()).unwrap();
    assert!(matches!(
        command.await.unwrap(),
        Err(RuntimeError::Stats(Error::Forbidden))
    ));
    let listener = server.await.unwrap();
    assert_eq!(
        f.store
            .text_effect("partial:0")
            .await
            .unwrap()
            .unwrap()
            .outcome,
        TextOutcome::Confirmed(99)
    );
    assert_eq!(
        f.store
            .text_effect("partial:1")
            .await
            .unwrap()
            .unwrap()
            .outcome,
        TextOutcome::Prepared
    );
    query("UPDATE users SET role='admin' WHERE id=42")
        .execute(f.store.database())
        .await
        .unwrap();
    query("UPDATE stats_messages SET new_chat_title='changed corpus' WHERE chat_id=-100")
        .execute(f.store.database())
        .await
        .unwrap();
    let mut changed = raw();
    changed["update_id"] = 90.into();
    changed["message"]["message_id"] = 90.into();
    changed["message"]["date"] = 1700000100.into();
    changed["message"]["from"]["first_name"] = "New name".into();
    f.runtime.observe(changed).await.unwrap();
    let count = result.chunks.len();
    let server = tokio::spawn(async move {
        for markup in result.chunks.iter().skip(1) {
            let (mut s, _) = listener.accept().await.unwrap();
            let (_, body) = request(&mut s).await;
            let (bound, _) = TextDelivery::html_identity(&identity(markup)).unwrap();
            assert_eq!(body["text"], bound.text);
            reply(&mut s, receipt(&body, 123)).await;
        }
    });
    assert_eq!(
        f.runtime
            .command("partial", target(), &args, Language::English)
            .await
            .unwrap(),
        vec![TextOutcome::Confirmed(99); count]
    );
    server.await.unwrap();
    for (t, a, l) in [
        (target(), vec!["counts".into()], Language::English),
        (target(), args.clone(), Language::Russian),
        (
            CommandTarget {
                topic: Some(9),
                ..target()
            },
            args.clone(),
            Language::English,
        ),
        (
            CommandTarget {
                actor: 43,
                ..target()
            },
            args.clone(),
            Language::English,
        ),
    ] {
        assert!(matches!(
            f.runtime.command("partial", t, &a, l).await,
            Err(RuntimeError::Delivery(StoreError::Conflict))
        ));
    }
    f.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn private_snapshot_retains_source_assignment_authority() {
    let (_listener, bot) = wire().await;
    let f = fixture(bot).await;
    f.runtime.observe(raw()).await.unwrap();
    query("UPDATE users SET role='moderator' WHERE id=42")
        .execute(f.store.database())
        .await
        .unwrap();
    query("INSERT INTO user_group_policies(user_id,group_id) VALUES(42,-100)")
        .execute(f.store.database())
        .await
        .unwrap();
    let t = CommandTarget {
        chat: 42,
        topic: None,
        ..target()
    };
    let args = vec!["user".into(), "500".into()];
    let result = f
        .runtime
        .command_snapshot(
            "private",
            t,
            &args,
            Language::English,
            &command_request(t, &args, Language::English),
        )
        .await
        .unwrap();
    assert_eq!(result.source_group, Some(-100));
    query("DELETE FROM user_group_policies WHERE user_id=42")
        .execute(f.store.database())
        .await
        .unwrap();
    assert!(matches!(
        f.runtime
            .command("private", t, &args, Language::English)
            .await,
        Err(RuntimeError::Stats(Error::Forbidden))
    ));
    assert_eq!(
        f.store
            .text_effect("private:0")
            .await
            .unwrap()
            .unwrap()
            .outcome,
        TextOutcome::Prepared
    );
    f.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn revocation_while_waiting_for_journal_connection_prevents_admission() {
    let (listener, bot) = wire().await;
    let f = fixture(bot).await;
    titles(&f, 1).await;
    let args = vec!["counts".into()];
    f.runtime
        .command_snapshot(
            "blocked",
            target(),
            &args,
            Language::English,
            &command_request(target(), &args, Language::English),
        )
        .await
        .unwrap();
    query("UPDATE users SET role='user' WHERE id=42")
        .execute(f.store.database())
        .await
        .unwrap();
    let (started, wait_started) = tokio::sync::oneshot::channel();
    let (release, wait_release) = tokio::sync::oneshot::channel();
    let (replied, wait_replied) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (header, _) = request(&mut s).await;
        assert!(header.contains("getChatMember"));
        started.send(()).unwrap();
        wait_release.await.unwrap();
        reply(
            &mut s,
            json!({"ok":true,"result":{"status":"member","user":{"id":42}}}),
        )
        .await;
        replied.send(()).unwrap();
    });
    let runtime = f.runtime.clone();
    let command = tokio::spawn(async move {
        runtime
            .command("blocked", target(), &args, Language::English)
            .await
    });
    wait_started.await.unwrap();
    let c = f.store.database().connection().await.unwrap();
    release.send(()).unwrap();
    wait_replied.await.unwrap();
    tokio::task::yield_now().await;
    assert!(!command.is_finished());
    query("UPDATE users SET role='restricted' WHERE id=42")
        .execute(&*c)
        .await
        .unwrap();
    drop(c);
    assert!(matches!(
        command.await.unwrap(),
        Err(RuntimeError::Stats(Error::Forbidden))
    ));
    assert_eq!(
        f.store
            .text_effect("blocked:0")
            .await
            .unwrap()
            .unwrap()
            .outcome,
        TextOutcome::Prepared
    );
    server.await.unwrap();
    f.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn corpus_changing_counts_replay_returns_original_confirmation() {
    let (listener, bot) = wire().await;
    let f = fixture(bot).await;
    titles(&f, 1).await;
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut s).await;
        reply(&mut s, receipt(&body, 123)).await;
    });
    let args = vec!["counts".into()];
    let original = f
        .runtime
        .command("counts-replay", target(), &args, Language::English)
        .await
        .unwrap();
    server.await.unwrap();
    let mut changed = raw();
    changed["update_id"] = 91.into();
    changed["message"]["message_id"] = 91.into();
    f.runtime.observe(changed).await.unwrap();
    assert_eq!(
        f.runtime
            .command("counts-replay", target(), &args, Language::English)
            .await
            .unwrap(),
        original
    );
    f.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn whole_result_limit_fails_without_half_snapshot_or_admission() {
    let (_listener, bot) = wire().await;
    let f = fixture(bot).await;
    titles(&f, 700).await;
    assert!(matches!(
        f.runtime
            .command("oversized", target(), &["titles".into()], Language::English)
            .await,
        Err(RuntimeError::Delivery(StoreError::InvalidInput))
    ));
    assert!(f.store.text_effect("oversized").await.unwrap().is_none());
    assert!(f.store.text_effect("oversized:0").await.unwrap().is_none());
    f.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn delivery_owned_format_cannot_be_forged_and_legacy_prepared_plain_survives() {
    let (listener, bot) = wire().await;
    let f = fixture(bot).await;
    let html = identity("<b>x</b>");
    let (forged, _) = TextDelivery::html_identity(&html).unwrap();
    assert!(matches!(
        f.runtime.delivery.dispatch("plain-first", &forged).await,
        Err(StoreError::Conflict)
    ));
    assert!(f.store.text_effect("plain-first").await.unwrap().is_none());
    assert!(matches!(
        f.runtime
            .delivery
            .dispatch_html("forged-html", &forged)
            .await,
        Err(StoreError::Conflict)
    ));
    let server = tokio::spawn(async move {
        for entities in [true, false] {
            let (mut s, _) = listener.accept().await.unwrap();
            let (_, body) = request(&mut s).await;
            assert_eq!(body.get("entities").is_some(), entities);
            reply(&mut s, receipt(&body, 123)).await;
        }
    });
    assert_eq!(
        f.runtime
            .delivery
            .dispatch_html("html-first", &html)
            .await
            .unwrap(),
        TextOutcome::Confirmed(99)
    );
    assert!(matches!(
        f.runtime.delivery.dispatch("html-first", &forged).await,
        Err(StoreError::Conflict)
    ));
    assert!(matches!(
        f.runtime
            .delivery
            .dispatch("html-first", &identity("x"))
            .await,
        Err(StoreError::Conflict)
    ));
    let plain = identity("x");
    let c = f.store.database().connection().await.unwrap();
    c.batch("BEGIN IMMEDIATE").await.unwrap();
    Store::prepare_text_on(&*c, "legacy-plain", &plain)
        .await
        .unwrap();
    c.batch("COMMIT").await.unwrap();
    drop(c);
    assert_eq!(
        f.runtime
            .delivery
            .dispatch("legacy-plain", &plain)
            .await
            .unwrap(),
        TextOutcome::Confirmed(99)
    );
    assert!(matches!(
        f.runtime
            .delivery
            .dispatch_html("legacy-plain", &html)
            .await,
        Err(StoreError::Conflict)
    ));
    let old_forgery = TextIdentity {
        text: "x".into(),
        payload: json!({"format":"stats-html-v1","application":{},"markup":"<b>x</b>","plain":"x","entities":[{"type":"bold","offset":0,"length":1}]}),
        ..plain
    };
    assert!(matches!(
        f.runtime
            .delivery
            .dispatch("old-forgery", &old_forgery)
            .await,
        Err(StoreError::Conflict)
    ));
    server.await.unwrap();
    f.runtime.shutdown().await.unwrap();
}
