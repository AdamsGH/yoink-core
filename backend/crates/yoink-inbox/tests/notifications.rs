mod support;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use support::*;
use yoink_db::{Database, Executor, query};
use yoink_inbox::{
    Error,
    contracts::{Authorization, Call, Content},
    notifications::{Notification, Outcome, RouteAuthority},
    pipeline::parse,
    store::Store,
    telegram::{ReplyTarget, bind_reply},
};
struct Route(AtomicBool);
impl RouteAuthority for Route {
    fn require<'a>(&'a self, _user: i64, _target: ReplyTarget) -> Call<'a, ()> {
        Box::pin(async move {
            if self.0.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(Error::Forbidden)
            }
        })
    }
}
struct Revoking<'a>(&'a AtomicBool);
impl RouteAuthority for Revoking<'_> {
    fn require<'a>(&'a self, _user: i64, _target: ReplyTarget) -> Call<'a, ()> {
        Box::pin(async move {
            self.0.store(false, Ordering::SeqCst);
            Ok(())
        })
    }
}
struct Fresh(AtomicBool);
impl Authorization for Fresh {
    fn require<'a>(&'a self, _user: i64, _feature: &'a str) -> Call<'a, ()> {
        Box::pin(async move {
            if self.0.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(Error::Forbidden)
            }
        })
    }
    fn require_admin(&self, _user: i64) -> Call<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
