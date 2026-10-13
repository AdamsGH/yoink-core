use super::*;
#[path = "../../yoink-insight/tests/support/mod.rs"]
mod support;
#[test]
fn dispatch_names_and_suffixes() {
    for c in ["about", "summary", "tldr", "cancel"] {
        assert_eq!(
            command(&format!("/{c}@OurBot https://example.com"), "ourbot"),
            Some(c)
        );
    }
    assert_eq!(command("/tldr@OtherBot x", "ourbot"), None);
    assert_eq!(command("/save x", "ourbot"), None);
}
#[test]
fn chunks_preserve_astral_boundaries() {
    let text = format!("{}😀{}", "a".repeat(3999), "😀".repeat(4000));
    let chunks = yoink_insight::telegram::messages(&text);
    assert_eq!(chunks.concat(), text);
    assert!(chunks.iter().all(|c| c.encode_utf16().count() <= 4000));
    assert_eq!(chunks[0].len(), 3999);
}

use serde_json::{Value, json};
use teloxide::Bot;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};
use yoink_db::query;
use yoink_insight::{
    Config, FutureResult,
    content::{Citation, Content, ContentSource, FetchOptions},
    provider::DeltaSink,
};
use yoink_storage::{Store, TextOutcome};

// Deterministic material input only; provider and Telegram use their actual HTTP
// clients against private loopback fixtures. No dummy provider/executor installed.
struct Material(Option<yoink_db::Database>);
impl ContentSource for Material {
    fn fetch<'a>(
        &'a self,
        _: &'a str,
        _: FetchOptions<'a>,
        _: CancellationToken,
    ) -> FutureResult<'a, Content> {
        Box::pin(async move {
            if let Some(db) = &self.0 {
                let connection = db.connection().await?;
                tokio::time::sleep(Duration::from_secs(2)).await;
                drop(connection);
            }
            Ok(Content {
                text: "Fixture source material.".into(),
                title: None,
                video_id: None,
                video_seconds: None,
                via: "fixture".into(),
            })
        })
    }
    fn research<'a>(&'a self, _: &'a str, _: CancellationToken) -> FutureResult<'a, Vec<Citation>> {
        Box::pin(async { Ok(vec![]) })
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    runtime: InsightRuntime,
    ledger: Arc<Store>,
}
async fn fixture(api: Bot, gateway: String) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let ledger = Arc::new(
        Store::open(&dir.path().join("test.db"), true, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    ledger.migrate().await.unwrap();
    query("INSERT INTO users(id,role,created_at,updated_at) VALUES(42,'restricted',1,1)")
        .execute(ledger.database())
        .await
        .unwrap();
    query("INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(42,'insight','summary',42),(42,'insight','tldr',42)").execute(ledger.database()).await.unwrap();
    let service = Insight::new(
        ledger.database().clone(),
        Config {
            gateway_base: gateway,
            model: "fixture".into(),
            mode: "a2a".into(),
            request_timeout: Duration::from_secs(5),
            ..Config::default()
        },
        Arc::new(Material(None)),
        Arc::new(support::completion(Duration::from_secs(5), 128 * 1024).await),
    )
    .unwrap();
    let ids = Arc::new(IdentityStore::new(ledger.database().clone(), None, true));
    let delivery = TextDelivery::new(api, 123, ledger.clone(), Duration::from_secs(2)).unwrap();
    Fixture {
        _dir: dir,
        runtime: InsightRuntime::new(service, ids, delivery, true),
        ledger,
    }
}
async fn wire() -> (TcpListener, Bot) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bot = Bot::with_client(
        "123:fixture",
        reqwest::Client::builder().no_proxy().build().unwrap(),
    )
    .set_api_url(url::Url::parse(&format!("http://{}/", l.local_addr().unwrap())).unwrap());
    (l, bot)
}
async fn request(s: &mut TcpStream) -> (String, Value) {
    let mut bytes = Vec::new();
    let mut buf = [0; 4096];
    let end = loop {
        let n = s.read(&mut buf).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buf[..n]);
        if let Some(n) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break n + 4;
        }
    };
    let header = String::from_utf8_lossy(&bytes[..end]).into_owned();
    let length = header
        .lines()
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().unwrap())
        })
        .unwrap();
    while bytes.len() < end + length {
        let n = s.read(&mut buf).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buf[..n]);
    }
    (
        header,
        serde_json::from_slice(&bytes[end..end + length]).unwrap(),
    )
}
async fn reply(s: &mut TcpStream, body: &Value, id: i32) {
    if body.get("draft_id").is_some() || body.get("action").is_some() || body.get("text").is_none()
    {
        let response = r#"{"ok":true,"result":true}"#;
        s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await.unwrap();
        return;
    }
    let group = body["chat_id"].as_i64().unwrap() < 0;
    let mut result = json!({"message_id":id,"date":0,"chat":{"id":body["chat_id"],"type":if group {"supergroup"} else {"private"},"title":"fixture"},"from":{"id":123,"is_bot":true,"first_name":"fixture"},"text":body["text"]});
    if let Some(topic) = body.get("message_thread_id") {
        result["message_thread_id"] = topic.clone();
    } else if group {
        result["message_thread_id"] = 8.into();
    }
    if let Some(entities) = body.get("entities") {
        result["entities"] = entities.clone();
    }
    let response = json!({"ok":true,"result":result}).to_string();
    s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await.unwrap();
}
async fn first_preview(api: TcpListener) -> TcpListener {
    loop {
        let (mut s, _) = api.accept().await.unwrap();
        let (_, b) = request(&mut s).await;
        reply(&mut s, &b, 90).await;
        if b.get("draft_id").is_some() && b["text"].as_str().unwrap().contains("streamed") {
            return api;
        }
    }
}
fn message(text: &str, chat: i64, topic: Option<i32>, id: i32) -> Message {
    let mut v = json!({"message_id":id,"date":0,"chat":{"id":chat,"type":if chat<0 {"supergroup"} else {"private"},"title":"fixture"},"from":{"id":42,"is_bot":false,"first_name":"fixture"},"text":text});
    if let Some(topic) = topic {
        v["message_thread_id"] = topic.into();
    }
    serde_json::from_value(v).unwrap()
}
async fn groups(f: &Fixture) {
    query("INSERT INTO groups(id,enabled,auto_grant_role,created_at) VALUES(-100,1,'user',1)")
        .execute(f.ledger.database())
        .await
        .unwrap();
    query("INSERT INTO thread_policies(group_id,thread_id,enabled) VALUES(-100,8,1),(-100,9,0)")
        .execute(f.ledger.database())
        .await
        .unwrap();
}
async fn provider(l: TcpListener, started: oneshot::Sender<()>, resume: oneshot::Receiver<()>) {
    let (mut s, _) = l.accept().await.unwrap();
    let (header, body) = request(&mut s).await;
    assert!(header.contains("/chat/completions"));
    assert_eq!(body["stream"], true);
    s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"streamed \"}}]}\n\n").await.unwrap();
    let _ = started.send(());
    let _ = resume.await;
    let _ = s
        .write_all(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"**answer**\"}}]}\n\ndata: [DONE]\n\n",
        )
        .await;
}
#[tokio::test]
async fn direct_and_group_dispatch_real_stream_final_and_receipt_binding() {
    for (chat, topic) in [(42, None), (-100, Some(8))] {
        let (api, bot) = wire().await;
        let gateway = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let f = fixture(bot, format!("http://{}", gateway.local_addr().unwrap())).await;
        groups(&f).await;
        let (started, seen) = oneshot::channel();
        let (resume, wait) = oneshot::channel();
        let upstream = tokio::spawn(provider(gateway, started, wait));
        let (preview_ready, previewed) = oneshot::channel();
        let (typing_ready, typed) = oneshot::channel();
        let telegram =
            tokio::spawn(async move {
                let mut preview_ready = Some(preview_ready);
                let mut typing_ready = Some(typing_ready);
                let mut durable = 0;
                let mut final_revision = 0;
                let mut reading = false;
                let mut generating = false;
                let mut completed = false;
                loop {
                    let (mut s, _) = api.accept().await.unwrap();
                    let (h, b) = request(&mut s).await;
                    assert_eq!(b["chat_id"], chat);
                    if b["action"] == "typing" {
                        assert!(h.contains("sendChatAction"));
                        assert_eq!(b["message_thread_id"], json!(topic));
                        reply(&mut s, &b, 90).await;
                        if let Some(ready) = typing_ready.take() {
                            ready.send(()).unwrap();
                        }
                        continue;
                    }
                    let text = b["text"].as_str().unwrap();
                    assert!(b["parse_mode"].is_null());
                    assert!(!text.contains("**"));
                    assert!(
                        b["entities"].as_array().unwrap().iter().any(
                            |e| e["type"] == "text_link" && e["url"] == "https://example.com/a"
                        ),
                        "request: {h}, body: {b}"
                    );
                    reading |= text.contains("Reading the source.");
                    generating |= text.contains("Generating the response.");
                    let final_body =
                        text.ends_with("streamed answer") && !text.starts_with("Partial response");
                    if b.get("draft_id").is_some() {
                        assert!(h.contains("sendMessageDraft"));
                        assert_eq!(chat, 42);
                        assert_eq!(b["draft_id"], 10);
                        assert!(!completed);
                        assert!(b["reply_parameters"].is_null());
                    } else if completed {
                        assert!(h.contains("sendMessage"));
                        assert_eq!(b["reply_parameters"]["message_id"], 20);
                        assert!(final_body);
                        assert!(text.starts_with("🔗 Summary"));
                        reply(&mut s, &b, 91).await;
                        assert!(reading && generating);
                        return (api, final_revision);
                    } else {
                        if chat > 0 {
                            assert!(final_body);
                            assert!(h.contains("sendMessage"));
                            assert_eq!(b["reply_parameters"]["message_id"], 10);
                        } else if durable == 0 {
                            assert!(h.contains("sendMessage"));
                            assert_eq!(b["reply_parameters"]["message_id"], 10);
                        } else {
                            assert!(h.contains("editMessageText"));
                            assert_eq!(b["message_id"], 90);
                        }
                        final_revision = if chat > 0 { 1 } else { durable };
                        durable += 1;
                        if final_body {
                            assert!(text.starts_with("🔗 Summary"));
                            assert!(
                                b["entities"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .any(|e| e["type"] == "bold" && e["length"] == 6)
                            );
                            completed = true;
                        }
                    }
                    reply(&mut s, &b, 90).await;
                    if text.contains("streamed")
                        && !final_body
                        && let Some(ready) = preview_ready.take()
                    {
                        ready.send(()).unwrap();
                    }
                }
            });
        let runtime = f.runtime.clone();
        let work = tokio::spawn(async move {
            runtime
                .handle(
                    &message("/tldr https://example.com/a", chat, topic, 10),
                    42,
                    "fixture",
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), seen)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), previewed)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), typed)
            .await
            .unwrap()
            .unwrap();
        resume.send(()).unwrap();
        assert!(work.await.unwrap());
        upstream.await.unwrap();
        // Same native request cache: no sink deltas, one final render, no LLM call.
        #[cfg(not(feature = "test-fixtures"))]
        assert!(
            f.runtime
                .handle(
                    &message("/tldr https://example.com/a", chat, topic, 20),
                    42,
                    "fixture"
                )
                .await
        );
        #[cfg(feature = "test-fixtures")]
        {
            use crate::confirmation_hold::HoldState;
            let hold = f.runtime.delivery.confirmation_hold();
            let token = format!("insight:123:{chat}:{}:20:0:0", topic.unwrap_or(0));
            hold.arm(&token, Duration::from_secs(120)).unwrap();
            let runtime = f.runtime.clone();
            let cached = tokio::spawn(async move {
                runtime
                    .handle(
                        &message("/tldr https://example.com/a", chat, topic, 20),
                        42,
                        "fixture",
                    )
                    .await
            });
            tokio::time::timeout(Duration::from_secs(5), async {
                while hold.status().unwrap().state != HoldState::Entered {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            // Actual validated final cached Bot API receipt, not a generation delta.
            let effect = f.ledger.text_effect(&token).await.unwrap().unwrap();
            assert_ne!(effect.outcome, TextOutcome::Confirmed(91));
            assert!(effect.identity.unwrap().text.starts_with("🔗 Summary"));
            let drain = f.runtime.drain_handlers();
            tokio::pin!(drain);
            assert!(futures::poll!(&mut drain).is_pending());
            assert!(f.runtime.admit_handler().is_none());
            cached.abort();
            assert!(cached.await.unwrap_err().is_cancelled());
            tokio::time::timeout(Duration::from_secs(2), &mut drain)
                .await
                .unwrap();
            assert_eq!(hold.status().unwrap().state, HoldState::Dropped);
            assert_ne!(
                f.ledger.text_effect(&token).await.unwrap().unwrap().outcome,
                TextOutcome::Confirmed(91)
            );
            assert!(f.runtime.active.lock().unwrap().is_empty());
        }
        let (api, final_revision) = tokio::time::timeout(Duration::from_secs(5), telegram)
            .await
            .unwrap()
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), api.accept())
                .await
                .is_err()
        );
        assert!(f.runtime.active.lock().unwrap().is_empty());
        let final_effect = f
            .ledger
            .text_effect(&format!(
                "insight:123:{chat}:{}:10:0:{final_revision}",
                topic.unwrap_or(0)
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(final_effect.outcome, TextOutcome::Confirmed(90));
        assert_eq!(final_effect.identity.unwrap().edit, chat < 0);
        if chat > 0 {
            assert_eq!(
                f.ledger
                    .text_effect("insight:123:42:0:10:0:0")
                    .await
                    .unwrap()
                    .unwrap()
                    .outcome,
                TextOutcome::Sealed
            );
        }
    }
}
#[tokio::test]
async fn disabled_grants_and_topic_or_membership_denial_emit_no_requests() {
    let (api, bot) = wire().await;
    let f = fixture(bot, "http://127.0.0.1:1".into()).await;
    groups(&f).await;
    assert!(
        f.runtime
            .handle(
                &message("/tldr https://example.com", -100, Some(9), 10),
                42,
                "fixture"
            )
            .await
    );
    query("DELETE FROM user_permissions WHERE user_id=42")
        .execute(f.ledger.database())
        .await
        .unwrap();
    for command in ["about", "summary", "tldr"] {
        assert!(
            f.runtime
                .handle(
                    &message(
                        &format!("/{command} https://youtu.be/abcdefghijk"),
                        42,
                        None,
                        10
                    ),
                    42,
                    "fixture"
                )
                .await
        );
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(30), api.accept())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn fresh_revoke_during_real_provider_prevents_completion() {
    let (api, bot) = wire().await;
    let gateway = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let f = fixture(bot, format!("http://{}", gateway.local_addr().unwrap())).await;
    let telegram = tokio::spawn(first_preview(api));
    let (started, seen) = oneshot::channel();
    let (resume, wait) = oneshot::channel();
    let upstream = tokio::spawn(provider(gateway, started, wait));
    let runtime = f.runtime.clone();
    let work = tokio::spawn(async move {
        runtime
            .handle(
                &message("/tldr https://example.com/a focus", 42, None, 10),
                42,
                "fixture",
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), seen)
        .await
        .unwrap()
        .unwrap();
    let api = tokio::time::timeout(Duration::from_secs(5), telegram)
        .await
        .unwrap()
        .unwrap();
    query("DELETE FROM user_permissions WHERE user_id=42")
        .execute(f.ledger.database())
        .await
        .unwrap();
    resume.send(()).unwrap();
    assert!(work.await.unwrap());
    upstream.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), api.accept())
            .await
            .is_err()
    );
    assert!(
        f.ledger
            .text_effect("insight:123:42:0:10:0:1")
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn cancel_is_actor_chat_topic_bound_and_shutdown_drains_without_final() {
    for mode in [0, 1, 2] {
        let (api, bot) = wire().await;
        let gateway = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let f = fixture(bot, format!("http://{}", gateway.local_addr().unwrap())).await;
        let telegram = tokio::spawn(first_preview(api));
        let (started, seen) = oneshot::channel();
        let (_resume, wait) = oneshot::channel();
        let upstream = tokio::spawn(provider(gateway, started, wait));
        let runtime = f.runtime.clone();
        let work = tokio::spawn(async move {
            runtime
                .handle(
                    &message("/tldr https://example.com/a focus", 42, None, 10),
                    42,
                    "fixture",
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), seen)
            .await
            .unwrap()
            .unwrap();
        let api = tokio::time::timeout(Duration::from_secs(5), telegram)
            .await
            .unwrap()
            .unwrap();
        let id = *f.runtime.active.lock().unwrap().keys().next().unwrap();
        let cmd = format!("/cancel {id}");
        assert!(
            !f.runtime
                .handle(&message(&cmd, -100, Some(8), 11), 42, "fixture")
                .await
        );
        assert!(
            !f.runtime
                .handle(&message(&cmd, 42, None, 11), 43, "fixture")
                .await
        );
        let retirement = tokio::spawn(async move {
            if mode != 2 {
                let (mut socket, _) = api.accept().await.unwrap();
                let (header, body) = request(&mut socket).await;
                assert!(header.contains("sendMessage"));
                assert!(body["text"].as_str().unwrap().ends_with("Cancelled."));
                assert!(!body["text"].as_str().unwrap().contains("streamed"));
                assert_eq!(body["reply_parameters"]["message_id"], 10);
                reply(&mut socket, &body, 91).await;
            }
            api
        });
        if mode == 2 {
            f.runtime.cancel_all();
        } else {
            let cmd = if mode == 1 { "/cancel" } else { &cmd };
            assert!(
                f.runtime
                    .handle(&message(cmd, 42, None, 11), 42, "fixture")
                    .await
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(2), work)
                .await
                .unwrap()
                .unwrap()
        );
        assert!(f.runtime.active.lock().unwrap().is_empty());
        let api = tokio::time::timeout(Duration::from_secs(2), retirement)
            .await
            .unwrap()
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), api.accept())
                .await
                .is_err()
        );
        upstream.abort();
        let _ = upstream.await;
    }
}

