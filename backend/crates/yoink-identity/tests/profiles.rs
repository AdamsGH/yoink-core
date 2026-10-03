use std::time::Duration;

use yoink_db::query;
use yoink_identity::{IdentityError, IdentityStore, TrustedProfile};
use yoink_storage::Store;

fn profile(user_id: i64) -> TrustedProfile {
    TrustedProfile {
        user_id,
        username: Some("profile-user".into()),
        first_name: Some("Profile".into()),
        photo_url: Some("https://example.test/p.png".into()),
    }
}

#[tokio::test]
async fn registration_settings_and_language_revision_use_migrated_schema() {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let database_path = directory.path().join("profiles.db");
    let store = Store::open(&database_path, true, Duration::from_secs(2))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    let db = store.database().clone();
    let identity = IdentityStore::new(db.clone(), Some(91_000_000_000_001), false);

    let owner_profile = profile(91_000_000_000_001);
    let (owner_a, owner_b) = tokio::join!(
        identity.register(&owner_profile),
        identity.register(&owner_profile)
    );
    let user = owner_a.unwrap();
    assert_eq!(owner_b.unwrap().role.to_string(), "owner");
    assert_eq!(user.role.to_string(), "owner");
    let open_profile = profile(91_000_000_000_010);
    let (open_a, open_b) = tokio::join!(
        identity.register(&open_profile),
        identity.register(&open_profile)
    );
    assert_eq!(open_a.unwrap().role.to_string(), "user");
    assert_eq!(open_b.unwrap().role.to_string(), "user");
    let callback_revision = user.updated_at.timestamp_micros();
    query("UPDATE users SET language = 'ru', theme = 'frappe' WHERE id = ?1")
        .bind(user.id)
        .execute(&db)
        .await
        .unwrap();
    let before = identity.current_profile(user.id).await.unwrap();
    let updated = identity
        .update_settings(user.id, Some("en"), Some("mocha"))
        .await
        .unwrap();
    assert_eq!(updated.language, "en");
    assert_eq!(updated.theme, "mocha");
    assert!(
        identity
            .change_language(user.id, "ru", callback_revision)
            .await
            .is_err()
    );
    assert_eq!(
        identity
            .current_profile(user.id)
            .await
            .unwrap()
            .first_name
            .as_deref(),
        Some("Profile")
    );
    assert_eq!(before.theme, "frappe");

    assert!(matches!(
        identity.current_profile(91_000_000_000_099).await,
        Err(IdentityError::Unauthorized)
    ));
    query("INSERT INTO users (id, username, first_name, role, language, theme, created_at, updated_at) VALUES (91000000000020, 'before', 'Before', 'restricted', 'ru', 'frappe', 10, 20)")
        .execute(&db).await.unwrap();
    query("INSERT INTO user_permissions (user_id, plugin, feature, granted_by, grant_source) VALUES (91000000000020, 'dl', 'inline', 91000000000001, 'manual')")
        .execute(&db).await.unwrap();
    let refreshed = identity
        .register(&profile(91_000_000_000_020))
        .await
        .unwrap();
    assert_eq!(refreshed.role.to_string(), "restricted");
    assert_eq!(refreshed.language, "ru");
    assert_eq!(refreshed.theme, "frappe");
    assert_eq!(refreshed.created_at.timestamp_micros(), 10);
    assert_eq!(
        query("SELECT count(*) FROM user_permissions WHERE user_id = ?1")
            .bind(refreshed.id)
            .fetch_one(&db)
            .await
            .unwrap()
            .try_get::<i64, _>(0)
            .unwrap(),
        1
    );

    query("UPDATE users SET ban_until = CAST(unixepoch('subsec') * 1000000 AS INTEGER) + 60000000 WHERE id = ?1")
        .bind(user.id).execute(&db).await.unwrap();
    assert!(matches!(
        identity.current_profile(user.id).await,
        Err(IdentityError::Forbidden)
    ));
    assert!(matches!(
        identity.register(&profile(user.id)).await,
        Err(IdentityError::Forbidden)
    ));

    query("INSERT INTO bot_settings (key, value, updated_at) VALUES ('bot_access_mode', 'approved_only', 1)")
        .execute(&db).await.unwrap();
    let restricted_profile = profile(91_000_000_000_002);
    let (restricted_a, restricted_b) = tokio::join!(
        identity.register(&restricted_profile),
        identity.register(&restricted_profile)
    );
    let restricted = restricted_a.unwrap();
    assert_eq!(restricted_b.unwrap().role.to_string(), "restricted");
    assert_eq!(restricted.role.to_string(), "restricted");
    assert!(matches!(
        identity
            .update_settings(restricted.id, Some("xx"), None)
            .await,
        Err(IdentityError::InvalidLanguage)
    ));
    assert!(matches!(
        identity
            .update_settings(restricted.id, None, Some("nope"))
            .await,
        Err(IdentityError::InvalidTheme)
    ));

    query("UPDATE bot_settings SET value = NULL WHERE key = 'bot_access_mode'")
        .execute(&db)
        .await
        .unwrap();
    assert!(matches!(
        identity.register(&profile(91_000_000_000_003)).await,
        Err(IdentityError::InvalidPolicy)
    ));
    assert!(matches!(
        identity
            .register(&TrustedProfile {
                photo_url: Some("javascript:alert(1)".into()),
                ..profile(91_000_000_000_004)
            })
            .await,
        Err(IdentityError::InvalidProfile)
    ));
}
