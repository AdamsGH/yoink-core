use std::{path::Path, time::Duration};

use yoink_db::{query, raw_sql};
use yoink_identity::{IdentityError, IdentityStore, LanguagePreference};
use yoink_storage::Store;

#[cfg(unix)]
fn secure(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}
#[cfg(not(unix))]
fn secure(_: &Path) {}

type UserState = (
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    Option<i64>,
    i64,
    i64,
    bool,
    Option<String>,
);

async fn state(db: &yoink_db::Database, id: i64) -> UserState {
    let row = query(include_str!("language_state.sql"))
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap();
    (
        row.try_get(0).unwrap(),
        row.try_get(1).unwrap(),
        row.try_get(2).unwrap(),
        row.try_get(3).unwrap(),
        row.try_get(4).unwrap(),
        row.try_get(5).unwrap(),
        row.try_get(6).unwrap(),
        row.try_get(7).unwrap(),
        row.try_get(8).unwrap(),
        row.try_get(9).unwrap(),
    )
}

async fn unchanged(db: &yoink_db::Database, id: i64, before: &UserState) {
    assert_eq!(&state(db, id).await, before);
}

fn assert_non_language_fields_unchanged(before: &UserState, after: &UserState) {
    assert_eq!(before.0, after.0, "username");
    assert_eq!(before.1, after.1, "first_name");
    assert_eq!(before.2, after.2, "role");
    assert_eq!(before.4, after.4, "theme");
    assert_eq!(before.5, after.5, "ban_until");
    assert_eq!(before.6, after.6, "created_at");
    assert_eq!(before.8, after.8, "is_premium");
    assert_eq!(before.9, after.9, "photo_url");
}

#[tokio::test]
async fn language_revision_authorization_and_preservation() {
    let dir = tempfile::tempdir().unwrap();
    secure(dir.path());
    let path = dir.path().join("language.db");
    let migration_store = Store::open(&path, true, Duration::from_secs(2))
        .await
        .unwrap();
    migration_store.migrate().await.unwrap();
    let db = migration_store.database().clone();
    raw_sql(include_str!("language_users.sql"))
        .execute(&db)
        .await
        .unwrap();

    let identity = IdentityStore::new(db.clone(), Some(42), false);
    let original_owner = state(&db, 42).await;
    let initial = identity.language_preference(42).await.unwrap().unwrap();
    let first = identity
        .change_language(42, "ru", initial.revision)
        .await
        .unwrap();
    assert_eq!(first.code, "ru");
    assert!(first.revision > initial.revision);

    let stale_before = state(&db, 42).await;
    assert!(matches!(
        identity.change_language(42, "en", initial.revision).await,
        Err(IdentityError::LanguageConflict)
    ));
    unchanged(&db, 42, &stale_before).await;
    let second = identity
        .change_language(42, "en", first.revision)
        .await
        .unwrap();
    let third = identity
        .change_language(42, "en", second.revision)
        .await
        .unwrap();
    assert!(
        third.revision > second.revision,
        "same-choice update must advance revision"
    );
    let aba_before = state(&db, 42).await;
    assert!(matches!(
        identity.change_language(42, "ru", second.revision).await,
        Err(IdentityError::LanguageConflict)
    ));
    unchanged(&db, 42, &aba_before).await;

    for (code, revision, expected) in [
        ("de", third.revision, IdentityError::InvalidLanguage),
        ("fr", third.revision, IdentityError::InvalidLanguage),
        ("en", -1, IdentityError::LanguageConflict),
    ] {
        let before = state(&db, 42).await;
        assert_eq!(
            identity
                .change_language(42, code, revision)
                .await
                .unwrap_err(),
            expected
        );
        unchanged(&db, 42, &before).await;
    }

    // Correctly configure each actor as its own owner so authorization reaches role/ban/revision checks.
    for (id, expected) in [
        (44_i64, IdentityError::LanguageDenied),
        (46, IdentityError::LanguageDenied),
    ] {
        let actor = IdentityStore::new(db.clone(), Some(id), false);
        let before = state(&db, id).await;
        assert_eq!(
            actor.change_language(id, "ru", before.7).await.unwrap_err(),
            expected
        );
        unchanged(&db, id, &before).await;
    }
    let expired = IdentityStore::new(db.clone(), Some(47), false);
    let expired_before = state(&db, 47).await;
    let permitted = expired
        .change_language(47, "ru", expired_before.7)
        .await
        .unwrap();
    assert_eq!(permitted.code, "ru");
    assert!(permitted.revision > expired_before.7);

    let overflow = IdentityStore::new(db.clone(), Some(45), false);
    let overflow_before = state(&db, 45).await;
    assert_eq!(
        overflow
            .change_language(45, "ru", i64::MAX)
            .await
            .unwrap_err(),
        IdentityError::LanguageRevisionOverflow
    );
    unchanged(&db, 45, &overflow_before).await;

    let non_owner_before = state(&db, 43).await;
    let ordinary = identity
        .change_language(43, "en", non_owner_before.7)
        .await
        .unwrap();
    assert_eq!(ordinary.code, "en");
    assert!(ordinary.revision > non_owner_before.7);
    assert_non_language_fields_unchanged(&non_owner_before, &state(&db, 43).await);
    assert_eq!(
        identity.change_language(43, "ru", non_owner_before.7).await,
        Err(IdentityError::LanguageConflict)
    );
    let pending_before = state(&db, 48).await;
    assert_eq!(
        identity.change_language(48, "ru", pending_before.7).await,
        Err(IdentityError::LanguageDenied)
    );
    unchanged(&db, 48, &pending_before).await;
    assert_eq!(identity.language_preference(999).await.unwrap(), None);
    assert_eq!(
        identity.language_preference(0).await.unwrap_err(),
        IdentityError::InvalidId
    );
    assert_eq!(identity.language_preference(i64::MAX).await.unwrap(), None);

    raw_sql(include_str!("language_unknown_code.sql"))
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(
        identity
            .language_preference(43)
            .await
            .unwrap()
            .unwrap()
            .code,
        "xx"
    );
    let unchanged_user = state(&db, 43).await;
    assert_eq!(unchanged_user.2, "user");
    assert!(unchanged_user.8);
    assert_eq!(unchanged_user.6, 1700000000000000);
    let others_before = state(&db, 43).await;
    let _ = identity
        .change_language(42, "ru", third.revision)
        .await
        .unwrap();
    unchanged(&db, 43, &others_before).await;
    let own = state(&db, 42).await;
    assert_non_language_fields_unchanged(&original_owner, &own);

    drop(identity);
    drop(expired);
    drop(overflow);
    migration_store.close().await.unwrap();
    drop(db);
    drop(migration_store);
    let reopened = Store::open(&path, false, Duration::from_secs(2))
        .await
        .unwrap();
    let persisted = IdentityStore::new(reopened.database().clone(), Some(42), false)
        .language_preference(42)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        persisted,
        LanguagePreference {
            code: "ru".into(),
            revision: own.7
        }
    );
    assert!(
        persisted.revision > third.revision,
        "successful post-ABA write must fence stale callbacks after reopen"
    );
    reopened.close().await.unwrap();
}
