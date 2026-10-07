use std::{path::PathBuf, sync::Arc, time::Duration};

use yoink_domain::DeliveryTarget;
use yoink_downloads::{
    Delivery, DeliveryArtifact, DeliveryFuture, DownloadConfig, DownloadError,
    DownloadNetworkConfig, DownloadService,
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
use yoink_downloads::cookie_web::{CookieInput, CookieSubmit};
fn submission(token: String) -> CookieSubmit {
    serde_json::from_value(serde_json::json!({"token":token,"cookies":{"example.com":[{"domain":".example.com","path":"/","name":"sid","value":"secret","httpOnly":true}]}})).unwrap()
}
#[tokio::test]
async fn cookie_web_issue_replay_expiry_revocation_and_atomic_retry() {
    let (_dir, store, service) = setup().await;
    service.install_cookie_web_schema().await.unwrap();
    let first = service
        .issue_cookie_token(OWNER as i64, 1000)
        .await
        .unwrap();
    let second = service
        .issue_cookie_token(OWNER as i64, 1000)
        .await
        .unwrap();
    service
        .submit_cookie_token(&submission(first.token), 1001)
        .await
        .unwrap();
    let mut body = submission(second.token);
    body.cookies.get_mut("example.com").unwrap()[0].value = "bad\nvalue".into();
    assert!(matches!(
        service.submit_cookie_token(&body, 1001).await,
        Err(DownloadError::InvalidInput)
    ));
    body.cookies.get_mut("example.com").unwrap()[0].value = "secret".into();
    service.submit_cookie_token(&body, 1001).await.unwrap();
    assert!(service.submit_cookie_token(&body, 1001).await.is_err());
    assert_eq!(
        service
            .web_cookie_list(OWNER as i64, 1001)
            .await
            .unwrap()
            .len(),
        1
    );
    let expired = service
        .issue_cookie_token(OWNER as i64, 1000)
        .await
        .unwrap();
    assert!(
        service
            .submit_cookie_token(&submission(expired.token), 1600)
            .await
            .is_err()
    );
    let revoked = service
        .issue_cookie_token(OWNER as i64, 2000)
        .await
        .unwrap();
    service
        .revoke_cookie_tokens(OWNER as i64, 2000)
        .await
        .unwrap();
    assert!(
        service
            .submit_cookie_token(&submission(revoked.token), 2001)
            .await
            .is_err()
    );
    let row = yoink_db::query("SELECT count(*) AS n FROM cookie_web_tokens")
        .fetch_one(store.database())
        .await
        .unwrap();
    assert_eq!(row.try_get::<i64, _>("n").unwrap(), 0);
}
#[tokio::test]
async fn cookie_web_concurrent_consume_single_winner_and_owned_mutation() {
    let (_dir, _store, service) = setup().await;
    service.install_cookie_web_schema().await.unwrap();
    let token = service
        .issue_cookie_token(OWNER as i64, 1000)
        .await
        .unwrap();
    let body = submission(token.token);
    let (a, b) = tokio::join!(
        service.submit_cookie_token(&body, 1001),
        service.submit_cookie_token(&body, 1001)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let row = service
        .web_cookie_list(OWNER as i64, 1001)
        .await
        .unwrap()
        .remove(0);
    assert!(
        !service
            .web_cookie_delete(FOREIGN as i64, Some(row.id), None, 1001)
            .await
            .unwrap()
    );
    assert!(
        service
            .web_cookie_delete(OWNER as i64, Some(row.id), None, 1001)
            .await
            .unwrap()
    );
}
#[tokio::test]
async fn cookie_web_capacity_and_ban_do_not_consume() {
    let (_dir, store, service) = setup().await;
    service.install_cookie_web_schema().await.unwrap();
    let service = service.with_network_config(
        DownloadNetworkConfig::new(None, vec![])
            .unwrap()
            .with_personal_cookie_limits(yoink_downloads::PersonalCookieLimits {
                domains: 1,
                actor_bytes: 512,
                global_bytes: 512,
            })
            .unwrap(),
    );
    service
        .upload_web_cookie(
            OWNER as i64,
            &CookieInput {
                domain: "other.com".into(),
                content:
                    "# Netscape HTTP Cookie File\nother.com\tFALSE\t/\tFALSE\t0\tsid\tsecret\n"
                        .into(),
            },
            1000,
        )
        .await
        .unwrap();
    let token = service
        .issue_cookie_token(OWNER as i64, 1000)
        .await
        .unwrap();
    let body = submission(token.token);
    assert!(matches!(
        service.submit_cookie_token(&body, 1001).await,
        Err(DownloadError::PersonalCookieCapacity)
    ));
    service
        .web_cookie_delete(OWNER as i64, None, Some("other.com"), 1001)
        .await
        .unwrap();
    yoink_db::query("UPDATE users SET ban_until=?1 WHERE id=?2")
        .bind(chrono::DateTime::from_timestamp(2000, 0).unwrap())
        .bind(OWNER as i64)
        .execute(store.database())
        .await
        .unwrap();
    assert!(service.submit_cookie_token(&body, 1001).await.is_err());
    yoink_db::query("UPDATE users SET ban_until=NULL WHERE id=?1")
        .bind(OWNER as i64)
        .execute(store.database())
        .await
        .unwrap();
    service.submit_cookie_token(&body, 1001).await.unwrap();
}
async fn reopen(directory: &tempfile::TempDir) -> (Arc<Store>, DownloadService) {
    let store = Arc::new(
        Store::open(
            &directory.path().join("cookies.db"),
            false,
            Duration::from_secs(2),
        )
        .await
        .unwrap(),
    );
    let identities = Arc::new(IdentityStore::new(
        store.database().clone(),
        Some(OWNER as i64),
        false,
    ));
    let executor = Executor::new(ExecutorConfig {
        program: PathBuf::from("/bin/false"),
        args: vec![],
        workspace_root: directory.path().join("workspace"),
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
    let svc = DownloadService::new_with_network_config(
        store.clone(),
        identities,
        executor,
        Arc::new(NoDelivery),
        DownloadConfig::default(),
        DownloadNetworkConfig::new(None, vec![]).unwrap(),
    )
    .unwrap();
    (store, svc)
}
#[tokio::test]
async fn cookie_web_restart_keeps_only_hashed_usable_tokens_and_consumption() {
    let (dir, store, svc) = setup().await;
    svc.install_cookie_web_schema().await.unwrap();
    let token = svc.issue_cookie_token(OWNER as i64, 1000).await.unwrap();
    let row = yoink_db::query("SELECT token_hash FROM cookie_web_tokens")
        .fetch_one(store.database())
        .await
        .unwrap();
    let stored: String = row.try_get("token_hash").unwrap();
    assert_eq!(stored.len(), 64);
    assert_ne!(stored, token.token);
    drop(svc);
    store.close().await.unwrap();
    drop(store);
    let (store, svc) = reopen(&dir).await;
    let body = submission(token.token);
    svc.submit_cookie_token(&body, 1001).await.unwrap();
    drop(svc);
    store.close().await.unwrap();
    drop(store);
    let (_store, svc) = reopen(&dir).await;
    assert!(svc.submit_cookie_token(&body, 1001).await.is_err());
    assert_eq!(
        svc.web_cookie_list(OWNER as i64, 1001).await.unwrap().len(),
        1
    );
}
#[tokio::test]
async fn cookie_web_multi_domain_capacity_rolls_back_every_write() {
    let (_dir, store, svc) = setup().await;
    svc.install_cookie_web_schema().await.unwrap();
    let token = svc.issue_cookie_token(OWNER as i64, 1000).await.unwrap();
    let body: CookieSubmit =
        serde_json::from_value(serde_json::json!({"token":token.token,"cookies":{
 "a.com":[{"domain":"a.com","path":"/","name":"sid","value":"secret"}],
 "b.com":[{"domain":"b.com","path":"/","name":"sid","value":"secret"}]}}))
        .unwrap();
    let narrow = svc.clone().with_network_config(
        DownloadNetworkConfig::new(None, vec![])
            .unwrap()
            .with_personal_cookie_limits(yoink_downloads::PersonalCookieLimits {
                domains: 1,
                actor_bytes: 512,
                global_bytes: 512,
            })
            .unwrap(),
    );
    assert!(matches!(
        narrow.submit_cookie_token(&body, 1001).await,
        Err(DownloadError::PersonalCookieCapacity)
    ));
    assert!(
        svc.web_cookie_list(OWNER as i64, 1001)
            .await
            .unwrap()
            .is_empty()
    );
    svc.submit_cookie_token(&body, 1001).await.unwrap();
    assert_eq!(
        svc.web_cookie_list(OWNER as i64, 1001).await.unwrap().len(),
        2
    );
    assert_eq!(
        yoink_db::query("SELECT count(*) AS n FROM cookie_web_tokens")
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get::<i64, _>("n")
            .unwrap(),
        0
    );
}
#[tokio::test]
async fn cookie_web_metadata_preserved_and_pool_admin_grants_fresh() {
    let (_dir, store, svc) = setup().await;
    svc.install_cookie_web_schema().await.unwrap();
    let input = CookieInput {
        domain: "example.com".into(),
        content: "# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tFALSE\t0\tsid\tsecret\n"
            .into(),
    };
    svc.upload_web_cookie(OWNER as i64, &input, 1000)
        .await
        .unwrap();
    let old = svc
        .web_cookie_list(OWNER as i64, 1001)
        .await
        .unwrap()
        .remove(0);
    yoink_db::query("UPDATE cookies SET label='Imported account',avatar_url='https://example.com/avatar' WHERE id=?1").bind(old.id).execute(store.database()).await.unwrap();
    svc.upload_web_cookie(OWNER as i64, &input, 1002)
        .await
        .unwrap();
    let new = svc
        .web_cookie_list(OWNER as i64, 1002)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(old.id, new.id);
    assert_eq!(old.created_at, new.created_at);
    assert_eq!(new.label.as_deref(), Some("Imported account"));
    assert!(
        svc.web_pool_upload(FOREIGN as i64, &input, 1000)
            .await
            .is_err()
    );
    let id = svc
        .web_pool_upload(OWNER as i64, &input, 1000)
        .await
        .unwrap();
    assert_eq!(
        id,
        svc.web_pool_upload(OWNER as i64, &input, 1001)
            .await
            .unwrap()
    );
    assert!(
        svc.web_cookie_list(FOREIGN as i64, 1001)
            .await
            .unwrap()
            .is_empty()
    );
    yoink_db::query("INSERT INTO user_permissions(user_id,plugin,feature,granted_by,granted_at,expires_at) VALUES(?1,'dl','shared_cookies',?2,?3,?4)").bind(FOREIGN as i64).bind(OWNER as i64).bind(chrono::DateTime::from_timestamp(1000,0).unwrap()).bind(chrono::DateTime::from_timestamp(1100,0).unwrap()).execute(store.database()).await.unwrap();
    let inherited = svc.web_cookie_list(FOREIGN as i64, 1001).await.unwrap();
    assert_eq!(inherited.len(), 1);
    assert!(inherited[0].inherited);
    assert!(
        svc.web_cookie_list(FOREIGN as i64, 1100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !svc.web_cookie_delete(FOREIGN as i64, Some(id), None, 1001)
            .await
            .unwrap()
    );
    assert!(
        svc.web_cookie_scoped_list(FOREIGN as i64, 1, 1001)
            .await
            .is_err()
    );
    assert!(svc.web_pool_delete(FOREIGN as i64, id, 1001).await.is_err());
    assert!(svc.web_pool_delete(OWNER as i64, id, 1001).await.unwrap());
}
#[test]
fn cookie_web_browser_format_rejects_cross_domain_and_controls() {
    let mut body = submission("a".repeat(64));
    body.cookies.get_mut("example.com").unwrap()[0].domain = "attacker.com".into();
    assert!(body.inputs().is_err());
    let mut body = submission("a".repeat(64));
    body.cookies.get_mut("example.com").unwrap()[0].expiration = Some(f64::INFINITY);
    assert!(body.inputs().is_err());
    let svcbody = submission("a".repeat(64));
    assert!(
        svcbody.inputs().unwrap()[0]
            .content
            .contains("#HttpOnly_.example.com")
    );
}
use std::sync::atomic::{AtomicUsize, Ordering};
use yoink_downloads::cookie_oauth::{
    OAuthCredential, OAuthCredentialContent, OAuthProvider, ProviderDevice, ProviderFuture,
    ProviderPoll,
};
struct FixtureOAuth {
    polls: AtomicUsize,
}
impl OAuthProvider for FixtureOAuth {
    fn start(&self) -> ProviderFuture<'_, ProviderDevice> {
        Box::pin(async {
            Ok(ProviderDevice {
                device_code: "private-device-code".into(),
                verification_url: "https://www.google.com/device".into(),
                user_code: "USER-CODE".into(),
                expires_in: 1800,
                interval: 5,
            })
        })
    }
    fn poll<'a>(&'a self, _: &'a str, now: i64) -> ProviderFuture<'a, ProviderPoll> {
        Box::pin(async move {
            let n = self.polls.fetch_add(1, Ordering::SeqCst);
            Ok(if n == 0 {
                ProviderPoll::Pending
            } else {
                ProviderPoll::Ready(OAuthCredential {
                    access_token: "private-access".into(),
                    refresh_token: "private-refresh".into(),
                    expires_at: chrono::DateTime::from_timestamp(now + 3600, 0)
                        .unwrap()
                        .to_rfc3339(),
                })
            })
        })
    }
    fn refresh<'a>(
        &'a self,
        c: &'a OAuthCredential,
        now: i64,
    ) -> ProviderFuture<'a, OAuthCredential> {
        Box::pin(async move {
            Ok(OAuthCredential {
                access_token: "rotated-access".into(),
                refresh_token: c.refresh_token.clone(),
                expires_at: chrono::DateTime::from_timestamp(now + 3600, 0)
                    .unwrap()
                    .to_rfc3339(),
            })
        })
    }
}
#[tokio::test]
async fn cookie_web_oauth_owned_durable_sessions_capacity_retry_and_refresh_cas() {
    let (dir, store, svc) = setup().await;
    svc.install_cookie_web_schema().await.unwrap();
    let provider = Arc::new(FixtureOAuth {
        polls: AtomicUsize::new(0),
    });
    let svc = svc.with_cookie_oauth_provider(provider.clone());
    let start = svc.oauth_start(OWNER as i64, 1000).await.unwrap();
    assert_eq!(start.interval, 5);
    assert_eq!(start.expires_in, 1800);
    assert!(
        svc.oauth_poll(FOREIGN as i64, &start.session_id, 1000)
            .await
            .is_err()
    );
    assert_eq!(provider.polls.load(Ordering::SeqCst), 0);
    assert_eq!(
        svc.oauth_poll(OWNER as i64, &start.session_id, 1000)
            .await
            .unwrap()
            .status,
        "pending"
    );
    assert_eq!(
        svc.oauth_poll(OWNER as i64, &start.session_id, 1001)
            .await
            .unwrap()
            .status,
        "pending"
    );
    assert_eq!(provider.polls.load(Ordering::SeqCst), 1);
    drop(svc);
    store.close().await.unwrap();
    drop(store);
    let (_store, svc) = reopen(&dir).await;
    let svc = svc.with_cookie_oauth_provider(provider.clone());
    let narrow = svc.clone().with_network_config(
        DownloadNetworkConfig::new(None, vec![])
            .unwrap()
            .with_personal_cookie_limits(yoink_downloads::PersonalCookieLimits {
                domains: 1,
                actor_bytes: 1,
                global_bytes: 1,
            })
            .unwrap(),
    );
    assert!(matches!(
        narrow
            .oauth_poll(OWNER as i64, &start.session_id, 1005)
            .await,
        Err(DownloadError::PersonalCookieCapacity)
    ));
    assert_eq!(
        svc.oauth_poll(OWNER as i64, &start.session_id, 1006)
            .await
            .unwrap()
            .status,
        "ok"
    );
    assert_eq!(provider.polls.load(Ordering::SeqCst), 2);
    assert_eq!(
        svc.oauth_poll(OWNER as i64, &start.session_id, 1007)
            .await
            .unwrap()
            .status,
        "expired"
    );
    let (reference, credential) = svc
        .oauth_credential(OWNER as i64, 1007)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reference.kind, "youtube_tv_oauth_v1");
    assert_eq!(credential.access_token, "private-access");
    assert!(
        svc.oauth_credential(FOREIGN as i64, 1007)
            .await
            .unwrap()
            .is_none()
    );
    let refreshed = svc
        .oauth_refresh(OWNER as i64, &reference, 1008)
        .await
        .unwrap();
    assert_ne!(reference.cookie.version(), refreshed.cookie.version());
    assert!(matches!(
        svc.oauth_refresh(OWNER as i64, &reference, 1009).await,
        Err(DownloadError::ChangedCredentials)
    ));
    let metadata = svc.web_cookie_list(OWNER as i64, 1009).await.unwrap();
    assert!(metadata[0].is_oauth);
    let start = svc.oauth_start(OWNER as i64, 2000).await.unwrap();
    assert_eq!(
        svc.oauth_poll(OWNER as i64, &start.session_id, 3800)
            .await
            .unwrap()
            .status,
        "expired"
    );
}
#[tokio::test]
async fn cookie_web_oauth_imported_content_typed_and_netscape_upload_stays_strict() {
    let (_dir, store, svc) = setup().await;
    let content = "__oauth2__{\"access_token\":\"imported-access\",\"refresh_token\":\"imported-refresh\",\"expires_at\":\"2025-01-01T00:00:00+00:00\"}";
    yoink_db::query("INSERT INTO cookies(user_id,domain,content,is_valid,is_pool,created_at,updated_at,label) VALUES(?1,'youtube.com',?2,1,0,?3,?3,'Imported label')").bind(OWNER as i64).bind(content).bind(chrono::DateTime::from_timestamp(1000,0).unwrap()).execute(store.database()).await.unwrap();
    let (r, c) = svc
        .oauth_credential(OWNER as i64, 1001)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.cookie.owner_id(), OWNER);
    assert_eq!(c.refresh_token, "imported-refresh");
    assert_eq!(format!("{c:?}"), "YoutubeOAuth([REDACTED])");
    let id = i64::from(r.cookie.id());
    assert!(matches!(
        svc.web_cookie_validate(OWNER as i64, id, 1001).await,
        Err(DownloadError::UnsupportedAuthentication)
    ));
    let (after, credential) = svc
        .oauth_credential(OWNER as i64, 1001)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after, r);
    assert_eq!(credential, c);
    let row =
        yoink_db::query("SELECT content,is_valid,updated_at,validated_at FROM cookies WHERE id=?1")
            .bind(id)
            .fetch_one(store.database())
            .await
            .unwrap();
    assert_eq!(row.try_get::<String, _>("content").unwrap(), content);
    assert!(row.try_get::<bool, _>("is_valid").unwrap());
    assert_eq!(
        row.try_get::<chrono::DateTime<chrono::Utc>, _>("updated_at")
            .unwrap()
            .timestamp(),
        1000
    );
    assert!(
        row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("validated_at")
            .unwrap()
            .is_none()
    );
    assert!(
        svc.upload_web_cookie(
            OWNER as i64,
            &CookieInput {
                domain: "youtube.com".into(),
                content: content.into()
            },
            1001
        )
        .await
        .is_err()
    );
    assert!(OAuthCredential::decode("__oauth2__{}").is_err());
    let provider = Arc::new(FixtureOAuth {
        polls: AtomicUsize::new(0),
    });
    let svc = svc.with_cookie_oauth_provider(provider);
    svc.oauth_refresh(OWNER as i64, &r, 1002).await.unwrap();
    let metadata = svc.web_cookie_list(OWNER as i64, 1002).await.unwrap();
    assert_eq!(metadata[0].label.as_deref(), Some("Imported label"));
}
use yoink_downloads::cookie_web_live::{AccountInfo, AccountProvider, AccountResult};
struct FixtureAccount;
impl AccountProvider for FixtureAccount {
    fn fetch<'a>(&'a self, _: &'a str, _: &'a str, _: i64) -> ProviderFuture<'a, AccountResult> {
        Box::pin(async {
            Ok(AccountResult {
                info: Some(AccountInfo {
                    label: "Verified account".into(),
                    avatar_url: Some("https://example.com/avatar".into()),
                }),
                updates: std::collections::BTreeMap::from([(
                    "sid".into(),
                    "rotated-secret".into(),
                )]),
            })
        })
    }
}
#[tokio::test]
async fn cookie_web_live_validation_rotation_labels_and_authority() {
    let (_dir, store, svc) = setup().await;
    let svc = svc.with_cookie_account_provider(Arc::new(FixtureAccount));
    let input = CookieInput {
        domain: "example.com".into(),
        content:
            "# Netscape HTTP Cookie File\n#HttpOnly_.example.com\tTRUE\t/\tFALSE\t0\tsid\tsecret\n"
                .into(),
    };
    svc.upload_web_cookie(OWNER as i64, &input, 1000)
        .await
        .unwrap();
    let id = svc.web_cookie_list(OWNER as i64, 1000).await.unwrap()[0].id;
    assert!(
        svc.web_cookie_validate(FOREIGN as i64, id, 1001)
            .await
            .is_err()
    );
    svc.web_cookie_validate(OWNER as i64, id, 1001)
        .await
        .unwrap();
    let metadata = svc
        .web_cookie_list(OWNER as i64, 1001)
        .await
        .unwrap()
        .remove(0);
    assert!(metadata.is_valid);
    assert_eq!(metadata.validated_at.unwrap().timestamp(), 1001);
    let content: String = yoink_db::query("SELECT content FROM cookies WHERE id=?1")
        .bind(id)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get("content")
        .unwrap();
    assert!(content.contains("rotated-secret"));
    assert!(content.contains("#HttpOnly_"));
    let pool = svc
        .web_pool_upload(OWNER as i64, &input, 1000)
        .await
        .unwrap();
    yoink_db::query("UPDATE cookies SET label='Old account' WHERE id=?1")
        .bind(pool)
        .execute(store.database())
        .await
        .unwrap();
    assert!(
        svc.web_pool_refresh_labels(FOREIGN as i64, 1001)
            .await
            .is_err()
    );
    assert_eq!(
        svc.web_pool_refresh_labels(OWNER as i64, 1001)
            .await
            .unwrap(),
        1
    );
    let row = svc
        .web_cookie_scoped_list(OWNER as i64, 2, 1001)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(row.id, pool);
    assert_eq!(row.label.as_deref(), Some("Verified account"));
    assert_eq!(
        svc.web_pool_refresh_labels(OWNER as i64, 1002)
            .await
            .unwrap(),
        0
    );
}
struct BlockingRefresh {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    inner: FixtureOAuth,
}
impl OAuthProvider for BlockingRefresh {
    fn start(&self) -> ProviderFuture<'_, ProviderDevice> {
        self.inner.start()
    }
    fn poll<'a>(&'a self, c: &'a str, now: i64) -> ProviderFuture<'a, ProviderPoll> {
        self.inner.poll(c, now)
    }
    fn refresh<'a>(
        &'a self,
        c: &'a OAuthCredential,
        now: i64,
    ) -> ProviderFuture<'a, OAuthCredential> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            self.inner.refresh(c, now).await
        })
    }
}
#[tokio::test]
async fn cookie_web_oauth_refresh_network_unlocked_and_fresh_ban_and_content_cas() {
    let (_dir, store, svc) = setup().await;
    svc.install_cookie_web_schema().await.unwrap();
    let imported = "__oauth2__{\"access_token\":\"imported-access\",\"refresh_token\":\"imported-refresh\",\"expires_at\":\"2099-01-01T00:00:00Z\"}";
    yoink_db::query("INSERT INTO cookies(user_id,domain,content,is_valid,is_pool,created_at,updated_at) VALUES(?1,'youtube.com',?2,1,0,?3,?3)").bind(OWNER as i64).bind(imported).bind(chrono::DateTime::from_timestamp(1000,0).unwrap()).execute(store.database()).await.unwrap();
    let provider = Arc::new(BlockingRefresh {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        inner: FixtureOAuth {
            polls: AtomicUsize::new(0),
        },
    });
    let svc = svc.with_cookie_oauth_provider(provider.clone());
    let (reference, _) = svc
        .oauth_credential(OWNER as i64, 1001)
        .await
        .unwrap()
        .unwrap();
    let task = {
        let svc = svc.clone();
        let reference = reference.clone();
        tokio::spawn(async move { svc.oauth_refresh(OWNER as i64, &reference, 1002).await })
    };
    tokio::time::timeout(Duration::from_secs(5), provider.entered.notified())
        .await
        .unwrap();
    // Mutation must complete while provider is blocked: no network-under-transaction.
    tokio::time::timeout(
        Duration::from_secs(1),
        yoink_db::query("UPDATE users SET ban_until=?1 WHERE id=?2")
            .bind(chrono::DateTime::from_timestamp(2000, 0).unwrap())
            .bind(OWNER as i64)
            .execute(store.database()),
    )
    .await
    .unwrap()
    .unwrap();
    provider.release.notify_one();
    assert!(matches!(task.await.unwrap(), Err(DownloadError::Denied)));
    yoink_db::query("UPDATE users SET ban_until=NULL WHERE id=?1")
        .bind(OWNER as i64)
        .execute(store.database())
        .await
        .unwrap();
    let task = {
        let svc = svc.clone();
        let reference = reference.clone();
        tokio::spawn(async move { svc.oauth_refresh(OWNER as i64, &reference, 1003).await })
    };
    tokio::time::timeout(Duration::from_secs(5), provider.entered.notified())
        .await
        .unwrap();
    yoink_db::query("UPDATE cookies SET content=?1 WHERE user_id=?2")
        .bind(imported.replace("imported-access", "newer-access"))
        .bind(OWNER as i64)
        .execute(store.database())
        .await
        .unwrap();
    provider.release.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(DownloadError::ChangedCredentials)
    ));
    let (_, c) = svc
        .oauth_credential(OWNER as i64, 1004)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(c.access_token, "newer-access");
}
#[test]
fn cookie_web_oauth_bounds_and_config() {
    use yoink_downloads::cookie_oauth::OAuthProviderConfig;
    assert!(OAuthProviderConfig::new(Duration::ZERO).is_err());
    assert!(OAuthProviderConfig::new(Duration::from_secs(61)).is_err());
    assert_eq!(
        OAuthProviderConfig::default().timeout(),
        Duration::from_secs(15)
    );
    let value = OAuthCredential {
        access_token: "a".repeat(16384),
        refresh_token: "b".repeat(16384),
        expires_at: "2099-01-01T00:00:00Z".into(),
    };
    assert!(value.encode().is_ok());
    let bad = OAuthCredential {
        access_token: "a".repeat(16385),
        ..value
    };
    assert!(bad.encode().is_err());
    assert!(OAuthCredential::decode(&format!("__oauth2__{}", " ".repeat(65536))).is_err());
}
#[tokio::test]
async fn cookie_web_imported_canonical_duplicates_preserve_ids_and_count_physical_bytes() {
    let (_dir, store, svc) = setup().await;
    svc.install_cookie_web_schema().await.unwrap();
    let input = CookieInput {
        domain: "example.com".into(),
        content: "# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tFALSE\t0\tsid\tsecret\n"
            .into(),
    };
    for domain in ["example.com", ".example.com"] {
        yoink_db::query("INSERT INTO cookies(user_id,domain,content,is_valid,is_pool,created_at,updated_at,label) VALUES(?1,?2,?3,1,0,?4,?4,'Imported label')").bind(OWNER as i64).bind(domain).bind(&input.content).bind(chrono::DateTime::from_timestamp(1000,0).unwrap()).execute(store.database()).await.unwrap();
    }
    let ids = svc
        .web_cookie_list(OWNER as i64, 1000)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect::<Vec<_>>();
    let narrow = svc.clone().with_network_config(
        DownloadNetworkConfig::new(None, vec![])
            .unwrap()
            .with_personal_cookie_limits(yoink_downloads::PersonalCookieLimits {
                domains: 2,
                actor_bytes: 160,
                global_bytes: 160,
            })
            .unwrap(),
    );
    let token = svc.issue_cookie_token(OWNER as i64, 1000).await.unwrap();
    let mut body = submission(token.token);
    body.cookies.get_mut("example.com").unwrap()[0].value = "long-secret".repeat(20);
    assert!(matches!(
        narrow.submit_cookie_token(&body, 1001).await,
        Err(DownloadError::PersonalCookieCapacity)
    ));
    svc.submit_cookie_token(&body, 1001).await.unwrap();
    let new = svc.web_cookie_list(OWNER as i64, 1001).await.unwrap();
    assert_eq!(new.into_iter().map(|r| r.id).collect::<Vec<_>>(), ids);
}
#[tokio::test]
async fn cookie_web_telegram_origin_links_and_private_token_authority() {
    let (_dir, _store, svc) = setup().await;
    svc.install_cookie_web_schema().await.unwrap();
    for bad in [
        "http://example.com/",
        "https://user:secret@example.com/",
        "https://example.com/?token=secret",
        "https://example.com/path",
        "https://example.com/#fragment",
    ] {
        assert!(svc.clone().with_cookie_web_origin(bad).is_err());
    }
    let svc = svc.with_cookie_web_origin("https://example.com/").unwrap();
    assert_eq!(
        svc.cookie_web_link("cookies").unwrap().as_str(),
        "https://example.com/cookies"
    );
    let private = DeliveryTarget::new(BOT, OWNER as i64, None, None).unwrap();
    assert!(
        svc.issue_telegram_cookie_token(OWNER, private)
            .await
            .is_ok()
    );
    let foreign = DeliveryTarget::new(BOT, FOREIGN as i64, None, None).unwrap();
    assert!(
        svc.issue_telegram_cookie_token(OWNER, foreign)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn cookie_web_issuance_capacity_preserves_existing_tokens_and_sessions() {
    let (_dir, _store, svc) = setup().await;
    svc.install_cookie_web_schema().await.unwrap();
    let first = svc.issue_cookie_token(OWNER as i64, 1000).await.unwrap();
    for _ in 1..128 {
        svc.issue_cookie_token(OWNER as i64, 1000).await.unwrap();
    }
    assert!(matches!(
        svc.issue_cookie_token(OWNER as i64, 1000).await,
        Err(DownloadError::Capacity)
    ));
    svc.submit_cookie_token(&submission(first.token), 1001)
        .await
        .unwrap();
    assert!(svc.issue_cookie_token(OWNER as i64, 1001).await.is_ok());
    let svc = svc.with_cookie_oauth_provider(Arc::new(FixtureOAuth {
        polls: AtomicUsize::new(0),
    }));
    let first = svc.oauth_start(OWNER as i64, 1000).await.unwrap();
    for _ in 1..8 {
        svc.oauth_start(OWNER as i64, 1000).await.unwrap();
    }
    assert!(matches!(
        svc.oauth_start(OWNER as i64, 1000).await,
        Err(DownloadError::Capacity)
    ));
    assert_eq!(
        svc.oauth_poll(OWNER as i64, &first.session_id, 1001)
            .await
            .unwrap()
            .status,
        "pending"
    );
}

struct BlockingPoll {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    returned: tokio::sync::Notify,
    calls: AtomicUsize,
    inner: FixtureOAuth,
}
impl BlockingPoll {
    fn new() -> Self {
        Self {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            returned: tokio::sync::Notify::new(),
            calls: AtomicUsize::new(0),
            inner: FixtureOAuth {
                polls: AtomicUsize::new(1),
            },
        }
    }
}
impl OAuthProvider for BlockingPoll {
    fn start(&self) -> ProviderFuture<'_, ProviderDevice> {
        self.inner.start()
    }
    fn poll<'a>(&'a self, code: &'a str, now: i64) -> ProviderFuture<'a, ProviderPoll> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            self.release.notified().await;
            let ready = self.inner.poll(code, now).await;
            self.returned.notify_one();
            ready
        })
    }
    fn refresh<'a>(
        &'a self,
        c: &'a OAuthCredential,
        now: i64,
    ) -> ProviderFuture<'a, OAuthCredential> {
        self.inner.refresh(c, now)
    }
}
async fn notified(n: &tokio::sync::Notify) {
    tokio::time::timeout(Duration::from_secs(5), n.notified())
        .await
        .unwrap();
}

