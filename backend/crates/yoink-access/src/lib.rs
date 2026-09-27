//! Pure, transport-independent identity and authorization policy.
//!
//! This crate deliberately fails closed before considering any grant source:
//! the legacy effective-grant helper could allow explicit grants and providers
//! to pass through for blocked users, and its bulk resolver treated a missing
//! principal as unblocked. Here a banned/actively banned principal and an
//! invalid principal ID are denied consistently, including owners.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use thiserror::Error;

/// Global roles in increasing privilege order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Banned,
    Restricted,
    User,
    Moderator,
    Admin,
    Owner,
}

/// Error returned when a role string is not one of the recognized role names.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("unknown role: {0}")]
pub struct ParseRoleError(String);

impl std::fmt::Display for Role {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Banned => "banned",
            Self::Restricted => "restricted",
            Self::User => "user",
            Self::Moderator => "moderator",
            Self::Admin => "admin",
            Self::Owner => "owner",
        })
    }
}

impl FromStr for Role {
    type Err = ParseRoleError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "banned" => Ok(Self::Banned),
            "restricted" => Ok(Self::Restricted),
            "user" => Ok(Self::User),
            "moderator" => Ok(Self::Moderator),
            "admin" => Ok(Self::Admin),
            "owner" => Ok(Self::Owner),
            other => Err(ParseRoleError(other.to_owned())),
        }
    }
}

/// Global identity state. A non-positive ID is invalid and always fails closed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Principal {
    pub user_id: i64,
    pub role: Role,
    pub ban_until: Option<DateTime<Utc>>,
}

impl Principal {
    /// Whether the global role or a currently active timed ban blocks access.
    pub fn is_blocked(&self, now: DateTime<Utc>) -> bool {
        self.role == Role::Banned || self.ban_until.is_some_and(|until| until > now)
    }
}

/// A persisted, potentially expiring user-to-feature grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Grant {
    pub plugin: String,
    pub feature: String,
    pub expires_at: Option<DateTime<Utc>>,
}

/// A provider result already evaluated by trusted application code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderGrant {
    pub plugin: String,
    pub feature: String,
}

/// A registered feature definition. Specs are trusted application input, not
/// user-controlled data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeatureSpec {
    pub plugin: String,
    pub feature: String,
    pub default_min_role: Option<Role>,
}

/// The first matching reason an effective feature is granted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrantSource {
    Owner,
    Explicit,
    Role,
    Provider,
}

/// Resolve a feature grant in owner, explicit, role, provider precedence order.
///
/// Explicit grants expire at the boundary (expires_at == now is expired).
/// Blocking and invalid IDs are checked before every source so no grant can
/// bypass a global ban.
pub fn effective_grant(
    principal: &Principal,
    spec: &FeatureSpec,
    grants: &[Grant],
    providers: &[ProviderGrant],
    now: DateTime<Utc>,
) -> Option<GrantSource> {
    if principal.user_id <= 0 || principal.is_blocked(now) {
        return None;
    }

    if principal.role == Role::Owner {
        return Some(GrantSource::Owner);
    }

    if grants.iter().any(|grant| {
        grant.plugin == spec.plugin
            && grant.feature == spec.feature
            && grant.expires_at.is_none_or(|expires_at| expires_at > now)
    }) {
        return Some(GrantSource::Explicit);
    }

    if spec
        .default_min_role
        .is_some_and(|minimum| principal.role >= minimum)
    {
        return Some(GrantSource::Role);
    }

    if providers
        .iter()
        .any(|grant| grant.plugin == spec.plugin && grant.feature == spec.feature)
    {
        return Some(GrantSource::Provider);
    }

    None
}

/// Chat context for a pure access decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessScope {
    Private,
    Group,
}

/// Declarative rules for one access check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccessPolicy {
    /// Minimum role required after applying a context-local group elevation.
    pub min_role: Role,
    /// Scopes in which this policy is allowed. An empty list denies all scopes.
    pub scopes: Vec<AccessScope>,
    /// Reject group checks when the group is disabled.
    pub check_group_enabled: bool,
    /// Reject group checks when the current thread is not allowed.
    pub check_thread_policy: bool,
}