#[tokio::test]
async fn provider_failure_is_terminal_without_stale_cancel_command() {
    let (api, bot) = wire().await;
    let gateway = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let f = fixture(bot, format!("http://{}", gateway.local_addr().unwrap())).await;
    let upstream = tokio::spawn(async move {
        let (mut socket, _) = gateway.accept().await.unwrap();
        request(&mut socket).await;
        socket
            .write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
            )
            .await
            .unwrap();
    });
    let telegram = tokio::spawn(async move {
        loop {
            let (mut socket, _) = api.accept().await.unwrap();
            let (_, body) = request(&mut socket).await;
            let terminal = body.get("draft_id").is_none() && body.get("action").is_none();
            if terminal {
                let text = body["text"].as_str().unwrap();
                assert!(!text.contains("/cancel"));
                assert!(text.contains("rejected authorization"));
            }
            reply(&mut socket, &body, 90).await;
            if terminal {
                break;
            }
        }
    });
    assert!(
        f.runtime
            .handle(
                &message("/tldr https://example.com", 42, None, 10),
                42,
                "fixture"
            )
            .await
    );
    upstream.await.unwrap();
    telegram.await.unwrap();
    assert!(f.runtime.active.lock().unwrap().is_empty());
    f.runtime.cancel_all();
    f.runtime.drain_handlers().await;
    f.ledger.close().await.unwrap();
}
#[tokio::test]
async fn unknown_outbound_receipt_blocks_replay_and_provider() {
    let (api, bot) = wire().await;
    let f = fixture(bot, "http://127.0.0.1:1".into()).await;
    let server = tokio::spawn(async move {
        let (mut s, _) = api.accept().await.unwrap();
        let (_, b) = request(&mut s).await;
        assert!(!b["text"].as_str().unwrap().is_empty());
        assert!(!b["text"].as_str().unwrap().contains("/cancel"));
        drop(s);
        api
    });
    let m = message("/tldr", 42, None, 10);
    assert!(f.runtime.handle(&m, 42, "fixture").await);
    assert!(f.runtime.handle(&m, 42, "fixture").await);
    assert_eq!(
        f.ledger
            .text_effect("insight:123:42:0:10:0:0")
            .await
            .unwrap()
            .unwrap()
            .outcome,
        TextOutcome::Unknown
    );
    let api = server.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), api.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn terminal_failure_retires_every_sent_page_without_replaying_partial_content() {
    let (api, bot) = wire().await;
    let f = fixture(bot, "http://127.0.0.1:1".into()).await;
    let draft = Draft {
        runtime: &f.runtime,
        identity: TextIdentity {
            bot: 123,
            actor: 42,
            chat: 42,
            topic: None,
            message: 10,
            item: None,
            edit: false,
            payload: json!({}),
            text: "pending".into(),
        },
        command: "tldr",
        cancel: CancellationToken::new(),
        base: "failed-pages".into(),
        id: Uuid::new_v4(),
        russian: false,
        state: tokio::sync::Mutex::new({
            let mut state = State::new(false);
            state.text = "Request could not be completed.".into();
            state.sent = vec![
                (90, "protected partial /cancel".into()),
                (91, "protected partial /cancel".into()),
            ];
            state.byok_final = true;
            state
        }),
    };
    let telegram = tokio::spawn(async move {
        for id in [90, 91] {
            let (mut socket, _) = api.accept().await.unwrap();
            let (_, body) = request(&mut socket).await;
            assert_eq!(body["message_id"], id);
            assert_eq!(body["text"], "Request could not be completed.");
            reply(&mut socket, &body, id).await;
        }
    });
    draft.flush_status(false, true).await.unwrap();
    telegram.await.unwrap();
    f.runtime.drain_handlers().await;
    f.ledger.close().await.unwrap();
}
#[tokio::test]
async fn heartbeat_keeps_execution_polling_while_waiting_for_its_database_lease() {
    let (api, bot) = wire().await;
    let gateway = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut f = fixture(bot, format!("http://{}", gateway.local_addr().unwrap())).await;
    f.runtime.service.content = Some(Arc::new(Material(Some(f.ledger.database().clone()))));
    let upstream = tokio::spawn(async move {
        let (mut s, _) = gateway.accept().await.unwrap();
        let (_, body) = request(&mut s).await;
        assert_eq!(body["stream"], true);
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"**answer**\"}}]}\n\ndata: [DONE]\n\n").await.unwrap();
    });
    let telegram = tokio::spawn(async move {
        loop {
            let (mut s, _) = api.accept().await.unwrap();
            let (header, body) = request(&mut s).await;
            let final_send = header.contains("sendMessage") && !header.contains("sendMessageDraft");
            if final_send {
                assert_eq!(body["text"], "🔗 Summary\n\nanswer");
            }
            reply(&mut s, &body, 91).await;
            if final_send {
                return api;
            }
        }
    });
    assert!(
        tokio::time::timeout(
            Duration::from_millis(4500),
            f.runtime.handle(
                &message("/tldr https://example.com/a focus", 42, None, 10),
                42,
                "fixture"
            )
        )
        .await
        .unwrap()
    );
    upstream.await.unwrap();
    let api = telegram.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), api.accept())
            .await
            .is_err()
    );
    f.runtime.drain_handlers().await;
    f.ledger.close().await.unwrap();
}