#[tokio::test]
async fn cookie_web_oauth_success_survives_db_contention_overlap_generation_and_request_abort() {
    let (_dir, store, svc) = setup().await;
    svc.install_cookie_web_schema().await.unwrap();
    let provider = Arc::new(BlockingPoll::new());
    let svc = svc.with_cookie_oauth_provider(provider.clone());
    let start = svc.oauth_start(OWNER as i64, 1000).await.unwrap();
    let request = {
        let svc = svc.clone();
        let session = start.session_id.clone();
        tokio::spawn(async move { svc.oauth_poll(OWNER as i64, &session, 1000).await })
    };
    notified(&provider.entered).await;
    // Occupy the serialized DB while the consumed Ready result arrives.
    let conn = store.database().connection().await.unwrap();
    provider.release.notify_one();
    notified(&provider.returned).await;
    // Past the old provider timeout+5 lease, no second provider call is accepted.
    assert!(matches!(
        svc.oauth_poll(OWNER as i64, &start.session_id, 1100).await,
        Err(DownloadError::ChangedCredentials)
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    // Simulate a newer Pending generation (e.g. a legacy poll owner).
    yoink_db::query("UPDATE cookie_oauth_sessions SET generation=generation+1,next_poll_at=1105")
        .execute(&*conn)
        .await
        .unwrap();
    request.abort();
    assert!(matches!(request.await, Err(e) if e.is_cancelled()));
    let drain = {
        let svc = svc.clone();
        tokio::spawn(async move { svc.drain_cookie_oauth().await })
    };
    tokio::task::yield_now().await;
    assert!(!drain.is_finished());
    drop(conn);
    drain.await.unwrap();
    let (_, credential) = svc
        .oauth_credential(OWNER as i64, 1100)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(credential.access_token, "private-access");
    assert!(matches!(
        svc.oauth_poll(OWNER as i64, &start.session_id, 1101).await,
        Err(DownloadError::Capacity)
    ));
}

#[tokio::test]
async fn cookie_web_oauth_inflight_expiry_cleanup_and_finite_result_retention() {
    let (_dir, store, svc) = setup().await;
    svc.install_cookie_web_schema().await.unwrap();
    let provider = Arc::new(BlockingPoll::new());
    let svc = svc.with_cookie_oauth_provider(provider.clone());
    let start = svc.oauth_start(OWNER as i64, 1000).await.unwrap();
    yoink_db::query("UPDATE cookie_oauth_sessions SET expires_at=1001")
        .execute(store.database())
        .await
        .unwrap();
    // Force capacity rejection after successful provider consumption.
    let svc = svc.with_network_config(
        DownloadNetworkConfig::new(None, vec![])
            .unwrap()
            .with_personal_cookie_limits(yoink_downloads::PersonalCookieLimits {
                domains: 1,
                actor_bytes: 1,
                global_bytes: 1,
            })
            .unwrap(),
    );
    let request = {
        let svc = svc.clone();
        let session = start.session_id.clone();
        tokio::spawn(async move { svc.oauth_poll(OWNER as i64, &session, 1000).await })
    };
    notified(&provider.entered).await;
    svc.cleanup_cookie_oauth(1002).await.unwrap();
    let n = yoink_db::query("SELECT count(*) AS n FROM cookie_oauth_sessions")
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get::<i64, _>("n")
        .unwrap();
    assert_eq!(n, 1);
    provider.release.notify_one();
    assert!(matches!(
        request.await.unwrap(),
        Err(DownloadError::PersonalCookieCapacity)
    ));
    let row = yoink_db::query("SELECT ready_content,expires_at FROM cookie_oauth_sessions")
        .fetch_one(store.database())
        .await
        .unwrap();
    assert!(
        row.try_get::<Option<String>, _>("ready_content")
            .unwrap()
            .is_some()
    );
    let expiry = row.try_get::<i64, _>("expires_at").unwrap();
    // Retrying staged success cannot renew its result-retention window.
    assert!(matches!(
        svc.oauth_poll(OWNER as i64, &start.session_id, 1100).await,
        Err(DownloadError::PersonalCookieCapacity)
    ));
    assert_eq!(
        yoink_db::query("SELECT expires_at FROM cookie_oauth_sessions")
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get::<i64, _>("expires_at")
            .unwrap(),
        expiry
    );
    svc.cleanup_cookie_oauth(expiry).await.unwrap();
    assert_eq!(
        svc.oauth_poll(OWNER as i64, &start.session_id, expiry)
            .await
            .unwrap()
            .status,
        "expired"
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    svc.drain_cookie_oauth().await;
}

#[tokio::test]
async fn cookie_web_oauth_ready_rechecks_current_ban_role_session_owner_and_deletion() {
    for fence in ["ban", "role", "foreign", "delete", "staged"] {
        let (_dir, store, svc) = setup().await;
        svc.install_cookie_web_schema().await.unwrap();
        let provider = Arc::new(BlockingPoll::new());
        let svc = svc.with_cookie_oauth_provider(provider.clone());
        // Use non-owner actor so configured-owner role bypass does not mask the test.
        let actor = FOREIGN as i64;
        let start = svc.oauth_start(actor, 1000).await.unwrap();
        let request = {
            let svc = svc.clone();
            let session = start.session_id.clone();
            tokio::spawn(async move { svc.oauth_poll(actor, &session, 1000).await })
        };
        notified(&provider.entered).await;
        match fence {
            "ban" => {
                yoink_db::query("UPDATE users SET ban_until=?1 WHERE id=?2")
                    .bind(chrono::DateTime::from_timestamp(2000, 0).unwrap())
                    .bind(actor)
                    .execute(store.database())
                    .await
                    .unwrap();
            }
            "role" => {
                yoink_db::query("UPDATE users SET role='restricted' WHERE id=?1")
                    .bind(actor)
                    .execute(store.database())
                    .await
                    .unwrap();
            }
            "foreign" => {
                yoink_db::query("UPDATE cookie_oauth_sessions SET user_id=?1")
                    .bind(OWNER as i64)
                    .execute(store.database())
                    .await
                    .unwrap();
            }
            "delete" => {
                yoink_db::query("DELETE FROM cookie_oauth_sessions")
                    .execute(store.database())
                    .await
                    .unwrap();
            }
            "staged" => {
                let content = OAuthCredential {
                    access_token: "already-staged".into(),
                    refresh_token: "staged-refresh".into(),
                    expires_at: "2099-01-01T00:00:00Z".into(),
                }
                .encode()
                .unwrap();
                yoink_db::query("UPDATE cookie_oauth_sessions SET ready_content=?1")
                    .bind(content)
                    .execute(store.database())
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        provider.release.notify_one();
        let result = request.await.unwrap();
        if fence == "staged" {
            assert_eq!(result.unwrap().status, "ok");
            assert_eq!(
                svc.oauth_credential(actor, 1001)
                    .await
                    .unwrap()
                    .unwrap()
                    .1
                    .access_token,
                "already-staged"
            );
        } else {
            assert!(result.is_err(), "{fence}");
            let n = yoink_db::query("SELECT count(*) AS n FROM cookies")
                .fetch_one(store.database())
                .await
                .unwrap()
                .try_get::<i64, _>("n")
                .unwrap();
            assert_eq!(n, 0, "{fence}");
        }
        svc.drain_cookie_oauth().await;
    }
}