/// Context-only group settings. Overrides never modify the global principal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessContext {
    pub scope: AccessScope,
    /// Whether the current group is enabled; ignored in private scope.
    pub group_enabled: bool,
    /// Whether the current thread is allowed; ignored in private scope.
    pub thread_allowed: bool,
    /// Explicit per-user role override for this group, if any.
    pub group_role_override: Option<Role>,
    /// Group auto-grant role, applied only to a restricted principal in an enabled group.
    pub group_auto_grant_role: Option<Role>,
}

/// Result of an access check, including the role used for the local decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccessDecision {
    pub allowed: bool,
    /// Stable machine-readable denial reason, empty when allowed.
    pub deny_reason: String,
    /// Effective role for this decision only; the principal remains unchanged.
    pub effective_role: Role,
}

/// Check role and chat-context restrictions without I/O or principal mutation.
///
/// Global bans and active timed bans take precedence over group overrides. An
/// explicit group override takes precedence over auto-grant; auto-grant applies
/// only to restricted principals and only while the group is enabled.
pub fn check_access(
    principal: &Principal,
    policy: &AccessPolicy,
    context: &AccessContext,
    now: DateTime<Utc>,
) -> AccessDecision {
    let denied = |reason: &str, effective_role| AccessDecision {
        allowed: false,
        deny_reason: reason.to_owned(),
        effective_role,
    };

    if principal.user_id <= 0 || principal.is_blocked(now) {
        return denied("blocked", principal.role);
    }

    let effective_role = if context.scope == AccessScope::Group {
        context.group_role_override.unwrap_or_else(|| {
            if principal.role == Role::Restricted && context.group_enabled {
                context
                    .group_auto_grant_role
                    .filter(|role| *role > principal.role)
                    .unwrap_or(principal.role)
            } else {
                principal.role
            }
        })
    } else {
        principal.role
    };

    if !policy.scopes.contains(&context.scope) {
        return denied(
            match context.scope {
                AccessScope::Private => "scope_private_not_allowed",
                AccessScope::Group => "scope_group_not_allowed",
            },
            effective_role,
        );
    }

    if context.scope == AccessScope::Group {
        if policy.check_group_enabled && !context.group_enabled {
            return denied("group_not_enabled", effective_role);
        }
        if policy.check_thread_policy && !context.thread_allowed {
            return denied("thread_not_allowed", effective_role);
        }
    }

    if effective_role < policy.min_role {
        return denied("role_below_minimum", effective_role);
    }

    AccessDecision {
        allowed: true,
        deny_reason: String::new(),
        effective_role,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).expect("valid fixed test timestamp")
    }

    fn principal(role: Role) -> Principal {
        Principal {
            user_id: 42,
            role,
            ban_until: None,
        }
    }

    fn spec(minimum: Option<Role>) -> FeatureSpec {
        FeatureSpec {
            plugin: "dl".to_owned(),
            feature: "download".to_owned(),
            default_min_role: minimum,
        }
    }

    fn access_policy(min_role: Role) -> AccessPolicy {
        AccessPolicy {
            min_role,
            scopes: vec![AccessScope::Private, AccessScope::Group],
            check_group_enabled: true,
            check_thread_policy: true,
        }
    }

    fn group_context() -> AccessContext {
        AccessContext {
            scope: AccessScope::Group,
            group_enabled: true,
            thread_allowed: true,
            group_role_override: None,
            group_auto_grant_role: None,
        }
    }

    #[test]
    fn role_order_and_safe_parsing() {
        let ordered = [
            Role::Banned,
            Role::Restricted,
            Role::User,
            Role::Moderator,
            Role::Admin,
            Role::Owner,
        ];
        for pair in ordered.windows(2) {
            assert!(pair[0] < pair[1]);
        }
        for role in ordered {
            assert_eq!(role.to_string(), format!("{role:?}").to_lowercase());
            assert_eq!(role.to_string().parse::<Role>(), Ok(role));
        }
        assert!("unknown".parse::<Role>().is_err());
        assert!("OWNER".parse::<Role>().is_err());
    }

    #[test]
    fn role_display_uses_snake_case() {
        assert_eq!(Role::Owner.to_string(), "owner");
        assert_eq!(Role::Restricted.to_string(), "restricted");
    }

    #[test]
    fn effective_grant_checks_each_source_and_precedence() {
        let p = principal(Role::User);
        let feature = spec(Some(Role::User));
        let explicit = Grant {
            plugin: "dl".to_owned(),
            feature: "download".to_owned(),
            expires_at: None,
        };
        let provider = ProviderGrant {
            plugin: "dl".to_owned(),
            feature: "download".to_owned(),
        };
        assert_eq!(
            effective_grant(&p, &feature, &[], &[], now()),
            Some(GrantSource::Role)
        );
        assert_eq!(
            effective_grant(
                &p,
                &spec(None),
                std::slice::from_ref(&explicit),
                std::slice::from_ref(&provider),
                now()
            ),
            Some(GrantSource::Explicit)
        );
        assert_eq!(
            effective_grant(&p, &spec(None), &[], std::slice::from_ref(&provider), now()),
            Some(GrantSource::Provider)
        );
        assert_eq!(
            effective_grant(
                &principal(Role::Owner),
                &spec(None),
                std::slice::from_ref(&explicit),
                std::slice::from_ref(&provider),
                now(),
            ),
            Some(GrantSource::Owner)
        );
        assert_eq!(
            effective_grant(&p, &feature, &[explicit], &[provider], now()),
            Some(GrantSource::Explicit)
        );
        assert_eq!(effective_grant(&p, &spec(None), &[], &[], now()), None);
    }

    #[test]
    fn explicit_grant_expiry_is_strict_and_unrelated_grants_do_not_match() {
        let p = principal(Role::Restricted);
        let at_boundary = Grant {
            plugin: "dl".to_owned(),
            feature: "download".to_owned(),
            expires_at: Some(now()),
        };
        assert_eq!(
            effective_grant(&p, &spec(None), &[at_boundary], &[], now()),
            None
        );
        let later = Grant {
            plugin: "dl".to_owned(),
            feature: "download".to_owned(),
            expires_at: Some(now() + chrono::Duration::seconds(1)),
        };
        assert_eq!(
            effective_grant(&p, &spec(None), &[later], &[], now()),
            Some(GrantSource::Explicit)
        );
        let other = Grant {
            plugin: "other".to_owned(),
            feature: "download".to_owned(),
            expires_at: None,
        };
        assert_eq!(effective_grant(&p, &spec(None), &[other], &[], now()), None);
    }

    #[test]
    fn blocked_and_invalid_principals_cannot_receive_any_grant() {
        let feature = spec(Some(Role::Banned));
        let grants = [Grant {
            plugin: "dl".to_owned(),
            feature: "download".to_owned(),
            expires_at: None,
        }];
        let providers = [ProviderGrant {
            plugin: "dl".to_owned(),
            feature: "download".to_owned(),
        }];
        for role in [Role::Banned, Role::Owner] {
            let mut p = principal(role);
            if role == Role::Owner {
                p.ban_until = Some(now() + chrono::Duration::seconds(1));
            }
            assert_eq!(
                effective_grant(&p, &feature, &grants, &providers, now()),
                None
            );
        }
        let mut invalid = principal(Role::Owner);
        invalid.user_id = 0;
        assert_eq!(
            effective_grant(&invalid, &feature, &grants, &providers, now()),
            None
        );
    }

    #[test]
    fn block_boundary_and_helper_are_consistent() {
        let mut p = principal(Role::User);
        p.ban_until = Some(now());
        assert!(!p.is_blocked(now()));
        p.ban_until = Some(now() + chrono::Duration::seconds(1));
        assert!(p.is_blocked(now()));
        assert_eq!(effective_grant(&p, &spec(None), &[], &[], now()), None);
    }

    #[test]
    fn access_hierarchy_and_context_local_group_elevations() {
        let p = principal(Role::Restricted);
        let policy = access_policy(Role::User);
        let mut context = group_context();
        context.group_auto_grant_role = Some(Role::Moderator);
        let decision = check_access(&p, &policy, &context, now());
        assert!(decision.allowed);
        assert_eq!(decision.effective_role, Role::Moderator);
        assert_eq!(p.role, Role::Restricted);

        context.group_role_override = Some(Role::Admin);
        assert_eq!(
            check_access(&p, &policy, &context, now()).effective_role,
            Role::Admin
        );
        assert_eq!(p.role, Role::Restricted);

        context.group_role_override = Some(Role::Restricted);
        assert!(!check_access(&p, &policy, &context, now()).allowed);
    }

    #[test]
    fn auto_grant_only_applies_to_restricted_users_and_enabled_groups() {
        let mut policy = access_policy(Role::Admin);
        let mut context = group_context();
        context.group_auto_grant_role = Some(Role::Admin);
        let regular = check_access(&principal(Role::User), &policy, &context, now());
        assert!(!regular.allowed);
        assert_eq!(regular.effective_role, Role::User);
        assert!(check_access(&principal(Role::Restricted), &policy, &context, now()).allowed);
        context.group_enabled = false;
        policy.check_group_enabled = false;
        let disabled = check_access(&principal(Role::Restricted), &policy, &context, now());
        assert!(!disabled.allowed);
        assert_eq!(disabled.effective_role, Role::Restricted);
    }

    #[test]
    fn blocked_global_principal_cannot_be_elevated_in_group() {
        let policy = access_policy(Role::Restricted);
        let mut context = group_context();
        context.group_role_override = Some(Role::Owner);
        context.group_auto_grant_role = Some(Role::Owner);
        assert_eq!(
            check_access(&principal(Role::Banned), &policy, &context, now()).deny_reason,
            "blocked"
        );
        let mut p = principal(Role::Restricted);
        p.ban_until = Some(now() + chrono::Duration::seconds(1));
        assert_eq!(
            check_access(&p, &policy, &context, now()).deny_reason,
            "blocked"
        );
    }

    #[test]
    fn group_enabled_and_thread_flags_are_policy_controlled() {
        let policy = access_policy(Role::User);
        let mut context = group_context();
        context.group_enabled = false;
        assert_eq!(
            check_access(&principal(Role::User), &policy, &context, now()).deny_reason,
            "group_not_enabled"
        );
        let mut relaxed = policy.clone();
        relaxed.check_group_enabled = false;
        context.thread_allowed = false;
        assert_eq!(
            check_access(&principal(Role::User), &relaxed, &context, now()).deny_reason,
            "thread_not_allowed"
        );
        relaxed.check_thread_policy = false;
        assert!(check_access(&principal(Role::User), &relaxed, &context, now()).allowed);
    }

    #[test]
    fn private_and_group_scopes_are_enforced() {
        let mut policy = access_policy(Role::User);
        policy.scopes = vec![AccessScope::Private];
        let private = AccessContext {
            scope: AccessScope::Private,
            group_enabled: false,
            thread_allowed: false,
            group_role_override: Some(Role::Owner),
            group_auto_grant_role: Some(Role::Owner),
        };
        assert!(check_access(&principal(Role::User), &policy, &private, now()).allowed);
        let group = AccessContext {
            scope: AccessScope::Group,
            ..private
        };
        assert_eq!(
            check_access(&principal(Role::User), &policy, &group, now()).deny_reason,
            "scope_group_not_allowed"
        );
        policy.scopes = vec![AccessScope::Group];
        assert_eq!(
            check_access(&principal(Role::User), &policy, &private, now()).deny_reason,
            "scope_private_not_allowed"
        );
    }
}
