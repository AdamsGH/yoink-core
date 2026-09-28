use chrono::{DateTime, Utc};
use thiserror::Error;
use yoink_access::{
    AccessContext, AccessDecision, AccessPolicy, AccessScope, FeatureSpec, Grant, GrantSource,
    Principal, ProviderGrant, Role, check_access, effective_grant,
};
use yoink_db::{Database, query};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupContext {
    pub group_id: i64,
    pub enabled: bool,
    pub role_override: Option<Role>,
    pub auto_grant_role: Role,
    pub thread_allowed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentitySnapshot {
    pub principal: Principal,
    pub language: String,
    pub grants: Vec<Grant>,
    pub providers: Vec<ProviderGrant>,
    pub now: DateTime<Utc>,
    pub group: Option<GroupContext>,
}

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("invalid identity or group identifier")]
    InvalidId,
    #[error("unknown role value in identity data")]
    UnknownRole,
    #[error("identity database read failed")]
    Database,
}

impl From<yoink_db::Error> for IdentityError {
    fn from(_: yoink_db::Error) -> Self {
        Self::Database
    }
}

pub struct IdentityStore {
    database: Database,
    owner_id: Option<i64>,
    insight_enabled: bool,
}

impl IdentityStore {
    pub fn new(database: Database, owner_id: Option<i64>, insight_enabled: bool) -> Self {
        Self {
            database,
            owner_id,
            insight_enabled,
        }
    }

