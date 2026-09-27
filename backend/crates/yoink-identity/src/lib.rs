use chrono::{DateTime, Utc};
use sqlx::{Row, postgres::PgPool};
use thiserror::Error;
use yoink_access::{
    AccessContext, AccessDecision, AccessPolicy, AccessScope, GrantSource, check_access,
    effective_grant,
};
use yoink_access::{FeatureSpec, Grant, Principal, ProviderGrant, Role};

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

impl From<sqlx::Error> for IdentityError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}

pub struct IdentityStore {
    pool: PgPool,
    owner_id: Option<i64>,
    insight_enabled: bool,
}

impl IdentityStore {
    pub fn new(pool: PgPool, owner_id: Option<i64>, insight_enabled: bool) -> Self {
        Self {
            pool,
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

        let mut tx = self.pool.begin().await?;
        sqlx::query(include_str!("sql/begin_read_only.sql"))
            .execute(&mut *tx)
            .await?;
        let now: DateTime<Utc> = sqlx::query(include_str!("sql/database_now.sql"))
            .fetch_one(&mut *tx)
            .await?
            .try_get("now")
            .map_err(|_| IdentityError::Database)?;

        let row = sqlx::query(include_str!("sql/user.sql"))
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Ok(None);
        };
        let role = parse_role(
            row.try_get::<String, _>("role")
                .map_err(|_| IdentityError::Database)?,
        )?;
        let ban_until = row
            .try_get::<Option<DateTime<Utc>>, _>("ban_until")
            .map_err(|_| IdentityError::Database)?;
        let mut principal = Principal {
            user_id,
            role,
            ban_until,
        };
        let language = row
            .try_get::<String, _>("language")
            .map_err(|_| IdentityError::Database)?;

        // A configured owner is elevated only after global blocking has been checked.
        if !principal.is_blocked(now) && self.owner_id == Some(user_id) {
            principal.role = Role::Owner;
        }

        let grant_rows = sqlx::query(include_str!("sql/grants.sql"))
            .bind(user_id)
            .fetch_all(&mut *tx)
            .await?;
        let mut grants = Vec::with_capacity(grant_rows.len());
        for grant in grant_rows {
            grants.push(Grant {
                plugin: grant
                    .try_get("plugin")
                    .map_err(|_| IdentityError::Database)?,
                feature: grant
                    .try_get("feature")
                    .map_err(|_| IdentityError::Database)?,
                expires_at: grant
                    .try_get("expires_at")
                    .map_err(|_| IdentityError::Database)?,
            });
        }

        let mut providers = Vec::new();
        if self.insight_enabled {
            let setting_row = sqlx::query(include_str!("sql/byok_enabled.sql"))
                .fetch_optional(&mut *tx)
                .await?;
            let setting_value = setting_row
                .map(|row| row.try_get::<Option<String>, _>("value"))
                .transpose()
                .map_err(|_| IdentityError::Database)?
                .flatten();
            let setting = setting_value.is_some_and(|value| {
                matches!(value.to_lowercase().as_str(), "1" | "true" | "yes" | "on")
            });
            if setting {
                let ready = sqlx::query(include_str!("sql/byok_ready.sql"))
                    .bind(user_id)
                    .fetch_one(&mut *tx)
                    .await?
                    .try_get::<bool, _>("ready")
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
            let row = sqlx::query(include_str!("sql/group_context.sql"))
                .bind(user_id)
                .bind(group_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(IdentityError::Database)?;
            let enabled: bool = row
                .try_get("enabled")
                .map_err(|_| IdentityError::Database)?;
            let role_override = row
                .try_get::<Option<String>, _>("role_override")
                .map_err(|_| IdentityError::Database)?
                .map(parse_role)
                .transpose()?;
            let auto_grant_role = parse_role(
                row.try_get::<String, _>("auto_grant_role")
                    .map_err(|_| IdentityError::Database)?,
            )?;
            let threads = sqlx::query(include_str!("sql/thread_context.sql"))
                .bind(group_id)
                .bind(thread_id)
                .fetch_one(&mut *tx)
                .await?;
            let matching = threads
                .try_get::<Option<bool>, _>("matching_enabled")
                .map_err(|_| IdentityError::Database)?;
            let has_enabled: bool = threads
                .try_get("has_enabled_policy")
                .map_err(|_| IdentityError::Database)?;
            let thread_allowed = matching.unwrap_or(!has_enabled);
            Some(GroupContext {
                group_id,
                enabled,
                role_override,
                auto_grant_role,
                thread_allowed,
            })
        } else {
            None
        };
        tx.commit().await?;
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
}

#[cfg(test)]
mod authorization_tests {
    use super::*;
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
