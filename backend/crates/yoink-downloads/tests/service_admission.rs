//! Public service admission against a migrated, isolated native store.
use std::{path::PathBuf, sync::Arc, time::Duration};

use yoink_access::Role;
use yoink_domain::{DeliveryTarget, DownloadParams, DownloadRequest, MediaMode};
use yoink_downloads::{
    Delivery, DeliveryArtifact, DeliveryFuture, DownloadConfig, DownloadService,
    profile::{NetworkContext, ProfileStore},
};
use yoink_executor::{Executor, ExecutorConfig};
use yoink_identity::IdentityStore;
use yoink_storage::Store;

const SOURCE: &str = "http://127.0.0.1:12345/fixture.mp4?source-secret-marker-fake";
const BOT_ID: i64 = 123456;
const ACTOR_ID: u64 = 42;
const IDEMPOTENCY_KEY: &str = "telegram-bot-123456-chat-42-message-2001";

struct PanicDelivery;

impl Delivery for PanicDelivery {
    fn bot_id(&self) -> i64 {
        BOT_ID
    }

    fn deliver(
        &self,
        _target: DeliveryTarget,
        _send_as_file: bool,
        _artifacts: Vec<DeliveryArtifact>,
    ) -> DeliveryFuture<'_> {
        panic!("delivery must not run during admission-only regression")
    }
}

#[tokio::test]
async fn public_submit_admits_native_media_for_user_without_settings_row() {
    let directory = tempfile::tempdir().expect("create isolated admission directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .expect("restrict admission directory permissions");
    }
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).expect("create temporary executor workspace");

    let store = Store::open(
        &directory.path().join("admission.db"),
        true,
        Duration::from_secs(2),
    )
    .await
    .expect("open temporary native store");
    store
        .migrate()
        .await
        .expect("migrate temporary native store");
    let database = store.database().clone();
    yoink_db::raw_sql(include_str!("../../yoink-identity/tests/fixtures.sql"))
        .execute(&database)
        .await
        .expect("seed shared identity fixtures");
    yoink_db::raw_sql(include_str!("admission_users.sql"))
        .execute(&database)
        .await
        .expect("seed test-only admission users");

    let identity_store = Arc::new(IdentityStore::new(database.clone(), None, false));
    let profile_store = ProfileStore::new(database);
    let identity = identity_store
        .snapshot(ACTOR_ID as i64, None)
        .await
        .expect("phase identity snapshot")
        .expect("phase identity snapshot: owner 42 must exist");
    assert_eq!(
        identity.principal.user_id, ACTOR_ID as i64,
        "phase identity snapshot"
    );
    assert_eq!(
        identity.principal.role,
        Role::Owner,
        "phase identity snapshot"
    );

    let preferences = profile_store
        .preferences(ACTOR_ID)
        .await
        .expect("phase profile preferences");
    let preferences = preferences.expect(
        "phase profile preferences: existing user without dl_user_settings must receive defaults",
    );
    assert_eq!(
        preferences.quality, "best",
        "phase profile preferences default values"
    );
    assert!(
        profile_store
            .preferences(43)
            .await
            .expect("phase profile preferences for user 43")
            .is_some(),
        "phase profile preferences: user 43 must exist without a settings row"
    );

    let no_cookie_domains = [];
    let selection = profile_store
        .prepare_network(
            NetworkContext {
                actor_id: ACTOR_ID,
                domain: "127.0.0.1",
                no_cookie_domains: &no_cookie_domains,
                identity: &identity,
                system_proxy: None,
            },
            0,
        )
        .await
        .expect("phase profile prepare_network");
    assert_eq!(
        selection,
        yoink_domain::NetworkSelection::NONE,
        "phase profile prepare_network"
    );

    let executor = Executor::new(ExecutorConfig {
        program: PathBuf::from("/bin/false"),
        args: Vec::new(),
        workspace_root: workspace,
        timeout: Duration::from_secs(30),
        processing_timeout: Duration::from_secs(20),
        socket_timeout: Duration::from_secs(5),
        max_artifact_bytes: 1024 * 1024,
        max_workspace_bytes: 2 * 1024 * 1024,
        max_output_bytes: 1024 * 1024,
        max_artifacts: 1,
        allowed_proxy_origins: Vec::new(),
        allowed_private_origins: Vec::new(),
    })
    .expect("construct admission-only executor");
    let store = Arc::new(store);
    let service = DownloadService::new(
        Arc::clone(&store),
        identity_store,
        executor,
        Arc::new(PanicDelivery),
        DownloadConfig::default(),
    )
    .expect("initialize download service");

    let target = DeliveryTarget::new(BOT_ID, ACTOR_ID as i64, None, Some(2001))
        .expect("valid private delivery target");
    let request = DownloadRequest::new(
        ACTOR_ID,
        SOURCE,
        MediaMode::Video,
        None,
        DownloadParams::default(),
    )
    .expect("valid fake media request")
    .with_target(target)
    .expect("attach private target");

    assert!(
        store
            .replay_network_admission(&request, IDEMPOTENCY_KEY)
            .await
            .expect("phase storage replay")
            .is_none(),
        "phase storage replay: no prior admission should exist"
    );

    let admission = service
        .submit_request(&request, IDEMPOTENCY_KEY)
        .await
        .expect("phase public submit_request must admit missing-preferences user");
    assert!(
        admission.created,
        "phase storage replay/admit: first request must create admission"
    );
}
