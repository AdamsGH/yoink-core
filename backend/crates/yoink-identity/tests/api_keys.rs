use std::{path::Path, time::Duration};

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use yoink_db::{query, raw_sql};
use yoink_identity::{
    IdentityStore,
    admin::{AdminError, api_keys::ApiKeyCreate},
};
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
    let store = Store::open(
        &dir.path().join("api-keys.db"),
        true,
        Duration::from_secs(2),
    )
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

fn at(micros: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(micros).unwrap()
}

#[tokio::test]
async fn create_defaults_scopes_secret_storage_unicode_and_persistence() {
    let (_dir, store, identity) = fixture().await;
    let expiry = at(1_700_000_000_123_456);
    let created = identity
        .api_key_create(
            7102,
            &ApiKeyCreate {
                name: "\u{9375} \u{2013} \u{00e9}quipe".into(),
                scopes: Some(vec!["users:read".into(), "groups:*".into()]),
                expires_at: Some(expiry),
            },
        )
        .await
        .unwrap();
    assert!(created.key.starts_with("yoink_"));
    assert_eq!(
        created.prefix,
        created.key.chars().take(8).collect::<String>()
    );
    assert_eq!(created.scopes, ["users:read", "groups:*"]);
    assert_eq!(created.expires_at, Some(expiry));
    let hash = format!("{:x}", Sha256::digest(created.key.as_bytes()));
    let stored = query(include_str!("sql/api_key_secret_read.sql"))
        .bind(created.id)
        .fetch_one(store.database())
        .await
        .unwrap();
    assert_eq!(stored.try_get::<String, _>(0).unwrap(), hash);
    assert_eq!(stored.try_get::<String, _>(1).unwrap(), created.prefix);
    assert_eq!(
        stored.try_get::<String, _>(2).unwrap(),
        r#"["users:read","groups:*"]"#
    );
    assert_eq!(
        stored.try_get::<Option<DateTime<Utc>>, _>(3).unwrap(),
        Some(expiry)
    );
    assert!(
        query(include_str!("sql/api_key_duplicate_hash.sql"))
            .bind(&hash)
            .execute(store.database())
            .await
            .is_err(),
        "key hashes are unique"
    );
    let listed = identity.api_keys_list(7102).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "\u{9375} \u{2013} \u{00e9}quipe");
    assert_eq!(listed[0].expires_at, Some(expiry));
    let rendered = serde_json::to_string(&listed).unwrap();
    assert!(!rendered.contains(&created.key));
    assert!(!rendered.contains(&hash));
    let defaulted = identity
        .api_key_create(
            7102,
            &ApiKeyCreate {
                name: "defaults".into(),
                scopes: None,
                expires_at: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(defaulted.scopes, ["*"]);
    assert_eq!(defaulted.expires_at, None);
    assert_ne!(created.key, defaulted.key);
    let empty = identity
        .api_key_create(
            7102,
            &ApiKeyCreate {
                name: "empty".into(),
                scopes: Some(vec![]),
                expires_at: Some(at(1_600_000_000_123_456)),
            },
        )
        .await
        .unwrap();
    assert!(empty.scopes.is_empty());
    assert_eq!(
        empty.expires_at,
        Some(at(1_600_000_000_123_456)),
        "past legacy expiries are retained"
    );
    let path = store.database().path().to_path_buf();
    drop(identity);
    store.close().await.unwrap();
    drop(store);
    let reopened = Store::open(&path, false, Duration::from_secs(2))
        .await
        .unwrap();
    let persisted = IdentityStore::new(reopened.database().clone(), Some(7101), false)
        .api_keys_list(7102)
        .await
        .unwrap();
    let mut names: Vec<_> = persisted.iter().map(|key| key.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["defaults", "empty", "\u{9375} \u{2013} \u{00e9}quipe"]
    );
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn auth_scope_validation_revocation_and_missing_ids() {
    let (_dir, store, identity) = fixture().await;
    assert_eq!(
        identity.api_keys_list(7103).await.unwrap_err(),
        AdminError::Forbidden
    );
    assert_eq!(
        identity.api_keys_list(7999).await.unwrap_err(),
        AdminError::Unauthorized
    );
    assert_eq!(
        identity
            .api_key_create(
                7102,
                &ApiKeyCreate {
                    name: "bad".into(),
                    scopes: Some(vec!["not-a-scope".into()]),
                    expires_at: None
                }
            )
            .await
            .err()
            .unwrap(),
        AdminError::InvalidScope
    );
    let key = identity
        .api_key_create(
            7102,
            &ApiKeyCreate {
                name: "revoke".into(),
                scopes: None,
                expires_at: None,
            },
        )
        .await
        .unwrap();
    identity.api_key_revoke(7102, key.id).await.unwrap();
    identity.api_key_revoke(7102, key.id).await.unwrap();
    assert!(identity.api_keys_list(7102).await.unwrap()[0].revoked);
    assert_eq!(
        identity.api_key_revoke(7102, i64::MAX).await.unwrap_err(),
        AdminError::NotFound
    );
    assert_eq!(
        identity.api_key_revoke(7102, -1).await.unwrap_err(),
        AdminError::InvalidInput
    );
    query(include_str!("sql/api_key_set_role.sql"))
        .bind("user")
        .bind(7102_i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        identity.api_keys_list(7102).await.unwrap_err(),
        AdminError::Forbidden,
        "a demoted JWT actor is rechecked against current DB role"
    );
    query(include_str!("sql/api_key_set_role.sql"))
        .bind("banned")
        .bind(7101_i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        identity.api_keys_authorized(7101).await.unwrap_err(),
        AdminError::Forbidden,
        "configured owner ban precedes owner elevation"
    );
    assert_eq!(
        identity.api_keys_authorized(7104).await.unwrap_err(),
        AdminError::Forbidden,
        "timed ban blocks admin"
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn imported_corrupt_scope_records_fail_closed_without_repair() {
    let (_dir, store, identity) = fixture().await;
    raw_sql(include_str!("sql/api_key_corrupt_scope.sql"))
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        identity.api_keys_list(7102).await.unwrap_err(),
        AdminError::Unavailable
    );
    let persisted: String = query(include_str!("sql/api_key_read_scope.sql"))
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(persisted, r#"{"broken":true}"#);
    store.close().await.unwrap();
}
