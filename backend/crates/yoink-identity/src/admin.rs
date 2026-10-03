use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use thiserror::Error;
use turso::transaction::{Transaction, TransactionBehavior};
use yoink_access::{FeatureSpec, Grant, Principal, Role, effective_grant};
use yoink_db::query;

use crate::{IdentityStore, registered_feature_metadata, registered_features};

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AdminError {
    #[error("not authenticated")]
    Unauthorized,
    #[error("insufficient permissions")]
    Forbidden,
    #[error("user or group not found")]
    NotFound,
    #[error("resource already exists")]
    Conflict,
    #[error("invalid administration input")]
    InvalidInput,
    #[error("invalid API key scope")]
    InvalidScope,
    #[error("administration data is unavailable")]
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminUsersQuery {
    pub offset: i64,
    pub limit: i64,
    pub search: Option<String>,
    pub role: Option<String>,
    pub status: Option<String>,
    pub sort: String,
    pub direction: String,
    pub period: Option<String>,
}
impl Default for AdminUsersQuery {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: 50,
            search: None,
            role: None,
            status: None,
            sort: "dl_count".into(),
            direction: "desc".into(),
            period: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AdminUser {
    pub id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub photo_url: Option<String>,
    pub role: Role,
    pub language: String,
    pub theme: String,
    pub ban_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub dl_count: i64,
    pub dl_last_at: Option<DateTime<Utc>>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AdminUserPage {
    pub items: Vec<AdminUser>,
    pub total: i64,
    pub offset: i64,
    pub limit: i64,
    pub has_more: bool,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AdminUserPatch {
    pub role: Option<Role>,
    pub language: Option<String>,
    pub ban_until: Option<Option<DateTime<Utc>>>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AdminPermission {
    pub id: i64,
    pub user_id: i64,
    pub plugin: String,
    pub feature: String,
    pub granted_by: i64,
    pub granted_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub grant_source: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AdminFeature {
    pub plugin: String,
    pub feature: String,
    pub label: String,
    pub description: String,
    pub default_min_role: Option<Role>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AdminTagMapEntry {
    pub tag: String,
    pub features: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AdminAvailableFeature {
    pub key: String,
    pub plugin: String,
    pub feature: String,
    pub label: String,
    pub description: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AdminFeatureAccess {
    pub plugin: String,
    pub feature: String,
    pub label: String,
    pub description: String,
    pub default_min_role: Option<Role>,
    pub access_via_role: bool,
    pub access_via_grant: bool,
    pub effective: bool,
    pub grant_expires_at: Option<DateTime<Utc>>,
    pub grant_source: String,
}

fn parse_role(s: String) -> Result<Role, AdminError> {
    s.parse().map_err(|_| AdminError::Unavailable)
}

impl IdentityStore {
    pub async fn admin_list_users(
        &self,
        actor_id: i64,
        q: &AdminUsersQuery,
    ) -> Result<AdminUserPage, AdminError> {
        if actor_id <= 0
            || q.offset < 0
            || !(1..=200).contains(&q.limit)
            || q.search.as_ref().is_some_and(|s| s.len() > 256)
        {
            return Err(AdminError::InvalidInput);
        }
        if q.role
            .as_deref()
            .is_some_and(|s| s != "all" && s.parse::<Role>().is_err())
            || q.status
                .as_deref()
                .is_some_and(|s| !matches!(s, "all" | "active" | "restricted" | "banned"))
            || q.period
                .as_deref()
                .is_some_and(|s| !matches!(s, "7" | "30" | "90"))
        {
            return Err(AdminError::InvalidInput);
        }
        if !matches!(
            q.sort.as_str(),
            "created_at" | "updated_at" | "name" | "role" | "dl_count" | "dl_last_at"
        ) || !matches!(q.direction.as_str(), "asc" | "desc")
        {
            return Err(AdminError::InvalidInput);
        }
        let sort = q.sort.as_str();
        let dir = if q.direction == "asc" { "ASC" } else { "DESC" };
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            admin_auth(&tx, actor_id, self.owner_id, true, false).await?;
            let total = query(include_str!("admin_users_count.sql"))
                .bind(q.search.as_deref().unwrap_or(""))
                .bind(q.role.as_deref().unwrap_or("all"))
                .bind(q.status.as_deref().unwrap_or("all"))
                .fetch_one(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .try_get::<i64, _>(0)
                .map_err(|_| AdminError::Unavailable)?;
            let sql = match (sort, dir) {
                ("created_at", "ASC") => include_str!("admin_users_created_asc.sql"),
                ("created_at", _) => include_str!("admin_users_created_desc.sql"),
                ("updated_at", "ASC") => include_str!("admin_users_updated_asc.sql"),
                ("updated_at", _) => include_str!("admin_users_updated_desc.sql"),
                ("name", "ASC") => include_str!("admin_users_name_asc.sql"),
                ("name", _) => include_str!("admin_users_name_desc.sql"),
                ("role", "ASC") => include_str!("admin_users_role_asc.sql"),
                ("role", _) => include_str!("admin_users_role_desc.sql"),
                ("dl_count", "ASC") => include_str!("admin_users_dl_count_asc.sql"),
                ("dl_count", _) => include_str!("admin_users_dl_count_desc.sql"),
                ("dl_last_at", "ASC") => include_str!("admin_users_dl_last_asc.sql"),
                _ => include_str!("admin_users_dl_last_desc.sql"),
            };
            let rows = query(sql)
                .bind(q.search.as_deref().unwrap_or(""))
                .bind(q.role.as_deref().unwrap_or("all"))
                .bind(q.status.as_deref().unwrap_or("all"))
                .bind(q.period.as_deref().unwrap_or("0"))
                .bind(q.offset)
                .bind(q.limit)
                .fetch_all(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            let mut items = Vec::new();
            for r in rows {
                let role = parse_role(r.try_get(4).map_err(|_| AdminError::Unavailable)?)?;
                let mut u = AdminUser {
                    id: r.try_get(0).map_err(|_| AdminError::Unavailable)?,
                    username: r.try_get(1).map_err(|_| AdminError::Unavailable)?,
                    first_name: r.try_get(2).map_err(|_| AdminError::Unavailable)?,
                    photo_url: r.try_get(3).map_err(|_| AdminError::Unavailable)?,
                    role,
                    language: r.try_get(5).map_err(|_| AdminError::Unavailable)?,
                    theme: r.try_get(6).map_err(|_| AdminError::Unavailable)?,
                    ban_until: r.try_get(7).map_err(|_| AdminError::Unavailable)?,
                    created_at: r.try_get(8).map_err(|_| AdminError::Unavailable)?,
                    updated_at: r.try_get(9).map_err(|_| AdminError::Unavailable)?,
                    dl_count: r.try_get(10).map_err(|_| AdminError::Unavailable)?,
                    dl_last_at: r.try_get(11).map_err(|_| AdminError::Unavailable)?,
                };
                if self.owner_id == Some(u.id) && !blocked(u.role, u.ban_until, now_db(&tx).await?)
                {
                    u.role = Role::Owner;
                }
                items.push(u);
            }
            Ok(AdminUserPage {
                has_more: q.offset.saturating_add(items.len() as i64) < total,
                items,
                total,
                offset: q.offset,
                limit: q.limit,
            })
        }
        .await;
        finish_admin(tx, result).await
    }

    pub async fn admin_get_user(
        &self,
        actor_id: i64,
        target_id: i64,
    ) -> Result<AdminUser, AdminError> {
        valid_ids(actor_id, target_id)?;
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            admin_auth(&tx, actor_id, self.owner_id, true, false).await?;
            let row = query(include_str!("admin_user.sql"))
                .bind(target_id)
                .fetch_optional(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .ok_or(AdminError::NotFound)?;
            decode_user(row, self.owner_id, now_db(&tx).await?)
        }
        .await;
        finish_admin(tx, result).await
    }
    pub async fn admin_update_user(
        &self,
        actor_id: i64,
        target_id: i64,
        p: &AdminUserPatch,
    ) -> Result<AdminUser, AdminError> {
        valid_ids(actor_id, target_id)?;
        if p.language
            .as_deref()
            .is_some_and(|s| !matches!(s, "en" | "ru"))
        {
            return Err(AdminError::InvalidInput);
        }
        if p.role == Some(Role::Owner) {
            return Err(AdminError::Forbidden);
        }
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            let actor = admin_auth(&tx, actor_id, self.owner_id, true, false).await?;
            let row = query(include_str!("admin_user.sql"))
                .bind(target_id)
                .fetch_optional(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .ok_or(AdminError::NotFound)?;
            let target = decode_user(row, self.owner_id, now_db(&tx).await?)?;
            if target.role == Role::Owner || self.owner_id == Some(target.id) {
                return Err(AdminError::Forbidden);
            }
            if actor != Role::Owner && target.role == Role::Admin {
                return Err(AdminError::Forbidden);
            }
            if actor != Role::Owner && p.role.is_some_and(|r| r >= Role::Admin) {
                return Err(AdminError::Forbidden);
            }
            if target_id == actor_id && p.role.is_some_and(|r| r > actor) {
                return Err(AdminError::Forbidden);
            }
            if p.role.is_some() || p.language.is_some() || p.ban_until.is_some() {
                let role = p.role.map(|r| r.to_string());
                let ban_set = if p.ban_until.is_some() { 1_i64 } else { 0_i64 };
                let ban = p.ban_until.flatten();
                let changed = query(include_str!("admin_user_update.sql"))
                    .bind(role.as_deref())
                    .bind(p.language.as_deref())
                    .bind(ban_set)
                    .bind(ban)
                    .bind(target_id)
                    .fetch_optional(&*tx)
                    .await
                    .map_err(|_| AdminError::Unavailable)?;
                if changed.is_none() {
                    return Err(AdminError::Unavailable);
                }
            }
            let row = query(include_str!("admin_user.sql"))
                .bind(target_id)
                .fetch_one(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            decode_user(row, self.owner_id, now_db(&tx).await?)
        }
        .await;
        let result = finish_admin(tx, result).await;
        drop(c);
        if result.is_ok() {
            self.invalidate();
        }
        result
    }

    pub async fn admin_permissions(
        &self,
        actor_id: i64,
        target_id: Option<i64>,
    ) -> Result<Vec<AdminPermission>, AdminError> {
        if actor_id <= 0 || target_id.is_some_and(|i| i <= 0) {
            return Err(AdminError::InvalidInput);
        }
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            admin_auth(&tx, actor_id, self.owner_id, true, false).await?;
            if let Some(id) = target_id {
                exists(&tx, id).await?;
            }
            let statement = match target_id {
                Some(id) => query(include_str!("admin_permissions_user.sql")).bind(id),
                None => query(include_str!("admin_permissions_all.sql")),
            };
            let rows = statement
                .fetch_all(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            rows.into_iter().map(decode_permission).collect()
        }
        .await;
        finish_admin(tx, result).await
    }
    pub async fn admin_grant_permission(
        &self,
        actor_id: i64,
        target_id: i64,
        plugin: &str,
        feature: &str,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<AdminPermission, AdminError> {
        valid_ids(actor_id, target_id)?;
        if plugin.is_empty()
            || plugin.len() > 32
            || feature.is_empty()
            || feature.len() > 64
            || !registered_features()
                .iter()
                .any(|s| s.plugin == plugin && s.feature == feature)
        {
            return Err(AdminError::InvalidInput);
        }
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            let role = admin_auth(&tx, actor_id, self.owner_id, true, false).await?;
            let target = user_role(&tx, target_id, self.owner_id)
                .await?
                .ok_or(AdminError::NotFound)?;
            if self.owner_id == Some(target_id) || (target >= Role::Admin && role != Role::Owner) {
                return Err(AdminError::Forbidden);
            }
            let old = query(include_str!("admin_permission_source.sql"))
                .bind(target_id)
                .bind(plugin)
                .bind(feature)
                .fetch_optional(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            if let Some(ref r) = old {
                let source: Option<String> = r.try_get(0).map_err(|_| AdminError::Unavailable)?;
                match source.as_deref().unwrap_or("manual") {
                    "manual" => {}
                    "tag" if role == Role::Owner => {}
                    "tag" => return Err(AdminError::Forbidden),
                    _ => return Err(AdminError::Unavailable),
                }
            }
            query(include_str!("admin_grant_upsert.sql"))
                .bind(target_id)
                .bind(plugin)
                .bind(feature)
                .bind(actor_id)
                .bind(expires_at)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            let row = query(include_str!("admin_permission_get.sql"))
                .bind(target_id)
                .bind(plugin)
                .bind(feature)
                .fetch_one(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            decode_permission(row)
        }
        .await;
        let result = finish_admin(tx, result).await;
        drop(c);
        if result.is_ok() {
            self.invalidate();
        }
        result
    }
    pub async fn admin_revoke_permission(
        &self,
        actor_id: i64,
        target_id: i64,
        plugin: &str,
        feature: &str,
    ) -> Result<(), AdminError> {
        valid_ids(actor_id, target_id)?;
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            let role = admin_auth(&tx, actor_id, self.owner_id, true, false).await?;
            let target = user_role(&tx, target_id, self.owner_id)
                .await?
                .ok_or(AdminError::NotFound)?;
            if self.owner_id == Some(target_id) || (target >= Role::Admin && role != Role::Owner) {
                return Err(AdminError::Forbidden);
            }
            let old = query(include_str!("admin_permission_source.sql"))
                .bind(target_id)
                .bind(plugin)
                .bind(feature)
                .fetch_optional(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            if let Some(ref r) = old {
                let source: Option<String> = r.try_get(0).map_err(|_| AdminError::Unavailable)?;
                match source.as_deref().unwrap_or("manual") {
                    "manual" => {}
                    "tag" if role == Role::Owner => {}
                    "tag" => return Err(AdminError::Forbidden),
                    _ => return Err(AdminError::Unavailable),
                }
            }
            query(include_str!("admin_revoke.sql"))
                .bind(target_id)
                .bind(plugin)
                .bind(feature)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            Ok(())
        }
        .await;
        let result = finish_admin(tx, result).await;
        drop(c);
        if result.is_ok() {
            self.invalidate();
        }
        result
    }

    pub async fn admin_feature_access(
        &self,
        actor_id: i64,
        target_id: Option<i64>,
    ) -> Result<Vec<AdminFeatureAccess>, AdminError> {
        if actor_id <= 0 || target_id.is_some_and(|x| x <= 0) {
            return Err(AdminError::InvalidInput);
        }
        let target = target_id.unwrap_or(actor_id);
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            let actor = admin_auth(&tx, actor_id, self.owner_id, false, false).await?;
            if target != actor_id && actor < Role::Admin {
                return Err(AdminError::Forbidden);
            }
            let role = user_role(&tx, target, self.owner_id)
                .await?
                .ok_or(AdminError::NotFound)?;
            let now = now_db(&tx).await?;
            let blocked = blocked(role, ban(&tx, target).await?, now);
            let permissions = permission_list(&tx, target).await?;
            let grants: Vec<_> = permissions
                .iter()
                .map(|p| Grant {
                    plugin: p.plugin.clone(),
                    feature: p.feature.clone(),
                    expires_at: p.expires_at,
                })
                .collect();
            let mut providers = Vec::new();
            if self.insight_enabled {
                let snap = self
                    .read_snapshot(&tx, target, None)
                    .await
                    .map_err(|_| AdminError::Unavailable)?
                    .ok_or(AdminError::NotFound)?;
                providers = snap.providers;
            }
            Ok(registered_features()
                .into_iter()
                .map(|s| {
                    let gr = grants
                        .iter()
                        .find(|g| g.plugin == s.plugin && g.feature == s.feature);
                    let principal = Principal {
                        user_id: target,
                        role,
                        ban_until: None,
                    };
                    let access = effective_grant(&principal, &s, &grants, &providers, now);
                    let via_role = !blocked
                        && (role == Role::Owner || s.default_min_role.is_some_and(|m| role >= m));
                    let via_grant =
                        !blocked && gr.is_some_and(|g| g.expires_at.is_none_or(|e| e > now));
                    let metadata = feature(s.clone());
                    AdminFeatureAccess {
                        plugin: s.plugin.clone(),
                        feature: s.feature.clone(),
                        label: metadata.label,
                        description: metadata.description,
                        default_min_role: s.default_min_role,
                        access_via_role: via_role,
                        access_via_grant: via_grant,
                        effective: !blocked && access.is_some(),
                        grant_expires_at: gr.and_then(|g| g.expires_at),
                        grant_source: permissions
                            .iter()
                            .find(|p| p.plugin == s.plugin && p.feature == s.feature)
                            .map_or_else(|| "manual".into(), |p| p.grant_source.clone()),
                    }
                })
                .collect())
        }
        .await;
        finish_admin(tx, result).await
    }
    pub async fn admin_features(&self, actor_id: i64) -> Result<Vec<AdminFeature>, AdminError> {
        if actor_id <= 0 {
            return Err(AdminError::InvalidInput);
        }
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            admin_auth(&tx, actor_id, self.owner_id, true, false).await?;
            Ok(registered_features().into_iter().map(feature).collect())
        }
        .await;
        finish_admin(tx, result).await
    }
    pub async fn admin_tag_map(&self, actor_id: i64) -> Result<Vec<AdminTagMapEntry>, AdminError> {
        if actor_id <= 0 {
            return Err(AdminError::InvalidInput);
        }
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            admin_auth(&tx, actor_id, self.owner_id, true, false).await?;
            let row = query(include_str!("admin_tag_map_read.sql"))
                .fetch_optional(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            match row {
                None => Ok(Vec::new()),
                Some(row) => {
                    let raw: String = row.try_get(0).map_err(|_| AdminError::Unavailable)?;
                    decode_tag_map(&raw)
                }
            }
        }
        .await;
        finish_admin(tx, result).await
    }

    pub async fn admin_update_tag_map(
        &self,
        actor_id: i64,
        entries: &[AdminTagMapEntry],
    ) -> Result<Vec<AdminTagMapEntry>, AdminError> {
        if actor_id <= 0 {
            return Err(AdminError::InvalidInput);
        }
        let mapping = validated_tag_map(entries).ok_or(AdminError::InvalidInput)?;
        let encoded = serde_json::to_string(&mapping).map_err(|_| AdminError::InvalidInput)?;
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            if admin_auth(&tx, actor_id, self.owner_id, true, true).await? != Role::Owner {
                return Err(AdminError::Forbidden);
            }
            query(include_str!("admin_tag_map_upsert.sql"))
                .bind(&encoded)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            Ok(mapping
                .into_iter()
                .map(|(tag, features)| AdminTagMapEntry { tag, features })
                .collect())
        }
        .await;
        let result = finish_admin(tx, result).await;
        drop(c);
        if result.is_ok() {
            self.invalidate();
        }
        result
    }

    pub async fn admin_available_features(
        &self,
        actor_id: i64,
    ) -> Result<Vec<AdminAvailableFeature>, AdminError> {
        if actor_id <= 0 {
            return Err(AdminError::InvalidInput);
        }
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            admin_auth(&tx, actor_id, self.owner_id, true, false).await?;
            let mut items: Vec<_> = registered_features()
                .into_iter()
                .map(|spec| {
                    let (label, description) =
                        registered_feature_metadata(&spec.plugin, &spec.feature)
                            .ok_or(AdminError::Unavailable)?;
                    Ok(AdminAvailableFeature {
                        key: format!("{}:{}", spec.plugin, spec.feature),
                        plugin: spec.plugin,
                        feature: spec.feature,
                        label: label.into(),
                        description: description.into(),
                    })
                })
                .collect::<Result<_, AdminError>>()?;
            items.sort_by(|a, b| a.key.cmp(&b.key));
            Ok(items)
        }
        .await;
        finish_admin(tx, result).await
    }

    pub async fn admin_bot_settings(
        &self,
        actor_id: i64,
    ) -> Result<BTreeMap<String, String>, AdminError> {
        if actor_id <= 0 {
            return Err(AdminError::InvalidInput);
        }
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            admin_auth(&tx, actor_id, self.owner_id, true, false).await?;
            let mut m = BTreeMap::from([
                ("browser_cookies_min_role".into(), "owner".into()),
                ("bot_access_mode".into(), "open".into()),
                ("inline_storage_chat_id".into(), String::new()),
                ("inline_storage_thread_id".into(), String::new()),
                ("tag_map".into(), "{}".into()),
            ]);
            for r in query(include_str!("admin_settings.sql"))
                .fetch_all(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
            {
                let key: String = r.try_get(0).map_err(|_| AdminError::Unavailable)?;
                let value: Option<String> = r.try_get(1).map_err(|_| AdminError::Unavailable)?;
                m.insert(key, value.ok_or(AdminError::Unavailable)?);
            }
            if !matches!(
                m.get("bot_access_mode").map(String::as_str),
                Some("open" | "approved_only")
            ) || m
                .get("browser_cookies_min_role")
                .is_some_and(|v| v.parse::<Role>().is_err())
            {
                return Err(AdminError::Unavailable);
            }
            Ok(m)
        }
        .await;
        finish_admin(tx, result).await
    }
    pub async fn admin_update_bot_settings(
        &self,
        actor_id: i64,
        patch: &BTreeMap<String, String>,
    ) -> Result<(), AdminError> {
        if actor_id <= 0 {
            return Err(AdminError::InvalidInput);
        }
        for (k, v) in patch {
            match k.as_str() {
                "bot_access_mode" if matches!(v.as_str(), "open" | "approved_only") => {}
                "browser_cookies_min_role" if v.parse::<Role>().is_ok() => {}
                "inline_storage_chat_id" => {
                    if !valid_chat_id(v) {
                        return Err(AdminError::InvalidInput);
                    }
                }
                "inline_storage_thread_id" => {
                    if !valid_thread_id(v) {
                        return Err(AdminError::InvalidInput);
                    }
                }
                _ => return Err(AdminError::InvalidInput),
            }
        }
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            if admin_auth(&tx, actor_id, self.owner_id, true, true).await? != Role::Owner {
                return Err(AdminError::Forbidden);
            }
            for (k, v) in patch {
                query(include_str!("admin_setting_upsert.sql"))
                    .bind(k)
                    .bind(v)
                    .execute(&*tx)
                    .await
                    .map_err(|_| AdminError::Unavailable)?;
            }
            Ok(())
        }
        .await;
        let result = finish_admin(tx, result).await;
        drop(c);
        if result.is_ok() {
            self.invalidate();
        }
        result
    }
}

fn validated_tag_map(entries: &[AdminTagMapEntry]) -> Option<BTreeMap<String, Vec<String>>> {
    let mut mapping = BTreeMap::new();
    for entry in entries {
        if entry.tag.trim().is_empty() || entry.tag.chars().any(char::is_control) {
            return None;
        }
        let mut seen = std::collections::BTreeSet::new();
        for key in &entry.features {
            let (plugin, feature) = key.split_once(':')?;
            if plugin.is_empty()
                || feature.is_empty()
                || feature.contains(':')
                || !registered_features()
                    .iter()
                    .any(|spec| spec.plugin == plugin && spec.feature == feature)
                || !seen.insert(key)
            {
                return None;
            }
        }
        if mapping
            .insert(entry.tag.clone(), entry.features.clone())
            .is_some()
        {
            return None;
        }
    }
    Some(mapping)
}

fn decode_tag_map(raw: &str) -> Result<Vec<AdminTagMapEntry>, AdminError> {
    let mapping: BTreeMap<String, Vec<String>> =
        serde_json::from_str(raw).map_err(|_| AdminError::Unavailable)?;
    let entries: Vec<_> = mapping
        .into_iter()
        .map(|(tag, features)| AdminTagMapEntry { tag, features })
        .collect();
    let validated = validated_tag_map(&entries).ok_or(AdminError::Unavailable)?;
    Ok(validated
        .into_iter()
        .map(|(tag, features)| AdminTagMapEntry { tag, features })
        .collect())
}

fn feature(s: FeatureSpec) -> AdminFeature {
    let (label, description) =
        registered_feature_metadata(&s.plugin, &s.feature).expect("registered feature metadata");
    AdminFeature {
        plugin: s.plugin,
        feature: s.feature,
        label: label.into(),
        description: description.into(),
        default_min_role: s.default_min_role,
    }
}
fn valid_chat_id(v: &str) -> bool {
    v.is_empty() || v.parse::<i64>().is_ok_and(|n| n < 0)
}
fn valid_thread_id(v: &str) -> bool {
    v.is_empty() || v.parse::<i64>().is_ok_and(|n| n > 0)
}
fn valid_ids(a: i64, t: i64) -> Result<(), AdminError> {
    if a <= 0 || t <= 0 {
        Err(AdminError::InvalidInput)
    } else {
        Ok(())
    }
}
pub mod api_keys;
pub mod groups;

fn blocked(role: Role, ban: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    role == Role::Banned || ban.is_some_and(|b| b > now)
}
async fn now_db(tx: &Transaction<'_>) -> Result<DateTime<Utc>, AdminError> {
    query(include_str!("sql/database_now.sql"))
        .fetch_one(&**tx)
        .await
        .map_err(|_| AdminError::Unavailable)?
        .try_get(0)
        .map_err(|_| AdminError::Unavailable)
}
pub(super) async fn admin_auth(
    tx: &Transaction<'_>,
    id: i64,
    owner: Option<i64>,
    admin: bool,
    owner_only: bool,
) -> Result<Role, AdminError> {
    let row = query(include_str!("admin_actor.sql"))
        .bind(id)
        .fetch_optional(&**tx)
        .await
        .map_err(|_| AdminError::Unavailable)?
        .ok_or(AdminError::Unauthorized)?;
    let role = parse_role(row.try_get(0).map_err(|_| AdminError::Unavailable)?)?;
    let ban: Option<DateTime<Utc>> = row.try_get(1).map_err(|_| AdminError::Unavailable)?;
    if blocked(role, ban, now_db(tx).await?) {
        return Err(AdminError::Forbidden);
    }
    let role = if owner == Some(id) { Role::Owner } else { role };
    if owner_only && role != Role::Owner {
        return Err(AdminError::Forbidden);
    }
    if admin && role < Role::Admin {
        return Err(AdminError::Forbidden);
    }
    Ok(role)
}
async fn user_role(
    tx: &Transaction<'_>,
    id: i64,
    owner: Option<i64>,
) -> Result<Option<Role>, AdminError> {
    let r = query(include_str!("admin_actor.sql"))
        .bind(id)
        .fetch_optional(&**tx)
        .await
        .map_err(|_| AdminError::Unavailable)?;
    let Some(r) = r else { return Ok(None) };
    let role = parse_role(r.try_get(0).map_err(|_| AdminError::Unavailable)?)?;
    let ban: Option<DateTime<Utc>> = r.try_get(1).map_err(|_| AdminError::Unavailable)?;
    Ok(Some(
        if owner == Some(id) && !blocked(role, ban, now_db(tx).await?) {
            Role::Owner
        } else {
            role
        },
    ))
}
async fn ban(tx: &Transaction<'_>, id: i64) -> Result<Option<DateTime<Utc>>, AdminError> {
    query(include_str!("admin_user_ban.sql"))
        .bind(id)
        .fetch_one(&**tx)
        .await
        .map_err(|_| AdminError::Unavailable)?
        .try_get(0)
        .map_err(|_| AdminError::Unavailable)
}
async fn exists(tx: &Transaction<'_>, id: i64) -> Result<(), AdminError> {
    if user_role(tx, id, None).await?.is_some() {
        Ok(())
    } else {
        Err(AdminError::NotFound)
    }
}
async fn permission_list(
    tx: &Transaction<'_>,
    id: i64,
) -> Result<Vec<AdminPermission>, AdminError> {
    query(include_str!("admin_permissions_user.sql"))
        .bind(id)
        .fetch_all(&**tx)
        .await
        .map_err(|_| AdminError::Unavailable)?
        .into_iter()
        .map(decode_permission)
        .collect()
}
fn decode_permission(r: yoink_db::Row) -> Result<AdminPermission, AdminError> {
    Ok(AdminPermission {
        id: r.try_get(0).map_err(|_| AdminError::Unavailable)?,
        user_id: r.try_get(1).map_err(|_| AdminError::Unavailable)?,
        plugin: r.try_get(2).map_err(|_| AdminError::Unavailable)?,
        feature: r.try_get(3).map_err(|_| AdminError::Unavailable)?,
        granted_by: r.try_get(4).map_err(|_| AdminError::Unavailable)?,
        granted_at: r.try_get(5).map_err(|_| AdminError::Unavailable)?,
        expires_at: r.try_get(6).map_err(|_| AdminError::Unavailable)?,
        grant_source: {
            let v: Option<String> = r.try_get(7).map_err(|_| AdminError::Unavailable)?;
            let v = v.unwrap_or_else(|| "manual".into());
            if !matches!(v.as_str(), "manual" | "tag") {
                return Err(AdminError::Unavailable);
            }
            v
        },
    })
}
fn decode_user(
    r: yoink_db::Row,
    owner: Option<i64>,
    now: DateTime<Utc>,
) -> Result<AdminUser, AdminError> {
    let role = parse_role(r.try_get(4).map_err(|_| AdminError::Unavailable)?)?;
    let ban_until = r.try_get(7).map_err(|_| AdminError::Unavailable)?;
    Ok(AdminUser {
        id: r.try_get(0).map_err(|_| AdminError::Unavailable)?,
        username: r.try_get(1).map_err(|_| AdminError::Unavailable)?,
        first_name: r.try_get(2).map_err(|_| AdminError::Unavailable)?,
        photo_url: r.try_get(3).map_err(|_| AdminError::Unavailable)?,
        role: if owner == Some(r.try_get(0).map_err(|_| AdminError::Unavailable)?)
            && !blocked(role, ban_until, now)
        {
            Role::Owner
        } else {
            role
        },
        language: r.try_get(5).map_err(|_| AdminError::Unavailable)?,
        theme: r.try_get(6).map_err(|_| AdminError::Unavailable)?,
        ban_until,
        created_at: r.try_get(8).map_err(|_| AdminError::Unavailable)?,
        updated_at: r.try_get(9).map_err(|_| AdminError::Unavailable)?,
        dl_count: r.try_get(10).map_err(|_| AdminError::Unavailable)?,
        dl_last_at: r.try_get(11).map_err(|_| AdminError::Unavailable)?,
    })
}
pub(super) async fn finish_admin<T>(
    tx: Transaction<'_>,
    r: Result<T, AdminError>,
) -> Result<T, AdminError> {
    match r {
        Ok(v) => {
            tx.commit().await.map_err(|_| AdminError::Unavailable)?;
            Ok(v)
        }
        Err(e) => {
            let _ = tx.rollback().await;
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_input_validators_are_deterministic() {
        let q = AdminUsersQuery::default();
        assert_eq!(
            (q.offset, q.limit, q.sort.as_str(), q.direction.as_str()),
            (0, 50, "dl_count", "desc")
        );
        assert!(valid_ids(1, 2).is_ok());
        assert_eq!(valid_ids(0, 2), Err(AdminError::InvalidInput));
        assert!(valid_chat_id("-100123"));
        assert!(!valid_chat_id("12"));
        assert!(valid_thread_id("42"));
        assert!(!valid_thread_id("-42"));
    }
    #[test]
    fn ban_precedes_configured_owner_elevation() {
        let now = DateTime::from_timestamp(100, 0).unwrap();
        assert!(blocked(Role::Banned, None, now));
        assert!(blocked(
            Role::User,
            Some(DateTime::from_timestamp(101, 0).unwrap()),
            now
        ));
        assert!(!blocked(Role::User, Some(now), now));
    }
}