#[tokio::test]
async fn unsupported_drafts_use_confirmed_edits_while_unavailable_previews_stay_ephemeral() {
    for status in [404, 503] {
        let (api, bot) = wire().await;
        let f = fixture(bot, "http://127.0.0.1:1".into()).await;
        let draft = Draft {
            runtime: &f.runtime,
            identity: TextIdentity {
                bot: 123,
                actor: 42,
                chat: 42,
                topic: None,
                message: 10,
                item: None,
                edit: false,
                payload: json!({}),
                text: "pending".into(),
            },
            command: "tldr",
            cancel: CancellationToken::new(),
            base: "preview-fallback".into(),
            id: Uuid::nil(),
            russian: false,
            state: tokio::sync::Mutex::new(State::new(true)),
        };
        let server = tokio::spawn(async move {
            let (mut s, _) = api.accept().await.unwrap();
            let (header, body) = request(&mut s).await;
            assert!(header.contains("sendMessageDraft"));
            assert_eq!(body["draft_id"], 10);
            assert!(
                body["text"]
                    .as_str()
                    .unwrap()
                    .contains("Reading the source.")
            );
            let data=json!({"ok":false,"error_code":status,"description":if status==404 {"Not Found"} else {"Temporarily unavailable"}}).to_string();
            s.write_all(format!("HTTP/1.1 {status} Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",data.len()).as_bytes()).await.unwrap();
            drop(s);
            if status == 404 {
                for (method, text) in [
                    ("sendMessage", "Reading the source."),
                    ("editMessageText", "value"),
                ] {
                    let (mut s, _) = api.accept().await.unwrap();
                    let (header, body) = request(&mut s).await;
                    assert!(header.contains(method));
                    assert!(body["text"].as_str().unwrap().contains(text));
                    if method == "editMessageText" {
                        assert_eq!(body["message_id"], 90);
                    }
                    reply(&mut s, &body, 90).await;
                }
            } else {
                let (mut s, _) = api.accept().await.unwrap();
                let (header, body) = request(&mut s).await;
                assert!(header.contains("sendMessageDraft"));
                assert_eq!(body["draft_id"], 10);
                assert!(body["text"].as_str().unwrap().contains("value"));
                reply(&mut s, &body, 0).await;
            }
            let (mut s, _) = api.accept().await.unwrap();
            let (header, body) = request(&mut s).await;
            assert!(header.contains("sendMessage") && !header.contains("sendMessageDraft"));
            assert_eq!(body["text"], "🔗 No BS\n\nvalue");
            assert!(
                body["entities"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["type"] == "bold" && e["offset"] == 10)
            );
            reply(&mut s, &body, 91).await;
            if status == 404 {
                let (mut s, _) = api.accept().await.unwrap();
                let (header, body) = request(&mut s).await;
                assert!(header.contains("deleteMessage"));
                assert_eq!(body["message_id"], 90);
                reply(&mut s, &body, 0).await;
            }
            api
        });
        draft
            .phase(
                yoink_insight::provider::Phase::Fetching,
                "https://example.com",
                "nobullshit".into(),
            )
            .await
            .unwrap();
        draft.delta("**value**").await.unwrap();
        draft.flush(true).await.unwrap();
        let state = draft.state.lock().await;
        assert_eq!(state.sent.len(), 1);
        assert_eq!(state.sent[0].0, 91);
        let revision = if status == 404 { 3 } else { 1 };
        assert!(matches!(
            f.ledger
                .text_effect("preview-fallback:0:0")
                .await
                .unwrap()
                .unwrap()
                .outcome,
            TextOutcome::Sealed
        ));
        assert!(matches!(
            f.ledger
                .text_effect(&format!("preview-fallback:0:{revision}"))
                .await
                .unwrap()
                .unwrap()
                .outcome,
            TextOutcome::Confirmed(91)
        ));
        drop(state);
        let api = server.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), api.accept())
                .await
                .is_err()
        );
        f.runtime.drain_handlers().await;
        f.ledger.close().await.unwrap();
    }
}

