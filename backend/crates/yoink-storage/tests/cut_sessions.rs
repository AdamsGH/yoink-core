use std::time::Duration;

use uuid::Uuid;
use yoink_domain::DeliveryTarget;
use yoink_storage::{CutStage, Store, StoreError};

mod common;

const ACTOR: u64 = 42;
const BOT: i64 = 123;
fn target(bot: i64, actor: u64) -> DeliveryTarget {
    DeliveryTarget::new(bot, actor as i64, None, None).unwrap()
}
async fn begin(store: &Store, message: i32, start: Option<f64>) -> yoink_storage::CutSession {
    store
        .cut_begin(
            ACTOR,
            target(BOT, ACTOR),
            message,
            "https://example.com/video",
            start,
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn cut_transitions_preserve_time_input_semantics_and_watermarks() {
    let store = common::store().await;
    let s = begin(&store, 10, Some(12.0)).await;
    assert_eq!((s.stage, s.revision), (CutStage::End, 1));
    assert_eq!(s.target.reply_to_message_id(), Some(10));
    let s = store
        .cut_input(ACTOR, target(BOT, ACTOR), 11, 5.0, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (s.start, s.end, s.stage, s.revision),
        (Some(12.0), Some(17.0), CutStage::Ready, 2)
    );
    assert_eq!(
        store
            .cut_input(ACTOR, target(BOT, ACTOR), 11, 1.0, false)
            .await
            .unwrap()
            .unwrap()
            .end,
        Some(17.0)
    );
    let edited = store
        .cut_edit(ACTOR, target(BOT, ACTOR), s.id, s.revision)
        .await
        .unwrap();
    assert_eq!(
        (edited.revision, edited.stage, edited.start),
        (3, CutStage::Start, None)
    );
    let start = store
        .cut_input(ACTOR, target(BOT, ACTOR), 12, 3.0, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((start.stage, start.revision), (CutStage::End, 4));
    let absolute = store
        .cut_input(ACTOR, target(BOT, ACTOR), 13, 9.0, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (absolute.start, absolute.end, absolute.revision),
        (Some(3.0), Some(9.0), 5)
    );
    assert_eq!(
        store
            .cut_input(ACTOR, target(BOT, ACTOR), 12, 100.0, true)
            .await
            .unwrap()
            .unwrap()
            .end,
        Some(9.0)
    );
    assert_eq!(
        store
            .cut_input(ACTOR, target(BOT, ACTOR), 14, f64::NAN, true)
            .await,
        Err(StoreError::InvalidInput)
    );
    assert_eq!(
        store
            .cut_input(ACTOR, target(BOT, ACTOR), 14, 2.0, true)
            .await,
        Err(StoreError::InvalidInput)
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn cut_cancel_tombstone_replacement_scope_and_restart() {
    let fixture = common::store().await;
    let path = fixture.path();
    let s = begin(&fixture, 20, None).await;
    let start = fixture
        .cut_input(ACTOR, target(BOT, ACTOR), 21, 2.0, false)
        .await
        .unwrap()
        .unwrap();
    let cancelled = fixture
        .cut_cancel(
            ACTOR,
            target(BOT, ACTOR),
            start.id,
            start.revision,
            Some(22),
        )
        .await
        .unwrap();
    assert_eq!(cancelled.stage, CutStage::Cancelled);
    assert_eq!(cancelled.revision, start.revision + 1);
    assert_eq!(
        fixture
            .cut_cancel(
                ACTOR,
                target(BOT, ACTOR),
                start.id,
                cancelled.revision,
                None
            )
            .await
            .unwrap()
            .stage,
        CutStage::Cancelled
    );
    assert_eq!(
        fixture
            .cut_current(ACTOR, target(BOT, ACTOR))
            .await
            .unwrap()
            .unwrap()
            .id,
        s.id
    );
    assert!(
        fixture
            .cut_current(43, target(999, 43))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .cut_current(ACTOR, target(BOT + 1, ACTOR))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture
            .cut_edit(43, target(BOT, 43), start.id, start.revision)
            .await,
        Err(StoreError::Conflict)
    );
    fixture.close().await.unwrap();
    let reopened = Store::open(&path, false, Duration::from_secs(5))
        .await
        .unwrap();
    reopened.verify_schema().await.unwrap();
    assert_eq!(
        reopened
            .cut_current(ACTOR, target(BOT, ACTOR))
            .await
            .unwrap()
            .unwrap()
            .stage,
        CutStage::Cancelled
    );
    let replacement = reopened
        .cut_begin(
            ACTOR,
            target(BOT, ACTOR),
            23,
            "https://example.com/new",
            None,
        )
        .await
        .unwrap();
    assert_eq!(replacement.stage, CutStage::Start);
    assert_eq!(
        reopened
            .cut_begin(
                ACTOR,
                target(BOT, ACTOR),
                23,
                "https://example.com/other",
                None
            )
            .await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        reopened
            .cut_begin(
                ACTOR,
                target(BOT, ACTOR),
                23,
                "https://example.com/new",
                Some(1.0)
            )
            .await,
        Err(StoreError::Conflict)
    );
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn confirming_fences_edits_cancels_replacement_and_submitted_replays() {
    let fixture = common::store().await;
    let path = fixture.path();
    let store = &fixture.store;
    let s = begin(store, 30, Some(0.0)).await;
    let ready = store
        .cut_input(ACTOR, target(BOT, ACTOR), 31, 8.0, false)
        .await
        .unwrap()
        .unwrap();
    let claimed = store
        .cut_claim(ACTOR, target(BOT, ACTOR), ready.id, ready.revision)
        .await
        .unwrap();
    assert_eq!(claimed.stage, CutStage::Confirming);
    assert_eq!(
        store
            .cut_claim(ACTOR, target(BOT, ACTOR), ready.id, ready.revision)
            .await
            .unwrap()
            .stage,
        CutStage::Confirming
    );
    assert_eq!(
        store
            .cut_edit(ACTOR, target(BOT, ACTOR), ready.id, ready.revision)
            .await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        store
            .cut_cancel(ACTOR, target(BOT, ACTOR), ready.id, ready.revision, None)
            .await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        store
            .cut_begin(
                ACTOR,
                target(BOT, ACTOR),
                32,
                "https://example.com/next",
                None
            )
            .await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        store.cut_edit(ACTOR, target(BOT, ACTOR), s.id, 99).await,
        Err(StoreError::Conflict)
    );
    let operation = Uuid::new_v4();
    let submitted = store
        .cut_submitted(
            ACTOR,
            target(BOT, ACTOR),
            ready.id,
            ready.revision,
            operation,
        )
        .await
        .unwrap();
    assert_eq!(submitted.stage, CutStage::Submitted);
    store.close().await.unwrap();
    let reopened = Store::open(&path, false, Duration::from_secs(5))
        .await
        .unwrap();
    let recovered = reopened
        .cut_claim(ACTOR, target(BOT, ACTOR), ready.id, ready.revision)
        .await
        .unwrap();
    assert_eq!(recovered, submitted);
    let replay = reopened
        .cut_submitted(
            ACTOR,
            target(BOT, ACTOR),
            ready.id,
            ready.revision,
            operation,
        )
        .await
        .unwrap();
    assert_eq!(replay.operation_id, Some(operation));
    assert_eq!(replay.source_url(), Some("https://example.com/video"));
    assert!(!format!("{replay:?}").contains("https://example.com/video"));
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn cut_begin_and_cancel_respect_consumed_message_watermark() {
    let store = common::store().await;
    let begin = begin(&store, 100, None).await;
    let advanced = store
        .cut_input(ACTOR, target(BOT, ACTOR), 110, 3.0, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(advanced.revision, begin.revision + 1);
    assert_eq!(
        store
            .cut_begin(
                ACTOR,
                target(BOT, ACTOR),
                105,
                "https://example.com/older",
                None
            )
            .await
            .unwrap()
            .id,
        begin.id
    );
    assert_eq!(
        store
            .cut_cancel(
                ACTOR,
                target(BOT, ACTOR),
                begin.id,
                advanced.revision,
                Some(109)
            )
            .await,
        Err(StoreError::Conflict)
    );
    let current = store
        .cut_current(ACTOR, target(BOT, ACTOR))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (current.stage, current.last_message_id, current.revision),
        (CutStage::End, 110, advanced.revision)
    );
    let cancelled = store
        .cut_cancel(
            ACTOR,
            target(BOT, ACTOR),
            begin.id,
            advanced.revision,
            Some(111),
        )
        .await
        .unwrap();
    assert_eq!(
        (
            cancelled.stage,
            cancelled.last_message_id,
            cancelled.revision
        ),
        (CutStage::Cancelled, 111, advanced.revision + 1)
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn duplicate_origin_preserves_immutable_begin_intent_and_url_bound() {
    let store = common::store().await;
    let initial = begin(&store, 150, Some(4.0)).await;
    let ready = store
        .cut_input(ACTOR, target(BOT, ACTOR), 151, 8.0, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready.stage, CutStage::Ready);
    assert_eq!(
        store
            .cut_begin(
                ACTOR,
                target(BOT, ACTOR),
                150,
                "https://example.com/video",
                Some(5.0)
            )
            .await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        store
            .cut_begin(
                ACTOR,
                target(BOT, ACTOR),
                150,
                "https://example.com/video",
                Some(4.0)
            )
            .await
            .unwrap()
            .id,
        initial.id
    );
    assert_eq!(
        store
            .cut_begin(
                ACTOR,
                target(BOT, ACTOR),
                160,
                &format!("https://example.com/{}", "x".repeat(4100)),
                None
            )
            .await,
        Err(StoreError::InvalidInput)
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn cut_ready_revision_fences_old_buttons_but_claim_replay_keeps_revision() {
    let store = common::store().await;
    let draft = begin(&store, 200, Some(0.0)).await;
    let ready = store
        .cut_input(ACTOR, target(BOT, ACTOR), 201, 5.0, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready.revision, draft.revision + 1);
    assert_eq!(
        store
            .cut_edit(ACTOR, target(BOT, ACTOR), ready.id, draft.revision)
            .await,
        Err(StoreError::Conflict)
    );
    let claimed = store
        .cut_claim(ACTOR, target(BOT, ACTOR), ready.id, ready.revision)
        .await
        .unwrap();
    assert_eq!(claimed.revision, ready.revision);
    assert_eq!(
        store
            .cut_claim(ACTOR, target(BOT, ACTOR), ready.id, ready.revision)
            .await
            .unwrap()
            .revision,
        ready.revision
    );
    store.close().await.unwrap();
}
