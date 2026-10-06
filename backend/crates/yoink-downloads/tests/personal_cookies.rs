use std::{path::PathBuf, sync::Arc, time::Duration};

use yoink_domain::DeliveryTarget;
use yoink_downloads::{
    CookieDomain, Delivery, DeliveryArtifact, DeliveryFuture, DownloadConfig, DownloadError,
    DownloadNetworkConfig, DownloadService, PersonalCookieStatus,
};
use yoink_executor::{Executor, ExecutorConfig};
use yoink_identity::IdentityStore;
use yoink_storage::Store;

const OWNER: u64 = 42;
const FOREIGN: u64 = 43;
const BOT: i64 = 123456;

struct NoDelivery;
impl Delivery for NoDelivery {
    fn bot_id(&self) -> i64 {
        BOT
    }
    fn deliver(&self, _: DeliveryTarget, _: bool, _: Vec<DeliveryArtifact>) -> DeliveryFuture<'_> {
        panic!("personal cookie service must not deliver")
    }
}

fn target(chat: i64, thread: Option<i64>, bot: i64) -> DeliveryTarget {
    DeliveryTarget::new(bot, chat, thread, None).unwrap()
}

async fn setup() -> (tempfile::TempDir, Arc<Store>, DownloadService) {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let store = Arc::new(
        Store::open(
            &directory.path().join("cookies.db"),
            true,
            Duration::from_secs(2),
        )
        .await
        .unwrap(),
    );
    store.migrate().await.unwrap();
    yoink_db::raw_sql(include_str!("admission_users.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let identities = Arc::new(IdentityStore::new(
        store.database().clone(),
        Some(OWNER as i64),
        false,
    ));
    let executor = Executor::new(ExecutorConfig {
        program: PathBuf::from("/bin/false"),
        args: vec![],
        workspace_root: workspace,
        timeout: Duration::from_secs(30),
        processing_timeout: Duration::from_secs(20),
        socket_timeout: Duration::from_secs(5),
        max_artifact_bytes: 1024 * 1024,
        max_workspace_bytes: 2 * 1024 * 1024,
        max_output_bytes: 1024 * 1024,
        max_artifacts: 1,
        allowed_proxy_origins: vec![],
        allowed_private_origins: vec![],
    })
    .unwrap();
    let service = DownloadService::new_with_network_config(
        store.clone(),
        identities,
        executor,
        Arc::new(NoDelivery),
        DownloadConfig::default(),
        DownloadNetworkConfig::new(None, vec![]).unwrap(),
    )
    .unwrap();
    (directory, store, service)
}

async fn add_cookie(
    store: &Store,
    id: i64,
    user: i64,
    domain: &str,
    content: &str,
    valid: bool,
    pool: bool,
) {
    yoink_db::query(include_str!("sql/insert_cookie.sql"))
        .bind(id)
        .bind(user)
        .bind(domain)
        .bind(content)
        .bind(valid)
        .bind(pool)
        .execute(store.database())
        .await
        .unwrap();
}

fn jar() -> &'static str {
    "# Netscape HTTP Cookie File\n#HttpOnly_.example.com\tTRUE\t/\tFALSE\t0\tsid\tsecret\nexample.com\tFALSE\t/\tFALSE\t0\tother\tvalue\n"
}

#[tokio::test]
async fn owner_roles_reads_metadata_pages_and_allows_negative_cursor() {
    let (_dir, store, service) = setup().await;
    let own = target(OWNER as i64, None, BOT);
    for role in ["user", "restricted"] {
        yoink_db::query(include_str!("sql/update_user_role.sql"))
            .bind(OWNER as i64)
            .bind(role)
            .bind(Option::<i64>::None)
            .execute(store.database())
            .await
            .unwrap();
        add_cookie(
            &store,
            1,
            OWNER as i64,
            "BAD DOMAIN!",
            "not a saved cookie format",
            true,
            false,
        )
        .await;
        let page = service.personal_cookies(OWNER, own, None).await.unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].domain, None);
        assert_eq!(page.next_after, None);
        yoink_db::query(include_str!("sql/delete_user_cookies.sql"))
            .bind(OWNER as i64)
            .execute(store.database())
            .await
            .unwrap();
    }
    yoink_db::query(include_str!("sql/update_user_role.sql"))
        .bind(OWNER as i64)
        .bind("user")
        .bind(Option::<i64>::None)
        .execute(store.database())
        .await
        .unwrap();
    for id in -3..=8 {
        add_cookie(
            &store,
            id,
            OWNER as i64,
            &format!("{id}.example.com"),
            "malformed, but not read on list",
            true,
            false,
        )
        .await;
    }
    add_cookie(
        &store,
        9,
        OWNER as i64,
        "excluded.invalid",
        jar(),
        false,
        false,
    )
    .await;
    add_cookie(&store, 10, OWNER as i64, "pool.invalid", jar(), true, true).await;
    let first = service.personal_cookies(OWNER, own, None).await.unwrap();
    assert_eq!(first.entries.len(), 10);
    assert_eq!(first.next_after, Some(6));
    let second = service
        .personal_cookies(OWNER, own, first.next_after)
        .await
        .unwrap();
    assert_eq!(second.entries.len(), 2);
    assert_eq!(second.next_after, None);
    assert_eq!(
        service
            .personal_cookies(OWNER, own, Some(i64::MIN))
            .await
            .unwrap()
            .entries
            .len(),
        10
    );
}