async fn complete(s: &Store, source: &str, bound: bool, now: i64) -> (i64, Notification) {
    let id = s
        .ingest(1, &format!("https://example.com/{now}"), source, now)
        .await
        .unwrap()
        .item_id;
    if bound {
        bind_reply(
            s,
            &Auth,
            1,
            id,
            ReplyTarget {
                chat_id: -100,
                message_id: 42,
            },
        )
        .await
        .unwrap();
    }
    let job = s
        .claim(now, 100, &format!("enrich-{now}"))
        .await
        .unwrap()
        .unwrap();
    assert!(
        s.apply_content(
            &job,
            &Content {
                title: Some("Card".into()),
                content_text: Some("body".into()),
                ..Default::default()
            },
            now
        )
        .await
        .unwrap()
    );
    let job = s
        .claim(now, 100, &format!("classify-{now}"))
        .await
        .unwrap()
        .unwrap();
    assert!(s.apply_classification(&job,&parse(&json!({"summary":format!("Summary {now}"),"categories":[{"name":"Topic","kind":"new","confidence":0.8}]}).to_string()),now).await.unwrap());
    assert!(
        !s.apply_classification(&job, &parse("{}"), now)
            .await
            .unwrap()
    );
    let row = query("SELECT id FROM inbox_notifications WHERE item_id=?")
        .bind(id)
        .fetch_one(&s.db)
        .await
        .unwrap();
    let n = s
        .notification(1, &row.try_get::<String, _>("id").unwrap())
        .await
        .unwrap();
    (id, n)
}
#[tokio::test]
async fn atomic_completion_binding_restart_and_manual_snapshot() {
    let (dir, s) = db().await;
    let (id, n) = complete(&s, "bot", false, 1).await;
    assert_eq!(n.state, "waiting");
    assert_eq!(n.payload["summary"], "Summary 1");
    assert_eq!(n.payload["categories"], json!(["Topic"]));
    assert!(s.claim_notification(2, 10).await.unwrap().is_none());
    assert!(matches!(
        s.notification(2, &n.id).await,
        Err(Error::NotFound)
    ));
    bind_reply(
        &s,
        &Auth,
        1,
        id,
        ReplyTarget {
            chat_id: -100,
            message_id: 42,
        },
    )
    .await
    .unwrap();
    bind_reply(
        &s,
        &Auth,
        1,
        id,
        ReplyTarget {
            chat_id: -100,
            message_id: 42,
        },
    )
    .await
    .unwrap();
    assert!(
        !bind_reply(
            &s,
            &Auth,
            1,
            id,
            ReplyTarget {
                chat_id: -100,
                message_id: 43
            }
        )
        .await
        .unwrap()
    );
    drop(s);
    let reopened = Store::new(
        Database::open(
            &dir.path().join("db"),
            false,
            std::time::Duration::from_secs(10),
        )
        .await
        .unwrap(),
    );
    let claim = reopened.claim_notification(4, 10).await.unwrap().unwrap();
    assert_eq!(claim.id, n.id);
    assert_eq!(claim.chat_id, Some(-100));
    let route = Route(AtomicBool::new(true));
    let token = reopened
        .begin_notification_send(&Auth, &route, &claim, || 5)
        .await
        .unwrap();
    assert!(
        reopened
            .notification_outcome(
                1,
                &claim.id,
                &token,
                Outcome::Confirmed {
                    receipt: "ledger-receipt".into()
                },
                6
            )
            .await
            .unwrap()
    );
    assert_eq!(
        reopened.notification(1, &n.id).await.unwrap().state,
        "acked"
    );
    assert!(
        !reopened
            .notification_outcome(1, &claim.id, &token, Outcome::Unknown, 7)
            .await
            .unwrap()
    );
}
#[tokio::test]
async fn transaction_rollback_has_no_completion_loss_or_partial_outbox() {
    let (_d, s) = db().await;
    let id = s
        .ingest(1, "https://example.com/rollback", "bot", 1)
        .await
        .unwrap()
        .item_id;
    let job = s.claim(1, 100, "enrich").await.unwrap().unwrap();
    s.apply_content(&job, &Content::default(), 2).await.unwrap();
    let job = s.claim(2, 100, "classify").await.unwrap().unwrap();
    s.db.batch("CREATE TRIGGER fail_notification BEFORE INSERT ON inbox_notifications BEGIN SELECT RAISE(ABORT,'fixture'); END;").await.unwrap();
    assert!(
        s.apply_classification(&job, &parse(r#"{"summary":"done"}"#), 3)
            .await
            .is_err()
    );
    assert_eq!(s.item(1, id).await.unwrap()["status"], "enriched");
    assert_eq!(
        query("SELECT state FROM inbox_jobs WHERE id=?")
            .bind(job.id)
            .fetch_one(&s.db)
            .await
            .unwrap()
            .try_get::<String, _>("state")
            .unwrap(),
        "running"
    );
    s.db.batch("DROP TRIGGER fail_notification").await.unwrap();
    assert!(
        s.apply_classification(&job, &parse(r#"{"summary":"done"}"#), 4)
            .await
            .unwrap()
    );
    assert_eq!(
        query("SELECT count(*) AS n FROM inbox_notifications")
            .fetch_one(&s.db)
            .await
            .unwrap()
            .try_get::<i64, _>("n")
            .unwrap(),
        1
    );
}
#[tokio::test]
async fn pre_send_crash_reclaim_retry_and_fresh_authority() {
    let (_d, s) = db().await;
    complete(&s, "bot", true, 1).await;
    let a = s.claim_notification(2, 10).await.unwrap().unwrap();
    assert!(s.claim_notification(3, 10).await.unwrap().is_none());
    let b = s.claim_notification(12, 10).await.unwrap().unwrap();
    assert_ne!(a.lease_token, b.lease_token);
    let route = Route(AtomicBool::new(true));
    assert!(
        s.begin_notification_send(&Auth, &route, &a, || 13)
            .await
            .is_err()
    );
    assert!(!s.retry_notification(&a, 13, 14).await.unwrap());
    assert!(s.retry_notification(&b, 13, 15).await.unwrap());
    assert!(s.claim_notification(14, 10).await.unwrap().is_none());
    let c = s.claim_notification(15, 10).await.unwrap().unwrap();
    route.0.store(false, Ordering::SeqCst);
    assert!(matches!(
        s.begin_notification_send(&Auth, &route, &c, || 16).await,
        Err(Error::Forbidden)
    ));
    let auth = Fresh(AtomicBool::new(true));
    let revoking = Revoking(&auth.0);
    assert!(matches!(
        s.begin_notification_send(&auth, &revoking, &c, || 16).await,
        Err(Error::Forbidden)
    ));
    route.0.store(true, Ordering::SeqCst);
    let (first, second) = tokio::join!(
        s.begin_notification_send(&Auth, &route, &c, || 16),
        s.begin_notification_send(&Auth, &route, &c, || 16)
    );
    assert_ne!(first.is_ok(), second.is_ok());
    assert!(!s.retry_notification(&c, 17, 18).await.unwrap());
}
#[tokio::test]
async fn ambiguous_send_quarantined_never_replayed_and_serializes_reply_edits() {
    let (dir, s) = db().await;
    let (_, n) = complete(&s, "bot", true, 1).await;
    let route = Route(AtomicBool::new(true));
    let a = s.claim_notification(2, 10).await.unwrap().unwrap();
    let token = s
        .begin_notification_send(&Auth, &route, &a, || 3)
        .await
        .unwrap();
    let (_, newer) = complete(&s, "bot", true, 4).await;
    assert!(s.claim_notification(5, 10).await.unwrap().is_none());
    drop(s);
    let s = Store::new(
        Database::open(
            &dir.path().join("db"),
            false,
            std::time::Duration::from_secs(10),
        )
        .await
        .unwrap(),
    );
    assert!(s.claim_notification(12, 10).await.unwrap().is_none());
    assert_eq!(s.notification(1, &n.id).await.unwrap().state, "unknown");
    assert!(
        !s.notification_outcome(
            2,
            &n.id,
            &token,
            Outcome::DefinitelyNotSent { retry_at: 13 },
            12
        )
        .await
        .unwrap()
    );
    assert!(
        !s.notification_outcome(
            1,
            &n.id,
            "wrong-token",
            Outcome::Confirmed {
                receipt: "bad".into()
            },
            12
        )
        .await
        .unwrap()
    );
    assert!(
        s.notification_outcome(1, &n.id, &token, Outcome::Unknown, 12)
            .await
            .unwrap()
    );
    assert!(s.claim_notification(13, 10).await.unwrap().is_none());
    assert!(
        s.notification_outcome(
            1,
            &n.id,
            &token,
            Outcome::DefinitelyNotSent { retry_at: 15 },
            14
        )
        .await
        .unwrap()
    );
    assert!(s.claim_notification(14, 10).await.unwrap().is_none());
    let b = s.claim_notification(15, 10).await.unwrap().unwrap();
    assert_eq!(b.id, n.id);
    let new_token = s
        .begin_notification_send(&Auth, &route, &b, || 16)
        .await
        .unwrap();
    assert_ne!(new_token, token);
    assert!(
        !s.notification_outcome(
            1,
            &n.id,
            &token,
            Outcome::Confirmed {
                receipt: "stale".into()
            },
            17
        )
        .await
        .unwrap()
    );
    assert!(
        s.notification_outcome(
            1,
            &n.id,
            &new_token,
            Outcome::Confirmed {
                receipt: "confirmed".into()
            },
            17
        )
        .await
        .unwrap()
    );
    assert_eq!(
        s.claim_notification(18, 10).await.unwrap().unwrap().id,
        newer.id
    );
}
#[tokio::test]
async fn archive_delete_cancels_pre_send_but_late_confirmed_effect_is_recorded() {
    let (_d, s) = db().await;
    let (id, n) = complete(&s, "bot", true, 1).await;
    let a = s.claim_notification(2, 10).await.unwrap().unwrap();
    s.archive(1, id, false, 3).await.unwrap();
    let route = Route(AtomicBool::new(true));
    assert!(
        s.begin_notification_send(&Auth, &route, &a, || 4)
            .await
            .is_err()
    );
    assert!(s.claim_notification(4, 10).await.unwrap().is_none());
    assert_eq!(s.notification(1, &n.id).await.unwrap().state, "cancelled");
    let (id, n) = complete(&s, "bot", true, 5).await;
    let a = s.claim_notification(6, 10).await.unwrap().unwrap();
    let token = s
        .begin_notification_send(&Auth, &route, &a, || 7)
        .await
        .unwrap();
    s.archive(1, id, true, 8).await.unwrap();
    assert!(s.claim_notification(16, 10).await.unwrap().is_none());
    assert_eq!(s.notification(1, &n.id).await.unwrap().item_id, None);
    assert!(
        s.notification_outcome(
            1,
            &n.id,
            &token,
            Outcome::Confirmed {
                receipt: "late".into()
            },
            17
        )
        .await
        .unwrap()
    );
}