    pub async fn snapshot(
        &self,
        user_id: i64,
        group: Option<(i64, Option<i64>)>,
    ) -> Result<Option<IdentitySnapshot>, IdentityError> {
        if user_id <= 0
            || self.owner_id.is_some_and(|id| id <= 0)
            || group.is_some_and(|(id, thread)| id >= 0 || thread.is_some_and(|v| v <= 0))
        {
            return Err(IdentityError::InvalidId);
        }

        // Hold the connection guard for the full transaction lifetime so all reads
        // observe one consistent database snapshot.
        let mut connection = self.database.connection().await?;
        let transaction = turso::transaction::Transaction::new(
            &mut connection,
            turso::transaction::TransactionBehavior::Deferred,
        )
        .await
        .map_err(|_| IdentityError::Database)?;

        let result = self.read_snapshot(&transaction, user_id, group).await;
        match result {
            Ok(Some(snapshot)) => {
                transaction
                    .commit()
                    .await
                    .map_err(|_| IdentityError::Database)?;
                Ok(Some(snapshot))
            }
            Ok(None) => {
                transaction
                    .rollback()
                    .await
                    .map_err(|_| IdentityError::Database)?;
                Ok(None)
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    async fn read_snapshot(
        &self,
        transaction: &turso::transaction::Transaction<'_>,
        user_id: i64,
        group: Option<(i64, Option<i64>)>,
    ) -> Result<Option<IdentitySnapshot>, IdentityError> {
        let now = query(include_str!("sql/database_now.sql"))
            .fetch_one(&**transaction)
            .await?
            .try_get::<DateTime<Utc>, _>(0)
            .map_err(|_| IdentityError::Database)?;

        let Some(row) = query(include_str!("sql/user.sql"))
            .bind(user_id)
            .fetch_optional(&**transaction)
            .await?
        else {
            return Ok(None);
        };
        let role = parse_role(
            row.try_get::<String, _>(1)
                .map_err(|_| IdentityError::Database)?,
        )?;
        let ban_until = row
            .try_get::<Option<DateTime<Utc>>, _>(3)
            .map_err(|_| IdentityError::Database)?;
        let mut principal = Principal {
            user_id,
            role,
            ban_until,
        };
        let language = row
            .try_get::<String, _>(2)
            .map_err(|_| IdentityError::Database)?;

        // A configured owner is elevated only after global blocking has been checked.
        if !principal.is_blocked(now) && self.owner_id == Some(user_id) {
            principal.role = Role::Owner;
        }

        let grant_rows = query(include_str!("sql/grants.sql"))
            .bind(user_id)
            .fetch_all(&**transaction)
            .await?;
        let mut grants = Vec::with_capacity(grant_rows.len());
        for row in grant_rows {
            grants.push(Grant {
                plugin: row.try_get(0).map_err(|_| IdentityError::Database)?,
                feature: row.try_get(1).map_err(|_| IdentityError::Database)?,
                expires_at: row.try_get(2).map_err(|_| IdentityError::Database)?,
            });
        }

        let mut providers = Vec::new();
        if self.insight_enabled {
            let setting_row = query(include_str!("sql/byok_enabled.sql"))
                .fetch_optional(&**transaction)
                .await?;
            let setting_value = setting_row
                .map(|row| row.try_get::<Option<String>, _>(0))
                .transpose()
                .map_err(|_| IdentityError::Database)?
                .flatten();
            let setting = setting_value.is_some_and(|value| {
                matches!(value.to_lowercase().as_str(), "1" | "true" | "yes" | "on")
            });
            if setting {
                let ready = query(include_str!("sql/byok_ready.sql"))
                    .bind(user_id)
                    .fetch_one(&**transaction)
                    .await?
                    .try_get::<bool, _>(0)
                    .map_err(|_| IdentityError::Database)?;
                if ready {
                    providers.push(ProviderGrant {
                        plugin: "insight".into(),
                        feature: "tldr".into(),
                    });
                }
            }
        }

        let group_context = if let Some((group_id, thread_id)) = group {
            let row = query(include_str!("sql/group_context.sql"))
                .bind(user_id)
                .bind(group_id)
                .fetch_optional(&**transaction)
                .await?
                .ok_or(IdentityError::Database)?;
            let enabled = row.try_get(0).map_err(|_| IdentityError::Database)?;
            let auto_grant_role = parse_role(
                row.try_get::<String, _>(1)
                    .map_err(|_| IdentityError::Database)?,
            )?;
            let role_override = row
                .try_get::<Option<String>, _>(2)
                .map_err(|_| IdentityError::Database)?
                .map(parse_role)
                .transpose()?;
            let threads = query(include_str!("sql/thread_context.sql"))
                .bind(group_id)
                .bind(thread_id)
                .fetch_one(&**transaction)
                .await?;
            let matching = threads
                .try_get::<Option<bool>, _>(0)
                .map_err(|_| IdentityError::Database)?;
            let has_enabled: bool = threads.try_get(1).map_err(|_| IdentityError::Database)?;
            Some(GroupContext {
                group_id,
                enabled,
                role_override,
                auto_grant_role,
                thread_allowed: matching.unwrap_or(!has_enabled),
            })
        } else {
            None
        };

        Ok(Some(IdentitySnapshot {
            principal,
            language,
            grants,
            providers,
            now,
            group: group_context,
        }))
    }
}

fn parse_role(value: String) -> Result<Role, IdentityError> {
    value.parse().map_err(|_| IdentityError::UnknownRole)
}

/// Features registered by the core plugin set. Unknown features never gain
/// access merely because the caller has the owner role.
pub fn registered_features() -> Vec<FeatureSpec> {
    use Role::*;
    [
        ("dl", "inline", Some(User)),
        ("dl", "shared_cookies", Some(Admin)),
        ("stats", "stats", Some(User)),
        ("insight", "summary", None),
        ("insight", "tldr", None),
        ("insight", "search", None),
        ("music", "inline", Some(User)),
        ("inbox", "ingest", Some(User)),
        ("inbox", "classify", None),
        ("inbox", "share", Some(User)),
        ("inbox", "gh_sync", None),
        ("inbox", "gh_organise", None),
        ("inbox", "gh_write", None),
        ("inbox", "admin", Some(Admin)),
    ]
    .into_iter()
    .map(|(plugin, feature, default_min_role)| FeatureSpec {
        plugin: plugin.into(),
        feature: feature.into(),
        default_min_role,
    })
    .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Authorization {
    pub decision: AccessDecision,
    pub grant_source: Option<GrantSource>,
}

impl IdentitySnapshot {
    /// Shared role, scope and feature decision for an already authenticated actor.
    pub fn authorize(&self, policy: &AccessPolicy, feature: Option<(&str, &str)>) -> Authorization {
        let context = match &self.group {
            Some(group) => AccessContext {
                scope: AccessScope::Group,
                group_enabled: group.enabled,
                thread_allowed: group.thread_allowed,
                group_role_override: group.role_override,
                group_auto_grant_role: Some(group.auto_grant_role),
            },
            None => AccessContext {
                scope: AccessScope::Private,
                group_enabled: false,
                thread_allowed: false,
                group_role_override: None,
                group_auto_grant_role: None,
            },
        };
        let mut decision = check_access(&self.principal, policy, &context, self.now);
        if !decision.allowed {
            return Authorization {
                decision,
                grant_source: None,
            };
        }
        let mut grant_source = None;
        if let Some((plugin, feature)) = feature {
            match registered_features()
                .into_iter()
                .find(|spec| spec.plugin == plugin && spec.feature == feature)
            {
                None => {
                    decision.allowed = false;
                    decision.deny_reason = "unknown_feature".into();
                }
                Some(spec) => {
                    let mut principal = self.principal.clone();
                    principal.role = decision.effective_role;
                    grant_source =
                        effective_grant(&principal, &spec, &self.grants, &self.providers, self.now);
                    if grant_source.is_none() {
                        decision.allowed = false;
                        decision.deny_reason = "feature_denied".into();
                    }
                }
            }
        }
        Authorization {
            decision,
            grant_source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::TempDir;
    use yoink_db::raw_sql;

    async fn store(
        insight_enabled: bool,
        owner_id: Option<i64>,
    ) -> (TempDir, Database, IdentityStore) {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let path = dir.path().join("identity.db");
        let db = Database::open(&path, true, Duration::from_secs(2))
            .await
            .unwrap();
        raw_sql(include_str!("sql/test_schema.sql"))
            .execute(&db)
            .await
            .unwrap();
        let identity = IdentityStore::new(db.clone(), owner_id, insight_enabled);
        (dir, db, identity)
    }

    async fn exec(db: &Database, sql: &str) {
        query(sql).execute(db).await.unwrap();
    }

    #[tokio::test]
    async fn reads_identity_grants_ban_and_owner_rules() {
        let (_dir, db, identity) = store(false, Some(1)).await;
        exec(&db, "INSERT INTO users (id, role, language, ban_until) VALUES (1, 'user', 'en', NULL), (2, 'banned', 'ru', 1700000000000000), (3, 'user', 'en', NULL), (4, 'user', 'en', 4102444800000000), (5, 'user', 'en', 1700000000000000)").await;
        exec(&db, "INSERT INTO user_permissions (user_id, plugin, feature, expires_at) VALUES (1, 'dl', 'inline', NULL), (3, 'dl', 'inline', 4102444800000000)").await;

        let owner = identity.snapshot(1, None).await.unwrap().unwrap();
        assert_eq!(owner.principal.role, Role::Owner);
        assert_eq!(owner.grants.len(), 1);
        assert_eq!(owner.grants[0].plugin, "dl");

        let blocked_owner = identity.snapshot(2, None).await.unwrap().unwrap();
        assert_eq!(blocked_owner.principal.role, Role::Banned);
        assert!(blocked_owner.principal.is_blocked(blocked_owner.now));

        let grant = identity.snapshot(3, None).await.unwrap().unwrap();
        assert_eq!(
            grant.grants[0].expires_at,
            Some(DateTime::from_timestamp_micros(4102444800000000).unwrap())
        );
        let currently_banned = identity.snapshot(4, None).await.unwrap().unwrap();
        assert!(currently_banned.principal.is_blocked(currently_banned.now));
        let expired_ban = identity.snapshot(5, None).await.unwrap().unwrap();
        assert!(!expired_ban.principal.is_blocked(expired_ban.now));
    }

    #[tokio::test]
    async fn loads_byok_only_when_globally_enabled_and_ready() {
        let (_dir, db, identity) = store(true, None).await;
        exec(
            &db,
            "INSERT INTO users (id, role, language) VALUES (9, 'user', 'en')",
        )
        .await;
        exec(
            &db,
            "INSERT INTO bot_settings (key, value) VALUES ('insight_byok_enabled', 'yes')",
        )
        .await;
        exec(&db, "INSERT INTO insight_user_byok (user_id, tested_at, test_error) VALUES (9, 1700000000000000, NULL)").await;

        let snapshot = identity.snapshot(9, None).await.unwrap().unwrap();
        assert_eq!(
            snapshot.providers,
            vec![ProviderGrant {
                plugin: "insight".into(),
                feature: "tldr".into()
            }]
        );
        let (_dir, db, disabled) = store(false, None).await;
        exec(
            &db,
            "INSERT INTO users (id, role, language) VALUES (9, 'user', 'en')",
        )
        .await;
        exec(
            &db,
            "INSERT INTO bot_settings (key, value) VALUES ('insight_byok_enabled', 'yes')",
        )
        .await;
        exec(&db, "INSERT INTO insight_user_byok (user_id, tested_at, test_error) VALUES (9, 1700000000000000, NULL)").await;
        assert!(
            disabled
                .snapshot(9, None)
                .await
                .unwrap()
                .unwrap()
                .providers
                .is_empty()
        );
    }

    #[tokio::test]
    async fn loads_group_override_auto_grant_and_thread_policy() {
        let (_dir, db, identity) = store(false, None).await;
        exec(
            &db,
            "INSERT INTO users (id, role, language) VALUES (7, 'restricted', 'en')",
        )
        .await;
        exec(
            &db,
            "INSERT INTO groups (id, enabled, auto_grant_role) VALUES (-10, 1, 'user')",
        )
        .await;
        exec(&db, "INSERT INTO user_group_policies (group_id, user_id, role_override) VALUES (-10, 7, 'moderator')").await;
        exec(&db, "INSERT INTO thread_policies (group_id, thread_id, enabled) VALUES (-10, NULL, 1), (-10, 55, 0), (-10, 88, 1)").await;

        let group = identity
            .snapshot(7, Some((-10, Some(55))))
            .await
            .unwrap()
            .unwrap()
            .group
            .unwrap();
        assert!(group.enabled);
        assert_eq!(group.role_override, Some(Role::Moderator));
        assert_eq!(group.auto_grant_role, Role::User);
        assert!(!group.thread_allowed);
        let inherited = identity
            .snapshot(7, Some((-10, Some(66))))
            .await
            .unwrap()
            .unwrap()
            .group
            .unwrap();
        assert!(!inherited.thread_allowed);
        let unrestricted = identity
            .snapshot(7, Some((-10, Some(77))))
            .await
            .unwrap()
            .unwrap()
            .group
            .unwrap();
        assert!(!unrestricted.thread_allowed);
        let allowed = identity
            .snapshot(7, Some((-10, Some(88))))
            .await
            .unwrap()
            .unwrap()
            .group
            .unwrap();
        assert!(allowed.thread_allowed);
    }

    #[tokio::test]
    async fn group_without_specific_thread_policy_inherits_default_behavior() {
        let (_dir, db, identity) = store(false, None).await;
        exec(
            &db,
            "INSERT INTO users (id, role, language) VALUES (7, 'user', 'en')",
        )
        .await;
        exec(
            &db,
            "INSERT INTO groups (id, enabled, auto_grant_role) VALUES (-10, 1, 'user')",
        )
        .await;
        assert!(
            identity
                .snapshot(7, Some((-10, Some(4))))
                .await
                .unwrap()
                .unwrap()
                .group
                .unwrap()
                .thread_allowed
        );
        exec(
            &db,
            "INSERT INTO thread_policies (group_id, thread_id, enabled) VALUES (-10, NULL, 0)",
        )
        .await;
        assert!(
            identity
                .snapshot(7, Some((-10, Some(4))))
                .await
                .unwrap()
                .unwrap()
                .group
                .unwrap()
                .thread_allowed
        );
    }

    #[tokio::test]
    async fn validates_ids_and_missing_user() {
        let (_dir, _db, identity) = store(false, None).await;
        assert!(matches!(
            identity.snapshot(0, None).await,
            Err(IdentityError::InvalidId)
        ));
        assert!(matches!(
            identity.snapshot(1, Some((10, None))).await,
            Err(IdentityError::InvalidId)
        ));
        assert!(identity.snapshot(123, None).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn byok_queries_project_only_non_secret_fields() {
        let sql = include_str!("sql/byok_ready.sql").to_lowercase();
        assert!(sql.contains("select exists"));
        assert!(!sql.contains("api_key"));
        assert!(!sql.contains("secret"));
    }

    #[test]
    fn catalog_matches_declared_plugin_features_and_minimums() {
        let specs = registered_features();
        assert_eq!(specs.len(), 14);
        let find = |plugin: &str, feature: &str| {
            specs
                .iter()
                .find(|s| s.plugin == plugin && s.feature == feature)
                .unwrap()
        };
        assert_eq!(find("dl", "inline").default_min_role, Some(Role::User));
        assert_eq!(
            find("dl", "shared_cookies").default_min_role,
            Some(Role::Admin)
        );
        assert_eq!(find("stats", "stats").default_min_role, Some(Role::User));
        assert_eq!(find("insight", "summary").default_min_role, None);
        assert_eq!(find("insight", "tldr").default_min_role, None);
        assert_eq!(find("insight", "search").default_min_role, None);
        assert_eq!(find("music", "inline").default_min_role, Some(Role::User));
        assert_eq!(find("inbox", "ingest").default_min_role, Some(Role::User));
        assert_eq!(find("inbox", "classify").default_min_role, None);
        assert_eq!(find("inbox", "share").default_min_role, Some(Role::User));
        assert_eq!(find("inbox", "gh_sync").default_min_role, None);
        assert_eq!(find("inbox", "gh_organise").default_min_role, None);
        assert_eq!(find("inbox", "gh_write").default_min_role, None);
        assert_eq!(find("inbox", "admin").default_min_role, Some(Role::Admin));
        assert!(
            !specs
                .iter()
                .any(|s| s.plugin == "dl" && s.feature == "download")
        );
        assert!("made_up".parse::<Role>().is_err());
        assert_eq!("restricted".parse::<Role>().unwrap(), Role::Restricted);
    }

    fn fixture(role: Role) -> IdentitySnapshot {
        IdentitySnapshot {
            principal: Principal {
                user_id: 42,
                role,
                ban_until: None,
            },
            language: "en".into(),
            grants: vec![],
            providers: vec![],
            now: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            group: None,
        }
    }
    fn policy() -> AccessPolicy {
        AccessPolicy {
            min_role: Role::User,
            scopes: vec![AccessScope::Private, AccessScope::Group],
            check_group_enabled: true,
            check_thread_policy: true,
        }
    }
    #[test]
    fn group_grants_reach_feature_policy_without_mutating_global_role() {
        let mut snapshot = fixture(Role::Restricted);
        snapshot.group = Some(GroupContext {
            group_id: -10042,
            enabled: true,
            role_override: None,
            auto_grant_role: Role::User,
            thread_allowed: true,
        });
        let auth = snapshot.authorize(&policy(), Some(("dl", "inline")));
        assert!(auth.decision.allowed);
        assert_eq!(auth.grant_source, Some(GrantSource::Role));
        assert_eq!(snapshot.principal.role, Role::Restricted);
    }
    #[test]
    fn owner_does_not_authorize_an_unregistered_feature() {
        let auth = fixture(Role::Owner).authorize(&policy(), Some(("unknown", "feature")));
        assert!(!auth.decision.allowed);
        assert_eq!(auth.decision.deny_reason, "unknown_feature");
    }
    #[test]
    fn explicit_grant_never_bypasses_blocked_identity() {
        let mut snapshot = fixture(Role::Banned);
        snapshot.grants.push(Grant {
            plugin: "insight".into(),
            feature: "tldr".into(),
            expires_at: None,
        });
        let auth = snapshot.authorize(&policy(), Some(("insight", "tldr")));
        assert!(!auth.decision.allowed);
        assert_eq!(auth.grant_source, None);
    }
}