#[tokio::test]
async fn authorization_denies_bans_roles_and_wrong_delivery_context_for_reads_and_writes() {
    let (_dir, store, service) = setup().await;
    let own = target(OWNER as i64, None, BOT);
    let domain = CookieDomain::parse("example.com").unwrap();
    for (index, role) in ["user", "restricted"].into_iter().enumerate() {
        yoink_db::query(include_str!("sql/update_user_role.sql"))
            .bind(OWNER as i64)
            .bind(role)
            .bind(Option::<i64>::None)
            .execute(store.database())
            .await
            .unwrap();
        assert_eq!(
            service
                .personal_cookies(OWNER, own, None)
                .await
                .unwrap()
                .entries
                .len(),
            0
        );
        assert_eq!(
            service
                .remove_personal_cookies(OWNER, own, index as i32 * 3 + 1, None)
                .await
                .unwrap(),
            Some(0)
        );
        assert_eq!(
            service
                .personal_cookie_upload_allowed(OWNER, own, index as i32 * 3 + 2)
                .await,
            Ok(true)
        );
        assert_eq!(
            service
                .store_personal_cookie(OWNER, own, index as i32 * 3 + 2, &domain, jar())
                .await,
            Ok(Some(2))
        );
        assert_eq!(
            service
                .remove_personal_cookies(OWNER, own, index as i32 * 3 + 3, None)
                .await,
            Ok(Some(1))
        );
    }
    for mutation in [("banned", None), ("user", Some(i64::MAX))] {
        yoink_db::query(include_str!("sql/update_user_role.sql"))
            .bind(OWNER as i64)
            .bind(mutation.0)
            .bind(mutation.1)
            .execute(store.database())
            .await
            .unwrap();
        assert_eq!(
            service.personal_cookies(OWNER, own, None).await,
            Err(DownloadError::Denied)
        );
        assert_eq!(
            service.remove_personal_cookies(OWNER, own, 2, None).await,
            Err(DownloadError::Denied)
        );
        assert_eq!(
            service.personal_cookie_upload_allowed(OWNER, own, 2).await,
            Err(DownloadError::Denied)
        );
        assert_eq!(
            service
                .store_personal_cookie(OWNER, own, 2, &domain, jar())
                .await,
            Err(DownloadError::Denied)
        );
    }
    yoink_db::query(include_str!("sql/update_user_role.sql"))
        .bind(OWNER as i64)
        .bind("user")
        .bind(Option::<i64>::None)
        .execute(store.database())
        .await
        .unwrap();
    for (actor, destination) in [
        (OWNER, target(FOREIGN as i64, None, BOT)),
        (OWNER, target(-10042, None, BOT)),
        (OWNER, target(OWNER as i64, Some(7), BOT)),
        (OWNER, target(OWNER as i64, None, BOT + 1)),
    ] {
        assert_eq!(
            service.personal_cookies(actor, destination, None).await,
            Err(DownloadError::Denied)
        );
        assert_eq!(
            service
                .personal_cookie_status(actor, destination, &domain)
                .await,
            Err(DownloadError::Denied)
        );
        assert_eq!(
            service
                .remove_personal_cookies(actor, destination, 3, None)
                .await,
            Err(DownloadError::Denied)
        );
        assert_eq!(
            service
                .personal_cookie_upload_allowed(actor, destination, 3)
                .await,
            Err(DownloadError::Denied)
        );
        assert_eq!(
            service
                .store_personal_cookie(actor, destination, 3, &domain, jar())
                .await,
            Err(DownloadError::Denied)
        );
    }
}

