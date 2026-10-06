use std::time::Duration;
use yoink_db::{query, raw_sql};
use yoink_domain::ContentMetadata;
use yoink_identity::{
    IdentityStore,
    admin::AdminError,
    nsfw::{NsfwImport, NsfwRuleInput, RuleKind},
};
use yoink_storage::Store;

async fn fixture() -> (tempfile::TempDir, Store, IdentityStore) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Store::open(&dir.path().join("nsfw.db"), true, Duration::from_secs(2))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    raw_sql(include_str!("nsfw_fixture.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let identities = IdentityStore::new(store.database().clone(), Some(42), false);
    (dir, store, identities)
}
#[tokio::test]
async fn nsfw_live_rules_group_policy_and_private_blur() {
    let (_dir, store, identities) = fixture().await;
    let source = "https://source.example/content-page.html";
    let metadata = vec![ContentMetadata {
        title: "Fixture sensitivecontent sample".into(),
        ..Default::default()
    }];
    assert!(
        identities
            .content_policy(44, -100, source, &metadata)
            .await
            .unwrap()
            .blocked
    );
    assert!(
        !identities
            .content_policy(44, -100, source, &[])
            .await
            .unwrap()
            .blocked
    );
    assert!(
        identities
            .content_policy(44, 44, source, &metadata)
            .await
            .unwrap()
            .spoiler
    );
    query(include_str!("nsfw_change_blur.sql"))
        .bind(false)
        .execute(store.database())
        .await
        .unwrap();
    assert!(
        !identities
            .content_policy(44, 44, source, &metadata)
            .await
            .unwrap()
            .spoiler
    );
    query(include_str!("nsfw_change_group.sql"))
        .bind(true)
        .execute(store.database())
        .await
        .unwrap();
    let allowed = identities
        .content_policy(44, -100, source, &metadata)
        .await
        .unwrap();
    assert!(!allowed.blocked && !allowed.spoiler);
    let (keywords, rev) = identities.nsfw_list(43, RuleKind::Keyword).await.unwrap();
    assert_eq!(keywords[0].note.as_deref(), Some("imported keyword"));
    identities
        .nsfw_mutate(43, RuleKind::Keyword, Some(keywords[0].id), None, Some(rev))
        .await
        .unwrap();
    assert!(
        !identities
            .content_policy(44, 44, source, &metadata)
            .await
            .unwrap()
            .spoiler
    );
    assert!(
        identities
            .content_policy(44, -100, "https://sub.legacy.example/x", &[])
            .await
            .is_ok()
    );
    assert!(
        identities
            .content_policy(44, -999, source, &[])
            .await
            .is_err()
    );
    raw_sql(include_str!("nsfw_bad_rule.sql"))
        .execute(store.database())
        .await
        .unwrap();
    assert!(
        identities
            .content_policy(44, 44, source, &[])
            .await
            .is_err()
    );
}
#[tokio::test]
async fn nsfw_management_auth_revisions_import_atomicity_and_preservation() {
    let (_dir, store, identities) = fixture().await;
    for actor in [44, 45] {
        assert!(identities.nsfw_list(actor, RuleKind::Domain).await.is_err());
    }
    let input = NsfwRuleInput {
        domain: Some("www.NEW.example".into()),
        note: Some("new note".into()),
        ..Default::default()
    };
    let row = identities
        .nsfw_mutate(43, RuleKind::Domain, None, Some(&input), Some(0))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.domain.as_deref(), Some("new.example"));
    assert!(row.created_at.timestamp() > 0);
    assert_eq!(
        identities
            .nsfw_mutate(43, RuleKind::Domain, None, Some(&input), Some(0))
            .await
            .unwrap_err(),
        AdminError::Conflict
    );
    let (rows, rev) = identities.nsfw_list(43, RuleKind::Domain).await.unwrap();
    assert_eq!(rev, 1);
    assert_eq!(rows.len(), 2);
    let import = NsfwImport {
        domains: vec![
            input.clone(),
            NsfwRuleInput {
                domain: Some("third.example".into()),
                ..Default::default()
            },
        ],
        keywords: vec![],
    };
    assert_eq!(
        identities
            .nsfw_import(43, &import, Some(rev))
            .await
            .unwrap(),
        (1, 0)
    );
    let bad = NsfwImport {
        domains: vec![
            NsfwRuleInput {
                domain: Some("fourth.example".into()),
                ..Default::default()
            },
            NsfwRuleInput {
                domain: Some("bad/path".into()),
                ..Default::default()
            },
        ],
        keywords: vec![],
    };
    assert!(identities.nsfw_import(43, &bad, None).await.is_err());
    let (domains, keywords, revision) = identities.nsfw_export(43).await.unwrap();
    assert_eq!(domains.len(), 3);
    assert_eq!(revision, 2);
    assert_eq!(keywords.len(), 1);
    assert!(
        domains
            .iter()
            .any(|r| r.note.as_deref() == Some("imported note"))
    );
    raw_sql(include_str!("nsfw_demote.sql"))
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        identities
            .nsfw_mutate(43, RuleKind::Domain, Some(row.id), None, None)
            .await
            .unwrap_err(),
        AdminError::Forbidden
    );
    raw_sql(include_str!("nsfw_bad_revision.sql"))
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        identities
            .nsfw_mutate(42, RuleKind::Domain, None, Some(&input), None)
            .await
            .unwrap_err(),
        AdminError::Unavailable
    );
}
