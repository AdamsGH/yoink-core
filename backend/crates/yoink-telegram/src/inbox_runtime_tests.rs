use super::*;
use serde_json::{Value, json};
use teloxide::Bot;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};
use yoink_storage::{Store as Ledger, StoreError};

struct Fixture {
    _dir: tempfile::TempDir,
    ledger: Arc<Ledger>,
    runtime: InboxRuntime,
}
async fn fixture(bot: Bot) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let ledger = Arc::new(
        Ledger::open(&dir.path().join("inbox.db"), true, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    ledger.migrate().await.unwrap();
    query("INSERT INTO users(id,role,created_at,updated_at) VALUES(42,'owner',1,1)")
        .execute(ledger.database())
        .await
        .unwrap();
    let store = Store::new(ledger.database().clone());
    let ids = Arc::new(IdentityStore::new(
        ledger.database().clone(),
        Some(42),
        true,
    ));
    let delivery = TextDelivery::new(bot, 123, ledger.clone(), Duration::from_secs(2)).unwrap();
    let runtime = InboxRuntime::new(store, ids, delivery, false);
    Fixture {
        _dir: dir,
        ledger,
        runtime,
    }
}
fn bot(address: std::net::SocketAddr) -> Bot {
    Bot::with_client(
        "123:fixture",
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
    )
    .set_api_url(url::Url::parse(&format!("http://{address}/")).unwrap())
}
async fn no_wire() -> (TcpListener, Bot) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = bot(listener.local_addr().unwrap());
    (listener, api)
}
async fn request(stream: &mut TcpStream) -> (String, Value) {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    let end = loop {
        let n = stream.read(&mut chunk).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(n) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            break n + 4;
        }
        assert!(bytes.len() < 8192);
    };
    let header = String::from_utf8_lossy(&bytes[..end]).to_string();
    let length = header
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().unwrap())
        })
        .unwrap();
    while bytes.len() < end + length {
        let n = stream.read(&mut chunk).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&chunk[..n]);
    }
    (
        header,
        serde_json::from_slice(&bytes[end..end + length]).unwrap(),
    )
}
fn receipt(body: &Value, id: i32) -> Value {
    let mut value = json!({"message_id":id,"date":0,"chat":{"id":body["chat_id"],"type":"private"},"from":{"id":123,"is_bot":true,"first_name":"Fixture"},"text":body["text"]});
    if let Some(topic) = body.get("message_thread_id") {
        value["message_thread_id"] = topic.clone();
    }
    value
}
async fn respond(stream: &mut TcpStream, value: Value) {
    let body = json!({"ok":true,"result":value}).to_string();
    stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).as_bytes()).await.unwrap();
}
fn identity(item: i64) -> TextIdentity {
    TextIdentity {
        bot: 123,
        chat: 42,
        topic: None,
        message: 10,
        actor: 42,
        item: Some(item),
        edit: false,
        payload: json!({}),
        text: "Saved".into(),
    }
}
async fn item(f: &Fixture) -> i64 {
    f.runtime
        .store
        .ingest(42, "https://example.com/a", "bot", 1)
        .await
        .unwrap()
        .item_id
}
async fn binding(f: &Fixture, item: i64) -> Option<i64> {
    query("SELECT tg_reply_message_id FROM inbox_items WHERE id=?")
        .bind(item)
        .fetch_one(f.ledger.database())
        .await
        .unwrap()
        .try_get("tg_reply_message_id")
        .unwrap()
}
async fn notification(f: &Fixture, item: i64, id: &str, state: &str, token: Option<&str>) {
    query("INSERT INTO inbox_notifications(id,user_id,item_id,completion_key,payload,chat_id,message_id,state,available_at,created_at,updated_at,send_token) VALUES(?,42,?,?,?,42,99,?,1,1,1,?)")
        .bind(id).bind(item).bind(id).bind(json!({"title":"A <title>","summary":"Summary & plain text","url":"https://example.com/a"})).bind(state).bind(token).execute(f.ledger.database()).await.unwrap();
}
async fn state(f: &Fixture, id: &str) -> String {
    f.runtime.store.notification(42, id).await.unwrap().state
}
async fn confirmed(f: &Fixture, item: i64) {
    f.ledger
        .begin_text("initial", &identity(item))
        .await
        .unwrap();
    f.ledger.confirm_text("initial", 99).await.unwrap();
    f.runtime.reconcile_bindings().await.unwrap();
}
#[tokio::test]
async fn accepted_initial_response_loss_quarantines_without_binding_or_resend() {
    let (listener, api) = no_wire().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut stream).await;
        body
    });
    let f = fixture(api).await;
    let item = item(&f).await;
    let i = identity(item);
    assert_eq!(
        f.runtime.delivery.dispatch("lost", &i).await.unwrap(),
        TextOutcome::Unknown
    );
    assert_eq!(server.await.unwrap()["reply_parameters"]["message_id"], 10);
    f.runtime.reconcile_bindings().await.unwrap();
    assert_eq!(binding(&f, item).await, None);
    assert_eq!(
        f.runtime.delivery.dispatch("lost", &i).await.unwrap(),
        TextOutcome::Unknown
    );
}
#[tokio::test]
async fn persisted_receipt_before_binding_recovers_after_actual_database_restart_and_revocation() {
    let (listener, api) = no_wire().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut stream).await;
        respond(&mut stream, receipt(&body, 99)).await;
        body
    });
    let f = fixture(api.clone()).await;
    let item = item(&f).await;
    assert_eq!(
        f.runtime
            .delivery
            .dispatch("receipt", &identity(item))
            .await
            .unwrap(),
        TextOutcome::Confirmed(99)
    );
    assert_eq!(binding(&f, item).await, None);
    query("UPDATE users SET role='banned' WHERE id=42")
        .execute(f.ledger.database())
        .await
        .unwrap();
    let path = f._dir.path().join("inbox.db");
    let dir = f._dir;
    drop(f.runtime);
    let ledger = f.ledger;
    ledger.close().await.unwrap();
    drop(ledger);
    let ledger = Arc::new(
        Ledger::open(&path, false, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    let runtime = InboxRuntime::new(
        Store::new(ledger.database().clone()),
        Arc::new(IdentityStore::new(
            ledger.database().clone(),
            Some(42),
            true,
        )),
        TextDelivery::new(api, 123, ledger.clone(), Duration::from_secs(2)).unwrap(),
        false,
    );
    runtime.reconcile_bindings().await.unwrap();
    let f = Fixture {
        _dir: dir,
        ledger,
        runtime,
    };
    assert_eq!(binding(&f, item).await, Some(99));
    let body = server.await.unwrap();
    assert!(body.get("parse_mode").is_none());
    assert_eq!(body["link_preview_options"]["is_disabled"], true);
}
#[tokio::test]
async fn classification_before_binding_wakes_waiting_notification() {
    let (_listener, api) = no_wire().await;
    let f = fixture(api).await;
    let item = item(&f).await;
    query("INSERT INTO inbox_notifications(id,user_id,item_id,completion_key,payload,state,available_at,created_at,updated_at) VALUES('waiting',42,?,'waiting','{}','waiting',1,1,1)").bind(item).execute(f.ledger.database()).await.unwrap();
    confirmed(&f, item).await;
    assert_eq!(state(&f, "waiting").await, "ready");
    assert_eq!(
        f.runtime
            .store
            .notification(42, "waiting")
            .await
            .unwrap()
            .message_id,
        Some(99)
    );
}
#[tokio::test]
async fn conflicting_confirmed_bindings_do_not_retarget_item() {
    let (_listener, api) = no_wire().await;
    let f = fixture(api).await;
    let item = item(&f).await;
    confirmed(&f, item).await;
    let other = TextIdentity {
        message: 11,
        ..identity(item)
    };
    f.ledger.begin_text("other", &other).await.unwrap();
    f.ledger.confirm_text("other", 100).await.unwrap();
    f.runtime.reconcile_bindings().await.unwrap();
    assert_eq!(binding(&f, item).await, Some(99));
}
#[tokio::test]
async fn actual_competing_text_dispatchers_make_one_bot_api_attempt() {
    let (listener, api) = no_wire().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut stream).await;
        respond(&mut stream, receipt(&body, 99)).await;
    });
    let f = fixture(api).await;
    let i = identity(item(&f).await);
    let (a, b) = tokio::join!(
        f.runtime.delivery.dispatch("competing", &i),
        f.runtime.delivery.dispatch("competing", &i)
    );
    let outcomes = [a.unwrap(), b.unwrap()];
    assert!(outcomes.contains(&TextOutcome::Confirmed(99)));
    server.await.unwrap();
    assert_eq!(
        f.ledger
            .text_effect("competing")
            .await
            .unwrap()
            .unwrap()
            .outcome,
        TextOutcome::Confirmed(99)
    );
}
#[tokio::test]
async fn sealed_no_attempt_completion_recovers_and_late_send_is_forbidden() {
    let (_listener, api) = no_wire().await;
    let f = fixture(api).await;
    let item = item(&f).await;
    confirmed(&f, item).await;
    notification(&f, item, "sealed", "sending", Some("exact-token")).await;
    let e = f.ledger.seal_text_absence("exact-token").await.unwrap();
    assert_eq!(e.outcome, TextOutcome::Sealed);
    let i = TextIdentity {
        edit: true,
        message: 99,
        ..identity(item)
    };
    assert_eq!(
        f.runtime
            .delivery
            .dispatch("exact-token", &i)
            .await
            .unwrap(),
        TextOutcome::Sealed
    );
    // Reconciliation records not-sent without authority, then revoke before any new admission.
    query("UPDATE users SET role='banned' WHERE id=42")
        .execute(f.ledger.database())
        .await
        .unwrap();
    f.runtime.tick().await.unwrap();
    assert_eq!(state(&f, "sealed").await, "cancelled");
    assert_eq!(
        f.runtime
            .store
            .notification(42, "sealed")
            .await
            .unwrap()
            .send_token
            .as_deref(),
        Some("exact-token")
    );
}
#[tokio::test]
async fn accepted_edit_response_loss_blocks_later_edit_ordering_without_retries() {
    let (listener, api) = no_wire().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (header, body) = request(&mut stream).await;
        assert!(header.contains("/editMessageText "));
        body
    });
    let f = fixture(api).await;
    let item = item(&f).await;
    confirmed(&f, item).await;
    notification(&f, item, "first", "ready", None).await;
    notification(&f, item, "later", "ready", None).await;
    f.runtime.tick().await.unwrap();
    let body = server.await.unwrap();
    assert_eq!(body["message_id"], 99);
    assert!(body.get("parse_mode").is_none());
    assert_eq!(state(&f, "first").await, "unknown");
    assert_eq!(state(&f, "later").await, "ready");
    f.runtime.tick().await.unwrap();
    assert_eq!(state(&f, "later").await, "ready");
    let n = f.runtime.store.notification(42, "first").await.unwrap();
    assert_eq!(
        f.ledger
            .text_effect(n.send_token.as_deref().unwrap())
            .await
            .unwrap()
            .unwrap()
            .outcome,
        TextOutcome::Unknown
    );
}
#[tokio::test]
async fn confirmed_edit_reconciles_without_fresh_grants_or_repeating_send() {
    let (_listener, api) = no_wire().await;
    let f = fixture(api).await;
    let item = item(&f).await;
    confirmed(&f, item).await;
    notification(&f, item, "confirmed", "sending", Some("unchanged-token")).await;
    let i = TextIdentity {
        edit: true,
        message: 99,
        payload: json!({"title":"A <title>","summary":"Summary & plain text","url":"https://example.com/a"}),
        text: "Snapshot".into(),
        ..identity(item)
    };
    f.ledger.begin_text("unchanged-token", &i).await.unwrap();
    f.ledger.confirm_text("unchanged-token", 99).await.unwrap();
    query("UPDATE users SET role='banned' WHERE id=42")
        .execute(f.ledger.database())
        .await
        .unwrap();
    f.runtime.tick().await.unwrap();
    assert_eq!(state(&f, "confirmed").await, "acked");
}
#[tokio::test]
async fn revoke_route_mismatch_and_bot_provenance_fail_before_send() {
    let (_listener, api) = no_wire().await;
    let f = fixture(api).await;
    let i = identity(item(&f).await);
    f.runtime.authorize(&i, "ingest").await.unwrap();
    for changed in [
        TextIdentity {
            chat: 43,
            ..i.clone()
        },
        TextIdentity {
            bot: 124,
            ..i.clone()
        },
        TextIdentity {
            topic: Some(5),
            ..i.clone()
        },
        TextIdentity {
            chat: -100,
            ..i.clone()
        },
    ] {
        assert!(matches!(
            f.runtime.authorize(&changed, "ingest").await,
            Err(Error::Forbidden)
        ));
    }
    query("UPDATE users SET role='banned' WHERE id=42")
        .execute(f.ledger.database())
        .await
        .unwrap();
    assert!(matches!(
        f.runtime.authorize(&i, "ingest").await,
        Err(Error::Forbidden)
    ));
    assert!(f.ledger.text_effect("no-attempt").await.unwrap().is_none());
}
#[tokio::test]
async fn shutdown_drains_admitted_edit_and_stops_new_admission() {
    let (listener, api) = no_wire().await;
    let (accepted_tx, accepted_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut stream).await;
        accepted_tx.send(()).unwrap();
        release_rx.await.unwrap();
        respond(&mut stream, receipt(&body, 99)).await;
    });
    let f = fixture(api).await;
    let item = item(&f).await;
    confirmed(&f, item).await;
    notification(&f, item, "admitted", "ready", None).await;
    notification(&f, item, "unadmitted", "ready", None).await;
    let stop = CancellationToken::new();
    let owned = f
        .runtime
        .clone()
        .with_config(InboxRuntimeConfig {
            dispatch_batch: 2,
            ..Default::default()
        })
        .unwrap();
    let token = stop.clone();
    let task = tokio::spawn(async move { owned.run(token, Duration::from_millis(1)).await });
    accepted_rx.await.unwrap();
    stop.cancel();
    release_tx.send(()).unwrap();
    task.await.unwrap().unwrap();
    server.await.unwrap();
    assert_eq!(state(&f, "admitted").await, "acked");
    assert_eq!(state(&f, "unadmitted").await, "ready");
}
#[tokio::test]
async fn actual_save_inbox_stars_and_stars_sync_dispatch_are_plain_and_localized() {
    let (listener, api) = no_wire().await;
    let server = tokio::spawn(async move {
        let mut bodies = Vec::new();
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (_, body) = request(&mut stream).await;
            respond(&mut stream, receipt(&body, 99 + bodies.len() as i32)).await;
            bodies.push(body);
        }
        bodies
    });
    let f = fixture(api).await;
    query("UPDATE users SET language='ru' WHERE id=42")
        .execute(f.ledger.database())
        .await
        .unwrap();
    for (index, text) in [
        "/save https://example.com/a",
        "/inbox",
        "/stars",
        "/stars_sync",
    ]
    .iter()
    .enumerate()
    {
        let message:Message=serde_json::from_value(json!({"message_id":10+index,"date":0,"chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"first_name":"Actor"},"text":text})).unwrap();
        assert!(f.runtime.handle(&message, "fixture").await);
    }
    let bodies = server.await.unwrap();
    query("UPDATE inbox_jobs SET state='done' WHERE kind='stars_sync'")
        .execute(f.ledger.database())
        .await
        .unwrap();
    for (index, text) in [
        "/save https://example.com/a",
        "/inbox",
        "/stars",
        "/stars_sync",
    ]
    .iter()
    .enumerate()
    {
        let message:Message=serde_json::from_value(json!({"message_id":10+index,"date":0,"chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"first_name":"Actor"},"text":text})).unwrap();
        assert!(f.runtime.handle(&message, "fixture").await);
    }
    let jobs: i64 = query("SELECT count(*) AS n FROM inbox_jobs WHERE kind='stars_sync'")
        .fetch_one(f.ledger.database())
        .await
        .unwrap()
        .try_get("n")
        .unwrap();
    assert_eq!(jobs, 1);
    assert!(bodies[0]["text"].as_str().unwrap().contains("Сохранено"));
    assert!(
        bodies[1]["text"]
            .as_str()
            .unwrap()
            .contains("https://example.com/a")
    );
    assert!(bodies[2]["text"].as_str().unwrap().contains("Нет"));
    assert!(
        bodies[3]["text"]
            .as_str()
            .unwrap()
            .contains("Синхронизация")
    );
    for body in bodies {
        assert!(body.get("parse_mode").is_none());
        assert_eq!(body["link_preview_options"]["is_disabled"], true);
    }
}
#[tokio::test]
async fn trusted_receipt_validation_rejects_wrong_sender_topic_address_and_payload() {
    for change in ["sender", "topic", "chat", "message", "payload"] {
        let (listener, api) = no_wire().await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (_, body) = request(&mut stream).await;
            let mut value = receipt(&body, 99);
            match change {
                "sender" => value["from"]["id"] = 124.into(),
                "topic" => value["message_thread_id"] = 5.into(),
                "chat" => value["chat"]["id"] = 43.into(),
                "message" => value["message_id"] = 0.into(),
                _ => value["text"] = "different".into(),
            }
            respond(&mut stream, value).await;
        });
        let f = fixture(api).await;
        let i = identity(item(&f).await);
        assert_eq!(
            f.runtime.delivery.dispatch("bad", &i).await.unwrap(),
            TextOutcome::Unknown
        );
        server.await.unwrap();
        f.runtime.reconcile_bindings().await.unwrap();
        assert_eq!(binding(&f, i.item.unwrap()).await, None);
    }
}
#[tokio::test]
async fn bounded_config_and_response_are_enforced() {
    let (listener, api) = no_wire().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 65537\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    let f = fixture(api).await;
    assert!(
        f.runtime
            .clone()
            .with_config(InboxRuntimeConfig {
                notification_lease: Duration::from_secs(2),
                ..Default::default()
            })
            .is_err()
    );
    assert!(
        f.runtime
            .clone()
            .with_config(InboxRuntimeConfig {
                recovery_batch: 0,
                ..Default::default()
            })
            .is_err()
    );
    assert_eq!(
        f.runtime
            .delivery
            .dispatch("oversize", &identity(item(&f).await))
            .await
            .unwrap(),
        TextOutcome::Unknown
    );
    server.await.unwrap();
    assert_eq!(
        f.ledger
            .begin_text(
                "oversize",
                &TextIdentity {
                    text: "conflict".into(),
                    ..identity(1)
                }
            )
            .await,
        Err(StoreError::Conflict)
    );
}

#[tokio::test]
async fn group_membership_and_topic_authority_are_refreshed_at_dispatch() {
    let (_listener, api) = no_wire().await;
    let mut f = fixture(api).await;
    let item = item(&f).await;
    f.runtime.groups = true;
    query("INSERT INTO groups(id,enabled,auto_grant_role,created_at) VALUES(-100,1,'user',1)")
        .execute(f.ledger.database())
        .await
        .unwrap();
    query("INSERT INTO thread_policies(group_id,thread_id,enabled) VALUES(-100,8,1),(-100,9,0)")
        .execute(f.ledger.database())
        .await
        .unwrap();
    let allowed = TextIdentity {
        chat: -100,
        topic: Some(8),
        ..identity(item)
    };
    f.runtime.authorize(&allowed, "ingest").await.unwrap();
    assert!(matches!(
        f.runtime
            .authorize(
                &TextIdentity {
                    topic: Some(9),
                    ..allowed.clone()
                },
                "ingest"
            )
            .await,
        Err(Error::Forbidden)
    ));
    query("INSERT INTO membership_users(source_id,group_id,user_id,occurred_at,update_id,payload,present,generation,tag_generation) VALUES(123,-100,42,1,1,'{}',0,0,0)").execute(f.ledger.database()).await.unwrap();
    assert!(matches!(
        f.runtime.authorize(&allowed, "ingest").await,
        Err(Error::Forbidden)
    ));
    query("UPDATE membership_users SET present=1 WHERE source_id=123")
        .execute(f.ledger.database())
        .await
        .unwrap();
    f.runtime.authorize(&allowed, "ingest").await.unwrap();
    query("UPDATE groups SET enabled=0 WHERE id=-100")
        .execute(f.ledger.database())
        .await
        .unwrap();
    assert!(matches!(
        f.runtime.authorize(&allowed, "ingest").await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn command_usage_duplicates_and_replied_hidden_urls_preserve_source_behavior() {
    let (listener, api) = no_wire().await;
    let server = tokio::spawn(async move {
        let mut bodies = Vec::new();
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (_, body) = request(&mut stream).await;
            respond(&mut stream, receipt(&body, 99 + bodies.len() as i32)).await;
            bodies.push(body);
        }
        bodies
    });
    let f = fixture(api).await;
    let base = json!({"message_id":10,"date":0,"chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"first_name":"Actor"},"text":"/save"});
    let message = serde_json::from_value(base.clone()).unwrap();
    assert!(f.runtime.handle(&message, "fixture").await);
    let mut hidden = base.clone();
    hidden["message_id"] = 11.into();
    hidden["reply_to_message"] = json!({"message_id":8,"date":0,"chat":{"id":42,"type":"private"},"text":"link","entities":[{"type":"text_link","offset":0,"length":4,"url":"https://example.com/hidden"}]});
    let message = serde_json::from_value(hidden).unwrap();
    assert!(f.runtime.handle(&message, "fixture").await);
    let mut duplicate = base;
    duplicate["message_id"] = 12.into();
    duplicate["text"] = "/save https://example.com/hidden".into();
    let message = serde_json::from_value(duplicate).unwrap();
    assert!(f.runtime.handle(&message, "fixture").await);
    let bodies = server.await.unwrap();
    assert!(bodies[0]["text"].as_str().unwrap().contains("/save <url>"));
    assert!(bodies[1]["text"].as_str().unwrap().contains("Saved"));
    assert!(bodies[2]["text"].as_str().unwrap().contains("already"));
    let item: i64 =
        query("SELECT id FROM inbox_items WHERE normalized_url='https://example.com/hidden'")
            .fetch_one(f.ledger.database())
            .await
            .unwrap()
            .try_get("id")
            .unwrap();
    assert_eq!(binding(&f, item).await, Some(100));
}
#[tokio::test]
async fn actual_competing_notification_dispatchers_preserve_the_exact_send_token() {
    let (listener, api) = no_wire().await;
    let (accepted_tx, accepted_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut stream).await;
        accepted_tx.send(()).unwrap();
        release_rx.await.unwrap();
        respond(&mut stream, receipt(&body, 99)).await;
    });
    let f = fixture(api).await;
    let item = item(&f).await;
    confirmed(&f, item).await;
    notification(&f, item, "competition", "ready", None).await;
    let owned = f.runtime.clone();
    let a = tokio::spawn(async move { owned.tick().await });
    accepted_rx.await.unwrap();
    f.runtime.tick().await.unwrap();
    release_tx.send(()).unwrap();
    a.await.unwrap().unwrap();
    server.await.unwrap();
    f.runtime.tick().await.unwrap();
    assert_eq!(state(&f, "competition").await, "acked");
    let n = f
        .runtime
        .store
        .notification(42, "competition")
        .await
        .unwrap();
    let token = n.send_token.unwrap();
    let effect = f.ledger.text_effect(&token).await.unwrap().unwrap();
    assert_eq!(effect.token, token);
    assert_eq!(effect.outcome, TextOutcome::Confirmed(99));
    assert_eq!(effect.identity.unwrap().payload, n.payload);
}

#[tokio::test]
async fn private_denial_feedback_does_not_ingest_or_bind_and_is_durable() {
    let (listener, api) = no_wire().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut stream).await;
        respond(&mut stream, receipt(&body, 99)).await;
        body
    });
    let f = fixture(api).await;
    query("UPDATE users SET role='banned' WHERE id=42")
        .execute(f.ledger.database())
        .await
        .unwrap();
    let message:Message=serde_json::from_value(json!({"message_id":10,"date":0,"chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"first_name":"Actor"},"text":"/save https://example.com/a"})).unwrap();
    assert!(f.runtime.handle(&message, "fixture").await);
    let body = server.await.unwrap();
    assert_eq!(
        body["text"],
        translate(Language::English, Key::ErrorDenied, &[])
    );
    assert!(f.runtime.handle(&message, "fixture").await);
    let count: i64 = query("SELECT count(*) AS n FROM inbox_items")
        .fetch_one(f.ledger.database())
        .await
        .unwrap()
        .try_get("n")
        .unwrap();
    assert_eq!(count, 0);
    let e = f
        .ledger
        .text_effect("inbox-denial:123:42:10")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e.identity.unwrap().item, None);
}

#[tokio::test]
async fn runtime_revocation_cancels_fenced_work_stays_live_and_delivers_next_eligible_notification()
{
    let (listener, api) = no_wire().await;
    let (accepted_tx, accepted_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut stream).await;
        assert_eq!(body["chat_id"], 43);
        assert_eq!(body["message_id"], 100);
        respond(&mut stream, receipt(&body, 100)).await;
        accepted_tx.send(()).unwrap();
    });
    let f = fixture(api).await;
    let first = item(&f).await;
    confirmed(&f, first).await;
    notification(&f, first, "revoked", "ready", None).await;
    query("INSERT INTO users(id,role,created_at,updated_at) VALUES(43,'user',1,1)")
        .execute(f.ledger.database())
        .await
        .unwrap();
    let next = f
        .runtime
        .store
        .ingest(43, "https://example.com/eligible", "bot", 1)
        .await
        .unwrap()
        .item_id;
    let initial = TextIdentity {
        actor: 43,
        chat: 43,
        item: Some(next),
        ..identity(next)
    };
    f.ledger.begin_text("next-initial", &initial).await.unwrap();
    f.ledger.confirm_text("next-initial", 100).await.unwrap();
    f.runtime.reconcile_bindings().await.unwrap();
    query("INSERT INTO inbox_notifications(id,user_id,item_id,completion_key,payload,chat_id,message_id,state,available_at,created_at,updated_at) VALUES('eligible',43,?,'eligible','{}',43,100,'ready',1,1,1)").bind(next).execute(f.ledger.database()).await.unwrap();
    query("UPDATE users SET role='banned' WHERE id=42")
        .execute(f.ledger.database())
        .await
        .unwrap();
    let stop = CancellationToken::new();
    let token = stop.clone();
    let owned = f.runtime.clone();
    let task = tokio::spawn(async move { owned.run(token, Duration::from_millis(1)).await });
    tokio::time::timeout(Duration::from_secs(5), accepted_rx)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if f.runtime
                .store
                .notification(43, "eligible")
                .await
                .unwrap()
                .state
                == "acked"
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(state(&f, "revoked").await, "cancelled");
    assert!(
        f.runtime
            .store
            .notification(42, "revoked")
            .await
            .unwrap()
            .send_token
            .is_none()
    );
    assert!(!task.is_finished());
    stop.cancel();
    task.await.unwrap().unwrap();
    server.await.unwrap();
}
#[tokio::test]
async fn stale_cancellation_cannot_cross_the_send_boundary_or_cancel_a_new_claim() {
    let (_listener, api) = no_wire().await;
    let f = fixture(api).await;
    let item = item(&f).await;
    confirmed(&f, item).await;
    notification(&f, item, "fenced", "ready", None).await;
    let now = chrono::Utc::now().timestamp_micros();
    let stale = f
        .runtime
        .store
        .claim_notification(now, 1)
        .await
        .unwrap()
        .unwrap();
    let current = f
        .runtime
        .store
        .claim_notification(now + 2, 1_000_000)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !f.runtime
            .store
            .cancel_notification_claim(&stale, now + 3)
            .await
            .unwrap()
    );
    assert_eq!(state(&f, "fenced").await, "claimed");
    query("UPDATE inbox_notifications SET state='sending',send_token='fence' WHERE id='fenced'")
        .execute(f.ledger.database())
        .await
        .unwrap();
    assert!(
        !f.runtime
            .store
            .cancel_notification_claim(&current, now + 3)
            .await
            .unwrap()
    );
    assert_eq!(state(&f, "fenced").await, "sending");
}

#[tokio::test]
async fn runtime_database_failure_remains_actionable_not_a_silent_retry() {
    let (_listener, api) = no_wire().await;
    let f = fixture(api).await;
    query("DROP TABLE runtime_text_effects")
        .execute(f.ledger.database())
        .await
        .unwrap();
    let result = f
        .runtime
        .run(CancellationToken::new(), Duration::from_millis(1))
        .await;
    assert!(matches!(result, Err(Error::External)));
}

fn save_message(id: i32) -> Message {
    serde_json::from_value(json!({"message_id":id,"date":0,"chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"first_name":"Actor"},"text":"/save https://example.com/a"})).unwrap()
}
async fn one_item_and_job(f: &Fixture, jobs: i64) -> i64 {
    let row = query("SELECT count(*) AS n,min(id) AS id FROM inbox_items")
        .fetch_one(f.ledger.database())
        .await
        .unwrap();
    assert_eq!(row.try_get::<i64, _>("n").unwrap(), 1);
    assert_eq!(
        query("SELECT count(*) AS n FROM inbox_jobs")
            .fetch_one(f.ledger.database())
            .await
            .unwrap()
            .try_get::<i64, _>("n")
            .unwrap(),
        jobs
    );
    row.try_get("id").unwrap()
}
#[tokio::test]
async fn save_prepared_commit_crash_restart_retains_creator_response_and_distinct_duplicate_never_binds()
 {
    let (listener, api) = no_wire().await;
    let mut f = fixture(api.clone()).await;
    let (entered_tx, entered_rx) = oneshot::channel();
    let (_resume_tx, resume_rx) = oneshot::channel();
    *Arc::get_mut(&mut f.runtime.hooks)
        .unwrap()
        .after_preparation
        .get_mut() = Some((entered_tx, resume_rx));
    let owned = f.runtime.clone();
    let task = tokio::spawn(async move { owned.handle(&save_message(10), "fixture").await });
    tokio::time::timeout(Duration::from_secs(5), entered_rx)
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
    let item = one_item_and_job(&f, 1).await;
    let effect = f
        .ledger
        .text_effect("inbox-command:123:42:0:10")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(effect.outcome, TextOutcome::Prepared);
    let original = effect.identity.unwrap();
    assert_eq!(original.item, Some(item));
    assert_eq!(original.payload["bind_item"], true);
    assert_eq!(original.payload["save_result"]["created"], true);
    assert_eq!(
        original.text,
        translate(Language::English, Key::InboxSaved, &[])
    );
    let waiting = classify_item(&f, item).await;
    assert_eq!(state(&f, &waiting).await, "waiting");
    // Full database close/reopen, not just a new runtime clone.
    let path = f._dir.path().join("inbox.db");
    let dir = f._dir;
    drop(f.runtime);
    f.ledger.close().await.unwrap();
    drop(f.ledger);
    let ledger = Arc::new(
        Ledger::open(&path, false, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    let runtime = InboxRuntime::new(
        Store::new(ledger.database().clone()),
        Arc::new(IdentityStore::new(
            ledger.database().clone(),
            Some(42),
            true,
        )),
        TextDelivery::new(api, 123, ledger.clone(), Duration::from_secs(2)).unwrap(),
        false,
    );
    let f = Fixture {
        _dir: dir,
        ledger,
        runtime,
    };
    query("UPDATE users SET language='ru' WHERE id=42")
        .execute(f.ledger.database())
        .await
        .unwrap();
    let server = tokio::spawn(async move {
        for id in [99, 100] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (_, body) = request(&mut stream).await;
            if id == 99 {
                assert_eq!(body["text"], original.text);
            } else {
                assert_eq!(body["reply_parameters"]["message_id"], 11);
            }
            respond(&mut stream, receipt(&body, id)).await;
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    });
    assert!(f.runtime.handle(&save_message(10), "fixture").await);
    assert_eq!(binding(&f, item).await, Some(99));
    assert_eq!(state(&f, &waiting).await, "ready");
    assert!(f.runtime.handle(&save_message(11), "fixture").await);
    let duplicate = f
        .ledger
        .text_effect("inbox-command:123:42:0:11")
        .await
        .unwrap()
        .unwrap()
        .identity
        .unwrap();
    assert_eq!(duplicate.payload["save_result"]["created"], false);
    assert_eq!(duplicate.payload["bind_item"], false);
    assert_eq!(binding(&f, item).await, Some(99));
    assert_eq!(one_item_and_job(&f, 2).await, item);
    server.await.unwrap();
}
#[tokio::test]
async fn concurrent_same_save_prepares_one_creator_result_and_one_external_attempt() {
    let (listener, api) = no_wire().await;
    let mut f = fixture(api).await;
    let (entered_tx, entered_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    let hooks = Arc::get_mut(&mut f.runtime.hooks).unwrap();
    hooks.before_preparation = Some(Arc::new(tokio::sync::Barrier::new(2)));
    *hooks.after_preparation.get_mut() = Some((entered_tx, resume_rx));
    let (sent_tx, sent_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut stream).await;
        assert_eq!(
            body["text"],
            translate(Language::English, Key::InboxSaved, &[])
        );
        respond(&mut stream, receipt(&body, 99)).await;
        sent_tx.send(()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
    });
    let left = f.runtime.clone();
    let right = f.runtime.clone();
    let a = tokio::spawn(async move { left.handle(&save_message(10), "fixture").await });
    let b = tokio::spawn(async move { right.handle(&save_message(10), "fixture").await });
    tokio::time::timeout(Duration::from_secs(5), entered_rx)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), sent_rx)
        .await
        .unwrap()
        .unwrap();
    resume_tx.send(()).unwrap();
    assert!(a.await.unwrap());
    assert!(b.await.unwrap());
    server.await.unwrap();
    let item = one_item_and_job(&f, 1).await;
    assert_eq!(binding(&f, item).await, Some(99));
    let effect = f
        .ledger
        .text_effect("inbox-command:123:42:0:10")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(effect.outcome, TextOutcome::Confirmed(99));
    assert_eq!(
        effect.identity.unwrap().payload["save_result"]["created"],
        true
    );
    let notification = classify_item(&f, item).await;
    assert_eq!(state(&f, &notification).await, "ready");
}
#[tokio::test]
async fn failure_to_prepare_response_rolls_back_ingestion_and_jobs() {
    let (_listener, api) = no_wire().await;
    let f = fixture(api).await;
    let mut i = identity(1);
    i.payload = json!({"incoming":"x".repeat(65537)});
    assert!(
        f.runtime
            .prepare_save(
                "invalid-preparation",
                i,
                "https://example.com/a",
                Language::English
            )
            .await
            .is_err()
    );
    for table in ["inbox_items", "inbox_jobs", "runtime_text_effects"] {
        assert_eq!(
            query(&format!("SELECT count(*) AS n FROM {table}"))
                .fetch_one(f.ledger.database())
                .await
                .unwrap()
                .try_get::<i64, _>("n")
                .unwrap(),
            0
        );
    }
}

#[tokio::test]
async fn post_admission_revocation_with_advancing_clock_seals_and_loop_continues() {
    let (listener, api) = no_wire().await;
    let (accepted_tx, accepted_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut stream).await;
        assert_eq!(body["chat_id"], 43);
        assert_eq!(body["message_id"], 100);
        respond(&mut stream, receipt(&body, 100)).await;
        accepted_tx.send(()).unwrap();
    });
    let mut f = fixture(api).await;
    let (entered_tx, entered_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    let hooks = Arc::get_mut(&mut f.runtime.hooks).unwrap();
    hooks.clock = Some(Arc::new(std::sync::atomic::AtomicI64::new(
        chrono::Utc::now().timestamp_micros(),
    )));
    *hooks.after_admission.get_mut() = Some((entered_tx, resume_rx));
    let first = item(&f).await;
    confirmed(&f, first).await;
    notification(&f, first, "revoked", "ready", None).await;
    query("INSERT INTO users(id,role,created_at,updated_at) VALUES(43,'user',1,1)")
        .execute(f.ledger.database())
        .await
        .unwrap();
    let next = f
        .runtime
        .store
        .ingest(43, "https://example.com/eligible", "bot", 1)
        .await
        .unwrap()
        .item_id;
    let initial = TextIdentity {
        actor: 43,
        chat: 43,
        item: Some(next),
        ..identity(next)
    };
    f.ledger.begin_text("next-initial", &initial).await.unwrap();
    f.ledger.confirm_text("next-initial", 100).await.unwrap();
    f.runtime.reconcile_bindings().await.unwrap();
    query("INSERT INTO inbox_notifications(id,user_id,item_id,completion_key,payload,chat_id,message_id,state,available_at,created_at,updated_at) VALUES('eligible',43,?,'eligible','{}',43,100,'ready',1,1,1)").bind(next).execute(f.ledger.database()).await.unwrap();

    let stop = CancellationToken::new();
    let token = stop.clone();
    let owned = f.runtime.clone();
    let task = tokio::spawn(async move { owned.run(token, Duration::from_millis(1)).await });
    tokio::time::timeout(Duration::from_secs(5), entered_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state(&f, "revoked").await, "sending");
    query("UPDATE users SET role='banned' WHERE id=42")
        .execute(f.ledger.database())
        .await
        .unwrap();
    resume_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), accepted_rx)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if f.runtime
                .store
                .notification(43, "eligible")
                .await
                .unwrap()
                .state
                == "acked"
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(state(&f, "revoked").await, "cancelled");
    let token = f
        .runtime
        .store
        .notification(42, "revoked")
        .await
        .unwrap()
        .send_token
        .unwrap();
    assert_eq!(
        f.ledger.text_effect(&token).await.unwrap().unwrap().outcome,
        TextOutcome::Sealed
    );
    assert!(!task.is_finished());
    stop.cancel();
    task.await.unwrap().unwrap();
    server.await.unwrap();
}

async fn classify_item(f: &Fixture, item: i64) -> String {
    let now = chrono::Utc::now().timestamp_micros() + 1000;
    let job = f
        .runtime
        .store
        .claim(now, 1_000_000, "enrich-test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.item_id, Some(item));
    assert_eq!(job.kind, "enrich");
    assert!(
        f.runtime
            .store
            .apply_content(
                &job,
                &yoink_inbox::contracts::Content {
                    title: Some("Card".into()),
                    content_text: Some("body".into()),
                    ..Default::default()
                },
                now
            )
            .await
            .unwrap()
    );
    let job = f
        .runtime
        .store
        .claim(now + 1, 1_000_000, "classify-test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.kind, "classify");
    assert!(
        f.runtime
            .store
            .apply_classification(
                &job,
                &yoink_inbox::pipeline::parse(r#"{"summary":"classified"}"#),
                now + 2
            )
            .await
            .unwrap()
    );
    query("SELECT id FROM inbox_notifications WHERE item_id=?")
        .bind(item)
        .fetch_one(f.ledger.database())
        .await
        .unwrap()
        .try_get("id")
        .unwrap()
}