#[tokio::test]
async fn status_uses_validity_bytes_and_unambiguous_canonical_domain() {
    let (_dir, store, service) = setup().await;
    let own = target(OWNER as i64, None, BOT);
    let domain = CookieDomain::parse("EXAMPLE.COM").unwrap();
    assert_eq!(
        service
            .personal_cookie_status(OWNER, own, &domain)
            .await
            .unwrap(),
        PersonalCookieStatus::Missing
    );
    add_cookie(&store, 1, OWNER as i64, ".Example.com.", jar(), true, false).await;
    assert_eq!(
        service
            .personal_cookie_status(OWNER, own, &domain)
            .await
            .unwrap(),
        PersonalCookieStatus::Stored { entries: 2 }
    );
    yoink_db::query(include_str!("sql/update_cookie_content.sql"))
        .bind(1_i64)
        .bind("é".repeat(yoink_worker_protocol::MAX_COOKIE_JAR_BYTES / 2 + 1))
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        service
            .personal_cookie_status(OWNER, own, &domain)
            .await
            .unwrap(),
        PersonalCookieStatus::Invalid
    );
    yoink_db::query(include_str!("sql/update_cookie_content.sql"))
        .bind(1_i64)
        .bind("invalid")
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        service
            .personal_cookie_status(OWNER, own, &domain)
            .await
            .unwrap(),
        PersonalCookieStatus::Invalid
    );
    add_cookie(&store, 2, OWNER as i64, "example.com", jar(), true, false).await;
    assert_eq!(
        service
            .personal_cookie_status(OWNER, own, &domain)
            .await
            .unwrap(),
        PersonalCookieStatus::Invalid
    );
    yoink_db::query(include_str!("sql/mark_cookie_invalid.sql"))
        .bind(2_i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        service
            .personal_cookie_status(OWNER, own, &domain)
            .await
            .unwrap(),
        PersonalCookieStatus::Invalid
    );
    let invalidated = CookieDomain::parse("invalidated.example").unwrap();
    add_cookie(
        &store,
        3,
        OWNER as i64,
        invalidated.as_str(),
        jar(),
        false,
        false,
    )
    .await;
    assert_eq!(
        service
            .personal_cookie_status(OWNER, own, &invalidated)
            .await
            .unwrap(),
        PersonalCookieStatus::Invalid
    );
}

#[tokio::test]
async fn invalid_delete_message_ids_do_not_mutate_state() {
    let (_dir, store, service) = setup().await;
    let own = target(OWNER as i64, None, BOT);
    for message_id in [0, -1, i32::MIN] {
        assert_eq!(
            service
                .remove_personal_cookies(OWNER, own, message_id, None)
                .await,
            Err(DownloadError::InvalidInput)
        );
    }
    let watermark: i64 = yoink_db::query(include_str!("sql/count_watermarks.sql"))
        .bind(OWNER as i64)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(watermark, 0);
}