#[tokio::test]
async fn incremental_chunks_edit_only_confirmed_bot_receipts_and_refresh_group_policy() {
    let (api, bot) = wire().await;
    let f = fixture(bot, "http://127.0.0.1:1".into()).await;
    groups(&f).await;
    let i = TextIdentity {
        bot: 123,
        actor: 42,
        chat: -100,
        topic: Some(8),
        message: 10,
        item: None,
        edit: false,
        payload: json!({}),
        text: "pending".into(),
    };
    let draft = Draft {
        runtime: &f.runtime,
        identity: i,
        command: "tldr",
        cancel: CancellationToken::new(),
        base: "draft-test".into(),
        id: Uuid::nil(),
        russian: false,
        state: tokio::sync::Mutex::new(State::new(false)),
    };
    let body = format!("{}😀{}", "a".repeat(3900), "😀".repeat(200));
    let writer = draft.state.lock().await;
    tokio::time::timeout(Duration::from_millis(50), draft.heartbeat())
        .await
        .unwrap()
        .unwrap();
    drop(writer);
    let chunks = crate::markdown::FormattedText::markdown(None, &body)
        .unwrap()
        .pages(4000);
    assert!(
        chunks
            .iter()
            .all(|c| c.text().encode_utf16().count() <= 4000)
    );
    assert_eq!(chunks.iter().map(|c| c.text()).collect::<String>(), body);
    let server = tokio::spawn(async move {
        for n in 0..2 {
            let (mut s, _) = api.accept().await.unwrap();
            let (_, b) = request(&mut s).await;
            assert!(b["text"].as_str().unwrap().encode_utf16().count() <= 4000);
            assert!(b["text"].as_str().unwrap().starts_with("Partial response"));
            reply(&mut s, &b, 90 + n).await;
        }
        api
    });
    draft.delta(&body).await.unwrap();
    assert_eq!(draft.state.lock().await.sent.len(), 2);
    query("UPDATE thread_policies SET enabled=0 WHERE thread_id=8")
        .execute(f.ledger.database())
        .await
        .unwrap();
    assert!(draft.delta("revoked").await.is_err());
    assert!(draft.flush(true).await.is_err());
    let api = server.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), api.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn about_summary_parse_and_localized_errors_use_trusted_direct_and_group_routes() {
    for (command, chat, topic) in [("about", 42, None), ("summary", -100, Some(8))] {
        let (api, bot) = wire().await;
        let mut f = fixture(bot, "http://127.0.0.1:1".into()).await;
        f.runtime.service.config.mode = "openrouter".into();
        groups(&f).await;
        query("INSERT INTO insight_user_settings(user_id,lang) VALUES(42,'ru')")
            .execute(f.ledger.database())
            .await
            .unwrap();
        let server = tokio::spawn(async move {
            let (mut s, _) = api.accept().await.unwrap();
            let (_, b) = request(&mut s).await;
            assert_eq!(b["chat_id"], chat);
            assert_eq!(b["reply_parameters"]["message_id"], 10);
            assert!(b["text"].as_str().unwrap().contains("Insight недоступен"));
            assert!(!b["text"].as_str().unwrap().contains("·"));
            assert!(!b["text"].as_str().unwrap().contains("/cancel"));
            assert!(b["parse_mode"].is_null());
            reply(&mut s, &b, 90).await;
        });
        // Arguments cannot retarget the route or change the established actor.
        assert!(
            f.runtime
                .handle(
                    &message(
                        &format!("/{command}@fixture https://youtu.be/abcdefghijk 999 owner"),
                        chat,
                        topic,
                        10
                    ),
                    42,
                    "fixture"
                )
                .await
        );
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn outbound_byok_grant_observes_toggle_readiness_and_ban_without_duplicate_resolver() {
    let (api, bot) = wire().await;
    let f = fixture(bot, "http://127.0.0.1:1".into()).await;
    query("DELETE FROM user_permissions WHERE feature='tldr'")
        .execute(f.ledger.database())
        .await
        .unwrap();
    query("INSERT INTO insight_user_byok(user_id,provider,api_key,model,tested_at,test_error,created_at,updated_at) VALUES(42,'openai','fixture-key','fixture',1,NULL,1,1)").execute(f.ledger.database()).await.unwrap();
    query("INSERT INTO bot_settings(key,value,updated_at) VALUES('insight_byok_enabled','true',1)")
        .execute(f.ledger.database())
        .await
        .unwrap();
    let identity = TextIdentity {
        bot: 123,
        actor: 42,
        chat: 42,
        topic: None,
        message: 10,
        item: None,
        edit: false,
        payload: json!({}),
        text: "pending".into(),
    };
    let draft = Draft {
        runtime: &f.runtime,
        identity: identity.clone(),
        command: "tldr",
        cancel: CancellationToken::new(),
        base: "byok-test".into(),
        id: Uuid::nil(),
        russian: false,
        state: tokio::sync::Mutex::new(State::new(false)),
    };
    let server = tokio::spawn(async move {
        let (mut s, _) = api.accept().await.unwrap();
        let (_, b) = request(&mut s).await;
        reply(&mut s, &b, 90).await;
        api
    });
    draft.delta("Visible partial").await.unwrap();
    query("UPDATE bot_settings SET value='false' WHERE key='insight_byok_enabled'")
        .execute(f.ledger.database())
        .await
        .unwrap();
    assert!(draft.delta("toggle revoked").await.is_err());
    assert!(draft.flush(true).await.is_err());
    query("UPDATE bot_settings SET value='true' WHERE key='insight_byok_enabled'")
        .execute(f.ledger.database())
        .await
        .unwrap();
    query("UPDATE insight_user_byok SET test_error='provider_probe_failed' WHERE user_id=42")
        .execute(f.ledger.database())
        .await
        .unwrap();
    assert!(
        f.runtime
            .require(&identity, "tldr", &CancellationToken::new())
            .await
            .is_err()
    );
    query("UPDATE insight_user_byok SET test_error=NULL WHERE user_id=42")
        .execute(f.ledger.database())
        .await
        .unwrap();
    query("UPDATE users SET role='banned' WHERE id=42")
        .execute(f.ledger.database())
        .await
        .unwrap();
    assert!(draft.flush(true).await.is_err());
    let api = server.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), api.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn actual_shared_handler_keeps_incoming_admission_and_optional_injection() {
    for (chat, topic, enabled) in [
        (42, None, true),
        (-100, Some(8), true),
        (-100, Some(8), false),
    ] {
        let (api, bot) = wire().await;
        let mut f = fixture(bot.clone(), "http://127.0.0.1:1".into()).await;
        f.runtime.service.config.mode = "openrouter".into();
        groups(&f).await;
        let system = yoink_application::SystemService::new("fixture");
        system.set_ready(true);
        let identity = crate::TelegramIdentity {
            owner_id: 99,
            bot_id: 123,
            groups_enabled: enabled,
            identity_enabled: true,
        };
        let deps = crate::MessageDependencies {
            music: None,
            stats: None,
            lifecycle: crate::TelegramHandlerLifecycle::new(),
            downloads: None,
            upload: crate::CookieUploadDependency {
                config: None,
                base: url::Url::parse("http://127.0.0.1/").unwrap(),
                client: reqwest::Client::new(),
            },
            cookie_url: crate::CookieUrlDependency(None),
            identities: Some(f.runtime.identities.clone()),
            inbox: None,
            insight: Some(f.runtime.clone()),
        };
        // Raw bot/sender-chat claims must never create product admissions.
        let mut spoof = serde_json::to_value(message(
            "/summary https://youtu.be/abcdefghijk",
            chat,
            topic,
            9,
        ))
        .unwrap();
        spoof["from"]["is_bot"] = true.into();
        crate::handle_message(
            bot.clone(),
            serde_json::from_value(spoof).unwrap(),
            identity,
            "fixture".into(),
            system.clone(),
            deps.clone(),
        )
        .await
        .unwrap();
        if enabled {
            let server = tokio::spawn(async move {
                let (mut s, _) = api.accept().await.unwrap();
                let (_, b) = request(&mut s).await;
                assert_eq!(b["reply_parameters"]["message_id"], 10);
                assert_eq!(b["chat_id"], chat);
                reply(&mut s, &b, 90).await;
                api
            });
            crate::handle_message(
                bot,
                message("/summary https://youtu.be/abcdefghijk", chat, topic, 10),
                identity,
                "fixture".into(),
                system,
                deps,
            )
            .await
            .unwrap();
            let api = tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(30), api.accept())
                    .await
                    .is_err()
            );
        } else {
            crate::handle_message(
                bot,
                message("/summary https://youtu.be/abcdefghijk", chat, topic, 10),
                identity,
                "fixture".into(),
                system,
                deps,
            )
            .await
            .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(30), api.accept())
                    .await
                    .is_err()
            );
        }
    }
}
#[tokio::test]
async fn builtin_help_uses_current_shared_product_grants() {
    let (_api, bot) = wire().await;
    let f = fixture(bot, "http://127.0.0.1:1".into()).await;
    let m = message("/help", 42, None, 10);
    let help = f
        .runtime
        .help_for(&m, 42, yoink_i18n::Language::Russian)
        .await;
    for name in ["/about", "/summary", "/tldr", "/cancel"] {
        assert!(help.contains(name));
    }
    query("DELETE FROM user_permissions WHERE user_id=42")
        .execute(f.ledger.database())
        .await
        .unwrap();
    assert!(
        f.runtime
            .help_for(&m, 42, yoink_i18n::Language::English)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn restricted_help_is_product_authority_not_media_elevation_and_is_durable() {
    let (api, bot) = wire().await;
    let f = fixture(bot.clone(), "http://127.0.0.1:1".into()).await;
    let system = yoink_application::SystemService::new("fixture");
    system.set_ready(true);
    let identity = crate::TelegramIdentity {
        owner_id: 99,
        bot_id: 123,
        groups_enabled: false,
        identity_enabled: true,
    };
    let deps = crate::MessageDependencies {
        music: None,
        stats: None,
        lifecycle: crate::TelegramHandlerLifecycle::new(),
        downloads: None,
        upload: crate::CookieUploadDependency {
            config: None,
            base: url::Url::parse("http://127.0.0.1/").unwrap(),
            client: reqwest::Client::new(),
        },
        cookie_url: crate::CookieUrlDependency(None),
        identities: Some(f.runtime.identities.clone()),
        inbox: None,
        insight: Some(f.runtime.clone()),
    };
    let server = tokio::spawn(async move {
        let (mut s, _) = api.accept().await.unwrap();
        let (_, b) = request(&mut s).await;
        let text = b["text"].as_str().unwrap();
        assert!(text.contains("/summary"));
        assert!(text.contains("/tldr"));
        assert!(!text.contains("/video"));
        reply(&mut s, &b, 90).await;
        api
    });
    crate::handle_message(
        bot.clone(),
        message("/help", 42, None, 10),
        identity,
        "fixture".into(),
        system.clone(),
        deps.clone(),
    )
    .await
    .unwrap();
    crate::handle_message(
        bot,
        message("/help", 42, None, 10),
        identity,
        "fixture".into(),
        system,
        deps,
    )
    .await
    .unwrap();
    let api = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), api.accept())
            .await
            .is_err()
    );
    assert_eq!(
        f.ledger
            .text_effect("insight-help:123:42:0:10")
            .await
            .unwrap()
            .unwrap()
            .outcome,
        TextOutcome::Confirmed(90)
    );
}

