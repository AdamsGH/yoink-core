use std::{path::Path, time::Duration};

use chrono::{DateTime, Utc};
use serde_json::json;
use yoink_db::{query, raw_sql};
use yoink_identity::{IdentityStore, admin::api_keys::ApiKeyCreate, m2m::M2mEventCreate};
use yoink_storage::Store;

#[cfg(unix)]
fn secure(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}
#[cfg(not(unix))]
fn secure(_: &Path) {}

async fn fixture() -> (tempfile::TempDir, Store, IdentityStore) {
    let dir = tempfile::tempdir().unwrap();
    secure(dir.path());
    let store = Store::open(&dir.path().join("m2m.db"), true, Duration::from_secs(2))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    raw_sql(include_str!("sql/api_keys_users.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let identity = IdentityStore::new(store.database().clone(), Some(7101), false);
    (dir, store, identity)
}

#[tokio::test]
async fn m2m_auth_tracks_only_valid_use_and_rechecks_revoke_and_expiry() {
    let (_dir, store, identity) = fixture().await;
    let created = identity
        .api_key_create(
            7102,
            &ApiKeyCreate {
                name: "m2m integration".into(),
                scopes: Some(vec!["events:read".into()]),
                expires_at: None,
            },
        )
        .await
        .unwrap();
    let authenticated = identity
        .m2m_authenticate(&created.key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(authenticated.scopes, ["events:read"]);
    let touched = query(include_str!("sql/m2m_last_used.sql"))
        .bind(created.id)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get::<Option<DateTime<Utc>>, _>(0)
        .unwrap();
    assert!(touched.is_some());

    let before_invalid = touched;
    assert!(
        identity
            .m2m_authenticate("not-a-real-key")
            .await
            .unwrap()
            .is_none()
    );
    let after_invalid = query(include_str!("sql/m2m_last_used.sql"))
        .bind(created.id)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get::<Option<DateTime<Utc>>, _>(0)
        .unwrap();
    assert_eq!(after_invalid, before_invalid);

    identity.api_key_revoke(7102, created.id).await.unwrap();
    assert!(
        identity
            .m2m_authenticate(&created.key)
            .await
            .unwrap()
            .is_none()
    );
    let after_revoke = query(include_str!("sql/m2m_last_used.sql"))
        .bind(created.id)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get::<Option<DateTime<Utc>>, _>(0)
        .unwrap();
    assert_eq!(after_revoke, before_invalid);
    store.close().await.unwrap();
}

#[tokio::test]
async fn expiry_at_database_now_and_corrupt_scopes_fail_closed_without_repair() {
    let (_dir, store, identity) = fixture().await;
    let expired = identity
        .api_key_create(
            7102,
            &ApiKeyCreate {
                name: "expired".into(),
                scopes: Some(vec!["*".into()]),
                expires_at: None,
            },
        )
        .await
        .unwrap();
    query(include_str!("sql/m2m_expire_now.sql"))
        .bind(expired.id)
        .execute(store.database())
        .await
        .unwrap();
    assert!(
        identity
            .m2m_authenticate(&expired.key)
            .await
            .unwrap()
            .is_none()
    );
    let used: Option<DateTime<Utc>> = query(include_str!("sql/m2m_last_used.sql"))
        .bind(expired.id)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(used, None);

    let corrupt = identity
        .api_key_create(
            7102,
            &ApiKeyCreate {
                name: "corrupt".into(),
                scopes: Some(vec!["*".into()]),
                expires_at: None,
            },
        )
        .await
        .unwrap();
    query(include_str!("sql/m2m_corrupt_scopes.sql"))
        .bind(corrupt.id)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        identity.m2m_authenticate(&corrupt.key).await.err().unwrap(),
        yoink_identity::admin::AdminError::Unavailable
    );
    let scopes: String = query(include_str!("sql/m2m_read_scopes.sql"))
        .bind(corrupt.id)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(scopes, r#"{"broken":true}"#);

    let legacy = identity
        .api_key_create(
            7102,
            &ApiKeyCreate {
                name: "legacy creator".into(),
                scopes: Some(vec!["*".into()]),
                expires_at: None,
            },
        )
        .await
        .unwrap();
    query(include_str!("sql/m2m_ban_creator.sql"))
        .bind(7102_i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        identity
            .m2m_authenticate(&legacy.key)
            .await
            .unwrap()
            .unwrap()
            .scopes,
        ["*"]
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn m2m_event_persists_nullable_ids_object_payload_thread_and_microseconds() {
    let (_dir, store, identity) = fixture().await;
    let payload = json!({"nested": [1, true, null]});
    let event = identity
        .m2m_event_create(M2mEventCreate {
            plugin: "dl",
            event_type: "join",
            user_id: Some(7103),
            group_id: Some(-7103),
            thread_id: Some(99),
            payload: Some(&payload),
        })
        .await
        .unwrap();
    assert_eq!(event.payload, Some(payload.clone()));
    let listed = identity
        .m2m_events(Some("dl"), Some("join"), 0, 50)
        .await
        .unwrap();
    assert_eq!(listed.as_slice(), std::slice::from_ref(&event));
    let saved = query(include_str!("sql/m2m_event_thread.sql"))
        .bind(event.id)
        .fetch_one(store.database())
        .await
        .unwrap();
    assert_eq!(saved.try_get::<Option<i64>, _>(0).unwrap(), Some(99));
    assert!(
        saved
            .try_get::<DateTime<Utc>, _>(1)
            .unwrap()
            .timestamp_subsec_micros()
            <= 999_999
    );
    let null_payload = identity
        .m2m_event_create(M2mEventCreate {
            plugin: "stats",
            event_type: "leave",
            user_id: None,
            group_id: None,
            thread_id: None,
            payload: None,
        })
        .await
        .unwrap();
    assert_eq!(null_payload.payload, None);
    query(include_str!("sql/m2m_json_null.sql"))
        .bind(null_payload.id)
        .execute(store.database())
        .await
        .unwrap();
    let listed = identity
        .m2m_events(Some("stats"), None, 0, 50)
        .await
        .unwrap();
    assert_eq!(listed.as_slice(), std::slice::from_ref(&null_payload));
    store.close().await.unwrap();
}

#[tokio::test]
async fn m2m_records_survive_database_reopen() {
    let (dir, store, identity) = fixture().await;
    let key = identity
        .api_key_create(
            7102,
            &ApiKeyCreate {
                name: "restart".into(),
                scopes: Some(vec!["*".into()]),
                expires_at: None,
            },
        )
        .await
        .unwrap();
    identity
        .m2m_event_create(M2mEventCreate {
            plugin: "insight",
            event_type: "summary",
            user_id: Some(7103),
            group_id: None,
            thread_id: None,
            payload: Some(&json!({"persisted": true})),
        })
        .await
        .unwrap();
    let path = store.database().path().to_path_buf();
    drop(identity);
    store.close().await.unwrap();
    drop(store);
    let reopened = Store::open(&path, false, Duration::from_secs(2))
        .await
        .unwrap();
    let identity = IdentityStore::new(reopened.database().clone(), Some(7101), false);
    assert_eq!(
        identity
            .m2m_authenticate(&key.key)
            .await
            .unwrap()
            .unwrap()
            .scopes,
        ["*"]
    );
    assert_eq!(
        identity
            .m2m_events(Some("insight"), None, 0, 50)
            .await
            .unwrap()
            .len(),
        1
    );
    reopened.close().await.unwrap();
    drop(dir);
}