#[tokio::test]
async fn upload_preflight_and_storage_validate_content_and_fence_replacements() {
    let (_dir, store, service) = setup().await;
    let own = target(OWNER as i64, None, BOT);
    let domain = CookieDomain::parse(".ExAmPlE.com.").unwrap();
    assert!(
        service
            .personal_cookie_upload_allowed(OWNER, own, 10)
            .await
            .unwrap()
    );
    let watermark: i64 = yoink_db::query(include_str!("sql/count_watermarks.sql"))
        .bind(OWNER as i64)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(watermark, 0);
    for bad in [0, -1, i32::MIN] {
        assert_eq!(
            service
                .personal_cookie_upload_allowed(OWNER, own, bad)
                .await,
            Err(DownloadError::InvalidInput)
        );
        assert_eq!(
            service
                .store_personal_cookie(OWNER, own, bad, &domain, jar())
                .await,
            Err(DownloadError::InvalidInput)
        );
    }
    for bad in ["", "not a cookie"] {
        assert_eq!(
            service
                .store_personal_cookie(OWNER, own, 11, &domain, bad)
                .await,
            Err(DownloadError::InvalidInput)
        );
    }
    let unicode_over = format!(
        "# Netscape HTTP Cookie File\nexample.com\tFALSE\t/\tFALSE\t0\tsid\t{}",
        "é".repeat(yoink_worker_protocol::MAX_COOKIE_JAR_BYTES / 2)
    );
    assert!(unicode_over.len() > yoink_worker_protocol::MAX_COOKIE_JAR_BYTES);
    assert_eq!(
        service
            .store_personal_cookie(OWNER, own, 11, &domain, &unicode_over)
            .await,
        Err(DownloadError::InvalidInput)
    );
    assert_eq!(
        yoink_db::query(include_str!("sql/count_watermarks.sql"))
            .bind(OWNER as i64)
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get::<i64, _>(0)
            .unwrap(),
        0
    );

    add_cookie(&store, 1, OWNER as i64, ".Example.com.", jar(), true, false).await;
    add_cookie(&store, 2, OWNER as i64, "example.com", jar(), true, false).await;
    add_cookie(&store, 3, OWNER as i64, "example.com", jar(), true, true).await;
    add_cookie(&store, 4, FOREIGN as i64, "example.com", jar(), true, false).await;
    add_cookie(&store, 5, OWNER as i64, "other.example", jar(), true, false).await;
    yoink_db::query(include_str!("sql/insert_settings.sql"))
        .bind(OWNER as i64)
        .bind("invalid-quality")
        .bind("avc1")
        .bind("mp4")
        .execute(store.database())
        .await
        .unwrap();
    yoink_db::query(include_str!("sql/insert_history.sql"))
        .bind(OWNER as i64)
        .bind("https://example.com/video")
        .bind("example.com")
        .bind("ok")
        .execute(store.database())
        .await
        .unwrap();
    let first_id = service
        .store_personal_cookie(OWNER, own, 20, &domain, jar())
        .await
        .unwrap();
    assert_eq!(first_id, Some(2));
    assert!(
        !service
            .personal_cookie_upload_allowed(OWNER, own, 20)
            .await
            .unwrap()
    );
    assert_eq!(
        service
            .store_personal_cookie(OWNER, own, 20, &domain, "# Netscape HTTP Cookie File\n")
            .await,
        Err(DownloadError::InvalidInput)
    );
    let second = service
        .store_personal_cookie(OWNER, own, 21, &domain, jar())
        .await
        .unwrap();
    assert_eq!(second, Some(2));
    let rows = yoink_db::query(include_str!("sql/upload_cookie_rows.sql"))
        .bind(OWNER as i64)
        .bind(FOREIGN as i64)
        .bind(domain.as_str())
        .fetch_all(store.database())
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    let personal = rows
        .iter()
        .find(|row| {
            row.try_get::<i64, _>("user_id").unwrap() == OWNER as i64
                && row.try_get::<i64, _>("is_pool").unwrap() == 0
        })
        .unwrap();
    assert!(personal.try_get::<i64, _>("id").unwrap() > 5);
    assert_eq!(
        personal.try_get::<String, _>("domain").unwrap(),
        "example.com"
    );
    assert_eq!(personal.try_get::<String, _>("content").unwrap(), jar());
    let other_domain: i64 = yoink_db::query(include_str!("sql/count_replacement.sql"))
        .bind(5_i64)
        .bind(OWNER as i64)
        .bind("other.example")
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(other_domain, 1);
    let preference: Option<String> = yoink_db::query(include_str!("sql/get_quality.sql"))
        .bind(OWNER as i64)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(preference.as_deref(), Some("invalid-quality"));
    let history: i64 = yoink_db::query(include_str!("sql/count_download_history.sql"))
        .bind(OWNER as i64)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(history, 1);
    assert_eq!(
        service
            .remove_personal_cookies(OWNER, own, 22, Some(&domain))
            .await
            .unwrap(),
        Some(1)
    );
    assert!(
        !service
            .personal_cookie_upload_allowed(OWNER, own, 21)
            .await
            .unwrap()
    );
    assert_eq!(
        service
            .store_personal_cookie(OWNER, own, 21, &domain, jar())
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn deletion_is_scoped_atomic_and_stale_replays_never_delete_replacements() {
    let (_dir, store, service) = setup().await;
    let own = target(OWNER as i64, None, BOT);
    let domain = CookieDomain::parse("example.com").unwrap();
    add_cookie(&store, 1, OWNER as i64, "example.com", jar(), true, false).await;
    add_cookie(&store, 2, OWNER as i64, "other.com", jar(), true, false).await;
    add_cookie(&store, 3, FOREIGN as i64, "example.com", jar(), true, false).await;
    add_cookie(&store, 4, OWNER as i64, "example.com", jar(), true, true).await;
    yoink_db::query(include_str!("sql/insert_settings.sql"))
        .bind(OWNER as i64)
        .bind("invalid-quality")
        .bind("avc1")
        .bind("mp4")
        .execute(store.database())
        .await
        .unwrap();
    yoink_db::query(include_str!("sql/insert_history.sql"))
        .bind(OWNER as i64)
        .bind("https://example.com/video")
        .bind("example.com")
        .bind("ok")
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        service
            .remove_personal_cookies(OWNER, own, 100, Some(&domain))
            .await
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        service
            .remove_personal_cookies(OWNER, own, 100, None)
            .await
            .unwrap(),
        None
    );
    add_cookie(&store, 5, OWNER as i64, "example.com", jar(), true, false).await;
    assert_eq!(
        service
            .remove_personal_cookies(OWNER, own, 100, Some(&domain))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        service
            .remove_personal_cookies(OWNER, own, 99, None)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        service
            .personal_cookie_status(OWNER, own, &domain)
            .await
            .unwrap(),
        PersonalCookieStatus::Stored { entries: 2 }
    );
    assert_eq!(
        service
            .remove_personal_cookies(OWNER, own, 101, None)
            .await
            .unwrap(),
        Some(2)
    );
    assert_eq!(
        service
            .remove_personal_cookies(OWNER, own, 102, Some(&domain))
            .await
            .unwrap(),
        Some(0)
    );
    let counts = yoink_db::query(include_str!("sql/count_cookie_owners.sql"))
        .bind(OWNER as i64)
        .bind(FOREIGN as i64)
        .fetch_one(store.database())
        .await
        .unwrap();
    assert_eq!(counts.try_get::<i64, _>("pool").unwrap(), 1);
    assert_eq!(counts.try_get::<i64, _>("foreign_count").unwrap(), 1);
    assert_eq!(counts.try_get::<i64, _>("own").unwrap(), 1);
    let preference: Option<String> = yoink_db::query(include_str!("sql/get_quality.sql"))
        .bind(OWNER as i64)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(preference.as_deref(), Some("invalid-quality"));
    let history: i64 = yoink_db::query(include_str!("sql/count_download_history.sql"))
        .bind(OWNER as i64)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(history, 1);
    add_cookie(&store, 5, OWNER as i64, "example.com", jar(), true, false).await;
    assert_eq!(
        service
            .remove_personal_cookies(OWNER, own, 101, None)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        service
            .remove_personal_cookies(OWNER, own, 102, None)
            .await
            .unwrap(),
        None
    );
    let database_path = _dir.path().join("cookies.db");
    store.close().await.unwrap();
    drop(service);
    drop(store);
    let reopened = Arc::new(
        Store::open(&database_path, false, Duration::from_secs(2))
            .await
            .unwrap(),
    );
    let identities = Arc::new(IdentityStore::new(
        reopened.database().clone(),
        Some(OWNER as i64),
        false,
    ));
    let executor = Executor::new(ExecutorConfig {
        program: PathBuf::from("/bin/false"),
        args: vec![],
        workspace_root: _dir.path().join("workspace"),
        timeout: Duration::from_secs(30),
        processing_timeout: Duration::from_secs(20),
        socket_timeout: Duration::from_secs(5),
        max_artifact_bytes: 1024 * 1024,
        max_workspace_bytes: 2 * 1024 * 1024,
        max_output_bytes: 1024 * 1024,
        max_artifacts: 1,
        allowed_proxy_origins: vec![],
        allowed_private_origins: vec![],
    })
    .unwrap();
    let reopened_service = DownloadService::new_with_network_config(
        reopened.clone(),
        identities,
        executor,
        Arc::new(NoDelivery),
        DownloadConfig::default(),
        DownloadNetworkConfig::new(None, vec![]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        reopened_service
            .remove_personal_cookies(OWNER, own, 100, Some(&domain))
            .await
            .unwrap(),
        None
    );
    assert!(
        !reopened_service
            .personal_cookie_upload_allowed(OWNER, own, 102)
            .await
            .unwrap()
    );
    assert_eq!(
        reopened_service
            .store_personal_cookie(OWNER, own, 102, &domain, jar())
            .await
            .unwrap(),
        None
    );
    let remains: i64 = yoink_db::query(include_str!("sql/count_replacement.sql"))
        .bind(5_i64)
        .bind(OWNER as i64)
        .bind(domain.as_str())
        .fetch_one(reopened.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(remains, 1);
    let watermark: Option<i64> = yoink_db::query(include_str!("sql/get_watermark.sql"))
        .bind(OWNER as i64)
        .fetch_optional(reopened.database())
        .await
        .unwrap()
        .map(|row| row.try_get(0).unwrap());
    assert_eq!(watermark, Some(102));
}

#[tokio::test]
async fn ordinary_user_capacity_is_atomic_isolated_and_replay_safe() {
    use yoink_downloads::PersonalCookieLimits;
    let (_dir, store, service) = setup().await;
    let service = service.with_network_config(
        DownloadNetworkConfig::new(None, vec![])
            .unwrap()
            .with_personal_cookie_limits(PersonalCookieLimits {
                domains: 1,
                actor_bytes: jar().len() as u64,
                global_bytes: jar().len() as u64 * 2,
            })
            .unwrap(),
    );
    let own = target(FOREIGN as i64, None, BOT);
    let a = CookieDomain::parse("a.example").unwrap();
    let b = CookieDomain::parse("b.example").unwrap();
    let (one, two) = tokio::join!(
        service.store_personal_cookie(FOREIGN, own, 900, &a, jar()),
        service.store_personal_cookie(FOREIGN, own, 901, &b, jar())
    );
    assert!(matches!(one, Ok(Some(_))) || matches!(two, Ok(Some(_))));
    assert_eq!(
        service
            .personal_cookies(FOREIGN, own, None)
            .await
            .unwrap()
            .entries
            .len(),
        1
    );
    assert!(
        service
            .personal_cookies(OWNER, target(OWNER as i64, None, BOT), None)
            .await
            .unwrap()
            .entries
            .is_empty()
    );
    assert_eq!(
        service
            .store_personal_cookie(
                FOREIGN,
                own,
                902,
                &a,
                &format!("{}# extra UTF-8: Ж\n", jar())
            )
            .await,
        Err(DownloadError::PersonalCookieCapacity)
    );
    service
        .remove_personal_cookies(FOREIGN, own, 903, None)
        .await
        .unwrap();
    assert_eq!(
        service
            .store_personal_cookie(FOREIGN, own, 902, &a, jar())
            .await,
        Ok(None)
    );
    assert!(
        service
            .store_personal_cookie(FOREIGN, own, 904, &a, jar())
            .await
            .unwrap()
            .is_some()
    );
    add_cookie(
        &store,
        90000,
        FOREIGN as i64,
        "import.example",
        jar(),
        true,
        false,
    )
    .await;
    add_cookie(
        &store,
        90001,
        OWNER as i64,
        "pool.example",
        jar(),
        true,
        true,
    )
    .await;
    // Lower limits below imported usage: replacement and clear must still work.
    let service = service.with_network_config(
        DownloadNetworkConfig::new(None, vec![])
            .unwrap()
            .with_personal_cookie_limits(PersonalCookieLimits {
                domains: 1,
                actor_bytes: 1,
                global_bytes: 1,
            })
            .unwrap(),
    );
    assert!(
        service
            .store_personal_cookie(FOREIGN, own, 905, &a, jar())
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        service
            .store_personal_cookie(FOREIGN, own, 906, &b, jar())
            .await,
        Err(DownloadError::PersonalCookieCapacity)
    );
    assert_eq!(
        service
            .remove_personal_cookies(FOREIGN, own, 907, None)
            .await,
        Ok(Some(2))
    );
}

#[tokio::test]
async fn global_capacity_races_exclude_pool_and_count_utf8_bytes() {
    use yoink_downloads::PersonalCookieLimits;
    let (_dir, store, service) = setup().await;
    let content = format!("{}# Ж\n", jar());
    let service = service.with_network_config(
        DownloadNetworkConfig::new(None, vec![])
            .unwrap()
            .with_personal_cookie_limits(PersonalCookieLimits {
                domains: 128,
                actor_bytes: 67108864,
                global_bytes: content.len() as u64,
            })
            .unwrap(),
    );
    add_cookie(
        &store,
        90010,
        OWNER as i64,
        "pool.example",
        &content,
        true,
        true,
    )
    .await;
    let domain = CookieDomain::parse("example.com").unwrap();
    let owner = target(OWNER as i64, None, BOT);
    let user = target(FOREIGN as i64, None, BOT);
    let (a, b) = tokio::join!(
        service.store_personal_cookie(OWNER, owner, 1000, &domain, &content),
        service.store_personal_cookie(FOREIGN, user, 1000, &domain, &content)
    );
    assert!(matches!(
        (a, b),
        (Ok(Some(_)), Err(DownloadError::PersonalCookieCapacity))
            | (Err(DownloadError::PersonalCookieCapacity), Ok(Some(_)))
    ));
    service
        .remove_personal_cookies(OWNER, owner, 1001, None)
        .await
        .unwrap();
    service
        .remove_personal_cookies(FOREIGN, user, 1001, None)
        .await
        .unwrap();
    assert!(
        service
            .store_personal_cookie(FOREIGN, user, 1002, &domain, &content)
            .await
            .unwrap()
            .is_some()
    );
    let bigger = format!("{content}# Ж\n");
    assert_eq!(
        service
            .store_personal_cookie(FOREIGN, user, 1003, &domain, &bigger)
            .await,
        Err(DownloadError::PersonalCookieCapacity)
    );
    // Smaller replacement frees global bytes for the other actor.
    assert!(
        service
            .store_personal_cookie(FOREIGN, user, 1004, &domain, jar())
            .await
            .unwrap()
            .is_some()
    );
    service
        .remove_personal_cookies(FOREIGN, user, 1005, None)
        .await
        .unwrap();
    assert!(
        service
            .store_personal_cookie(OWNER, owner, 1006, &domain, jar())
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn ordinary_personal_controls_recheck_role_and_ban_without_owner_exception() {
    let (_dir, store, service) = setup().await;
    let own = target(FOREIGN as i64, None, BOT);
    let domain = CookieDomain::parse("example.com").unwrap();
    assert!(
        service
            .store_personal_cookie(FOREIGN, own, 2000, &domain, jar())
            .await
            .unwrap()
            .is_some()
    );
    for (role, ban) in [
        ("restricted", None),
        ("banned", None),
        ("user", Some(4102444800000000_i64)),
    ] {
        yoink_db::query(include_str!("sql/update_user_role.sql"))
            .bind(FOREIGN as i64)
            .bind(role)
            .bind(ban)
            .execute(store.database())
            .await
            .unwrap();
        assert_eq!(
            service.personal_cookies(FOREIGN, own, None).await,
            Err(DownloadError::Denied)
        );
        assert_eq!(
            service
                .store_personal_cookie(FOREIGN, own, 2001, &domain, jar())
                .await,
            Err(DownloadError::Denied)
        );
        assert_eq!(
            service
                .remove_personal_cookies(FOREIGN, own, 2001, None)
                .await,
            Err(DownloadError::Denied)
        );
        assert_eq!(
            service.proxy_settings(FOREIGN, own).await,
            Err(DownloadError::Denied)
        );
        assert_eq!(
            service.change_proxy_setting(FOREIGN, own, false, 0).await,
            Err(DownloadError::Denied)
        );
    }
}

#[tokio::test]
async fn malformed_imports_consume_each_personal_limit_without_counting_pools() {
    use yoink_downloads::PersonalCookieLimits;

    // Exercise each bound independently, so one working bound cannot mask another.
    for bound in ["domains", "actor_bytes", "global_bytes"] {
        let (_dir, store, service) = setup().await;
        let own = target(FOREIGN as i64, None, BOT);
        let foreign_content = "foreign malformed Ж🍪";
        let multibyte_content = "malformed personal Ж🍪";
        let pool_content = "pool Ж🍪".repeat(4096);
        assert!(CookieDomain::parse("not/a/domain").is_err());
        let seeds = [
            (91000, FOREIGN, "not/a/domain", jar(), true, false),
            (
                91001,
                FOREIGN,
                "invalid-content.example",
                "not Netscape",
                true,
                false,
            ),
            (91002, FOREIGN, "invalid-flag.example", jar(), false, false),
            (
                91003,
                FOREIGN,
                "multibyte.example",
                multibyte_content,
                true,
                false,
            ),
            (
                91004,
                OWNER,
                "foreign-import.example",
                foreign_content,
                false,
                false,
            ),
            (
                91005,
                FOREIGN,
                "own-pool.example",
                pool_content.as_str(),
                false,
                true,
            ),
            (
                91006,
                OWNER,
                "foreign-pool.example",
                pool_content.as_str(),
                true,
                true,
            ),
        ];
        for &(id, actor, domain, content, valid, pool) in &seeds {
            add_cookie(&store, id, actor as i64, domain, content, valid, pool).await;
        }
        let actor_bytes = (jar().len() * 2 + "not Netscape".len() + multibyte_content.len()) as u64;
        let global_bytes = actor_bytes + foreign_content.len() as u64;
        assert!(multibyte_content.len() > multibyte_content.chars().count());
        let domain = CookieDomain::parse("new.example").unwrap();
        let usage = yoink_db::query(include_str!("../src/personal_cookies_usage.sql"))
            .bind(FOREIGN as i64)
            .bind(domain.as_str())
            .fetch_one(store.database())
            .await
            .unwrap();
        assert_eq!(usage.try_get::<i64, _>("actor_domains").unwrap(), 4);
        assert_eq!(
            usage.try_get::<i64, _>("actor_bytes").unwrap(),
            actor_bytes as i64
        );
        assert_eq!(
            usage.try_get::<i64, _>("global_bytes").unwrap(),
            global_bytes as i64
        );
        assert_eq!(usage.try_get::<i64, _>("replaced_domains").unwrap(), 0);
        assert_eq!(usage.try_get::<i64, _>("replaced_bytes").unwrap(), 0);
        let limits = PersonalCookieLimits {
            domains: if bound == "domains" { 4 } else { 128 },
            actor_bytes: if bound == "actor_bytes" {
                actor_bytes + jar().len() as u64 - 1
            } else {
                67108864
            },
            global_bytes: if bound == "global_bytes" {
                global_bytes + jar().len() as u64 - 1
            } else {
                1073741824
            },
        };
        let service = service.with_network_config(
            DownloadNetworkConfig::new(None, vec![])
                .unwrap()
                .with_personal_cookie_limits(limits)
                .unwrap(),
        );
        assert_eq!(
            service
                .store_personal_cookie(FOREIGN, own, 4000, &domain, jar())
                .await,
            Err(DownloadError::PersonalCookieCapacity),
            "{bound}"
        );
        // A rejected growth event retains its watermark, not a credential write.
        assert_eq!(
            service
                .store_personal_cookie(FOREIGN, own, 4000, &domain, jar())
                .await,
            Ok(None)
        );
        let watermark = yoink_db::query(include_str!("sql/get_watermark.sql"))
            .bind(FOREIGN as i64)
            .fetch_one(store.database())
            .await
            .unwrap();
        assert_eq!(watermark.try_get::<i64, _>(0).unwrap(), 4000);
        let after_rejection = yoink_db::query(include_str!("../src/personal_cookies_usage.sql"))
            .bind(FOREIGN as i64)
            .bind(domain.as_str())
            .fetch_one(store.database())
            .await
            .unwrap();
        for key in [
            "actor_domains",
            "actor_bytes",
            "global_bytes",
            "replaced_domains",
            "replaced_bytes",
        ] {
            assert_eq!(
                after_rejection.try_get::<i64, _>(key).unwrap(),
                usage.try_get::<i64, _>(key).unwrap()
            );
        }
        for raw_domain in [
            "invalid-content.example",
            "invalid-flag.example",
            "multibyte.example",
        ] {
            assert_eq!(
                service
                    .personal_cookie_status(FOREIGN, own, &CookieDomain::parse(raw_domain).unwrap())
                    .await,
                Ok(PersonalCookieStatus::Invalid)
            );
        }
        for &(id, actor, raw_domain, content, _, pool) in &seeds {
            let rows = yoink_db::query(include_str!("sql/upload_cookie_rows.sql"))
                .bind(FOREIGN as i64)
                .bind(OWNER as i64)
                .bind(raw_domain)
                .fetch_all(store.database())
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
            let row = &rows[0];
            assert_eq!(row.try_get::<i64, _>("id").unwrap(), id);
            assert_eq!(row.try_get::<i64, _>("user_id").unwrap(), actor as i64);
            assert_eq!(row.try_get::<String, _>("domain").unwrap(), raw_domain);
            assert!(
                row.try_get::<String, _>("content").unwrap() == content,
                "imported content preserved"
            );
            assert_eq!(row.try_get::<bool, _>("is_pool").unwrap(), pool);
            assert_eq!(row.try_get::<i64, _>("created_at").unwrap(), 1);
            assert_eq!(row.try_get::<i64, _>("updated_at").unwrap(), 1);
        }
        // Clear is permitted even with all three bounds below imported usage.
        let service = service.with_network_config(
            DownloadNetworkConfig::new(None, vec![])
                .unwrap()
                .with_personal_cookie_limits(PersonalCookieLimits {
                    domains: 1,
                    actor_bytes: 1,
                    global_bytes: 1,
                })
                .unwrap(),
        );
        assert_eq!(
            service
                .store_personal_cookie(FOREIGN, own, 4001, &domain, jar())
                .await,
            Err(DownloadError::PersonalCookieCapacity)
        );
        assert_eq!(
            service
                .remove_personal_cookies(FOREIGN, own, 4002, None)
                .await,
            Ok(Some(4))
        );
        let usage = yoink_db::query(include_str!("../src/personal_cookies_usage.sql"))
            .bind(FOREIGN as i64)
            .bind(domain.as_str())
            .fetch_one(store.database())
            .await
            .unwrap();
        assert_eq!(usage.try_get::<i64, _>("actor_domains").unwrap(), 0);
        assert_eq!(usage.try_get::<i64, _>("actor_bytes").unwrap(), 0);
        assert_eq!(
            usage.try_get::<i64, _>("global_bytes").unwrap(),
            foreign_content.len() as i64
        );
        let watermark = yoink_db::query(include_str!("sql/get_watermark.sql"))
            .bind(FOREIGN as i64)
            .fetch_one(store.database())
            .await
            .unwrap();
        assert_eq!(watermark.try_get::<i64, _>(0).unwrap(), 4002);
        // Both pools and the other actor's malformed personal row survive clear.
        for &(id, actor, raw_domain, content, _, _) in &seeds[4..] {
            let row = yoink_db::query(include_str!("sql/upload_cookie_rows.sql"))
                .bind(FOREIGN as i64)
                .bind(OWNER as i64)
                .bind(raw_domain)
                .fetch_one(store.database())
                .await
                .unwrap();
            assert_eq!(row.try_get::<i64, _>("id").unwrap(), id);
            assert_eq!(row.try_get::<i64, _>("user_id").unwrap(), actor as i64);
            assert!(
                row.try_get::<String, _>("content").unwrap() == content,
                "foreign/pool content preserved"
            );
        }
    }
}
