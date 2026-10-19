use std::time::Duration;

use chrono::Utc;
use yoink_domain::{DeliveryTarget, DownloadParams, DownloadRequest, MediaMode};
use yoink_storage::{ProgressMessage, Store, StoreError};

mod common;

async fn admitted(store: &Store, key: &str) -> ProgressMessage {
    let request = DownloadRequest::new(
        42,
        "https://example.com/media",
        MediaMode::Video,
        None,
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(123456, 42, None, Some(17)).unwrap())
    .unwrap();
    let id = store.admit(&request, key, 1).await.unwrap().operation_id;
    ProgressMessage {
        operation_id: id,
        actor_id: 42,
        bot_id: 123456,
        chat_id: 42,
        message_id: 1001,
        language: "ru".into(),
    }
}

#[tokio::test]
async fn reply_lookup_matches_input_and_progress_only_for_the_original_owner_and_route() {
    let store = common::store().await;
    let message = admitted(&store, "reply-lookup").await;
    store.register_progress_message(&message).await.unwrap();
    for message_id in [17, 1001] {
        assert_eq!(
            store
                .operation_by_reply(42, 123456, 42, message_id)
                .await
                .unwrap(),
            Some(message.operation_id)
        );
        for (actor, bot, chat) in [(43, 123456, 42), (42, 654321, 42), (42, 123456, 43)] {
            assert_eq!(
                store
                    .operation_by_reply(actor, bot, chat, message_id)
                    .await
                    .unwrap(),
                None
            );
        }
    }
    assert_eq!(
        store
            .operation_by_reply(42, 123456, 42, 1002)
            .await
            .unwrap(),
        None
    );
    assert!(matches!(
        store.operation_by_reply(42, 123456, 42, 0).await,
        Err(StoreError::InvalidInput)
    ));
    store.close().await.unwrap();
}

#[tokio::test]
async fn progress_keeps_first_confirmed_destination_and_recovers_after_reopen() {
    let store = common::store().await;
    let message = admitted(&store, "progress-first").await;
    for altered in [
        ProgressMessage {
            actor_id: 43,
            ..message.clone()
        },
        ProgressMessage {
            bot_id: 654321,
            ..message.clone()
        },
        ProgressMessage {
            chat_id: 43,
            ..message.clone()
        },
    ] {
        assert!(!store.register_progress_message(&altered).await.unwrap());
    }
    assert!(store.register_progress_message(&message).await.unwrap());
    assert!(
        !store
            .register_progress_message(&ProgressMessage {
                message_id: 1002,
                language: "en".into(),
                ..message.clone()
            })
            .await
            .unwrap()
    );
    assert!(store.due_progress_notice(654321).await.unwrap().is_none());
    assert_eq!(
        store
            .due_progress_notice(message.bot_id)
            .await
            .unwrap()
            .unwrap()
            .message,
        message
    );
    store.close().await.unwrap();
    let reopened = Store::open(&store.path(), false, Duration::from_secs(5))
        .await
        .unwrap();
    reopened.verify_schema().await.unwrap();
    assert_eq!(
        reopened
            .due_progress_notice(message.bot_id)
            .await
            .unwrap()
            .unwrap()
            .message,
        message
    );
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn progress_retry_retains_acknowledged_text_and_finished_notice_cannot_resume() {
    let store = common::store().await;
    let message = admitted(&store, "progress-retry").await;
    store.register_progress_message(&message).await.unwrap();
    let due = Utc::now() - chrono::Duration::seconds(1);
    let future = Utc::now() + chrono::Duration::hours(1);
    assert!(
        store
            .advance_progress_notice(&message, Some("Downloading 25%"), false, due)
            .await
            .unwrap()
    );
    assert!(
        store
            .advance_progress_notice(&message, None, false, future)
            .await
            .unwrap()
    );
    assert!(
        store
            .due_progress_notice(message.bot_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .advance_progress_notice(&message, None, false, due)
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .due_progress_notice(message.bot_id)
            .await
            .unwrap()
            .unwrap()
            .last_text
            .as_deref(),
        Some("Downloading 25%")
    );
    assert!(
        !store
            .advance_progress_notice(
                &ProgressMessage {
                    message_id: 1002,
                    ..message.clone()
                },
                Some("stale"),
                true,
                due
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .advance_progress_notice(&message, Some("Succeeded"), true, due)
            .await
            .unwrap()
    );
    assert!(
        !store
            .advance_progress_notice(&message, Some("stale"), false, due)
            .await
            .unwrap()
    );
    assert!(
        store
            .due_progress_notice(message.bot_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn invalid_progress_inputs_leave_registration_and_updates_unchanged() {
    let store = common::store().await;
    let message = admitted(&store, "progress-invalid").await;
    for altered in [
        ProgressMessage {
            actor_id: u64::MAX,
            ..message.clone()
        },
        ProgressMessage {
            message_id: 0,
            ..message.clone()
        },
        ProgressMessage {
            language: "unknown".into(),
            ..message.clone()
        },
    ] {
        assert!(matches!(
            store.register_progress_message(&altered).await,
            Err(StoreError::InvalidInput)
        ));
    }
    assert!(
        store
            .due_progress_notice(message.bot_id)
            .await
            .unwrap()
            .is_none()
    );
    store.register_progress_message(&message).await.unwrap();
    assert!(matches!(
        store
            .advance_progress_notice(&message, Some(&"x".repeat(4097)), true, Utc::now())
            .await,
        Err(StoreError::InvalidInput)
    ));
    assert!(
        store
            .due_progress_notice(message.bot_id)
            .await
            .unwrap()
            .unwrap()
            .last_text
            .is_none()
    );
}