#[tokio::test]
async fn final_byok_result_cannot_escape_disabled_toggle_even_with_explicit_grant() {
    let (api, bot) = wire().await;
    let f = fixture(bot, "http://127.0.0.1:1".into()).await;
    query(
        "INSERT INTO bot_settings(key,value,updated_at) VALUES('insight_byok_enabled','false',1)",
    )
    .execute(f.ledger.database())
    .await
    .unwrap();
    let sink = Draft {
        runtime: &f.runtime,
        identity: TextIdentity {
            bot: 123,
            actor: 42,
            chat: 42,
            topic: None,
            message: 10,
            item: None,
            edit: false,
            payload: serde_json::json!({}),
            text: "pending".into(),
        },
        command: "tldr",
        cancel: f.runtime.stop.child_token(),
        base: "byok-final".into(),
        id: Uuid::new_v4(),
        russian: false,
        state: tokio::sync::Mutex::new({
            let mut state = State::new(false);
            state.text = "answer".into();
            state.byok_final = true;
            state
        }),
    };
    // Explicit grant still authorizes another current route, but not this BYOK answer.
    f.runtime
        .require(&sink.identity, "tldr", &sink.cancel)
        .await
        .unwrap();
    assert!(matches!(
        sink.flush(true).await,
        Err(Error::Code("access_denied"))
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), api.accept())
            .await
            .is_err()
    );
}
