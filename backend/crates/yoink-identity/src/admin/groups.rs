use chrono::{DateTime, Utc};
use serde::Serialize;
use turso::transaction::{Transaction, TransactionBehavior};
use yoink_access::Role;
use yoink_db::query;

use super::{AdminError, IdentityStore, admin_auth, finish_admin};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Group {
    pub id: i64,
    pub title: Option<String>,
    pub enabled: bool,
    pub auto_grant_role: Role,
    pub allow_pm: bool,
    pub nsfw_allowed: bool,
    pub storage_chat_id: Option<i64>,
    pub storage_thread_id: Option<i64>,
    pub photo_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub thread_policies: Vec<ThreadInline>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ThreadInline {
    pub id: i64,
    pub thread_id: Option<i64>,
    pub name: Option<String>,
    pub enabled: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ThreadPolicy {
    pub id: i64,
    pub group_id: i64,
    pub thread_id: Option<i64>,
    pub name: Option<String>,
    pub enabled: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemberPolicy {
    pub user_id: i64,
    pub group_id: i64,
    pub role_override: Option<Role>,
    pub allow_pm_override: Option<bool>,
}
#[derive(Clone, Debug, Default)]
pub struct GroupCreate {
    pub id: i64,
    pub title: Option<String>,
    pub auto_grant_role: Option<Role>,
    pub allow_pm: Option<bool>,
    pub nsfw_allowed: Option<bool>,
}
#[derive(Clone, Debug, Default)]
pub struct GroupPatch {
    pub title: Option<Option<String>>,
    pub enabled: Option<bool>,
    pub auto_grant_role: Option<Role>,
    pub allow_pm: Option<bool>,
    pub nsfw_allowed: Option<bool>,
    pub storage_chat_id: Option<Option<i64>>,
    pub storage_thread_id: Option<Option<i64>>,
}
#[derive(Clone, Debug)]
pub struct ThreadPatch {
    pub thread_id: Option<i64>,
    pub name: Option<String>,
    pub enabled: bool,
}
#[derive(Clone, Debug, Default)]
pub struct MemberPatch {
    pub role_override: Option<Role>,
    pub allow_pm_override: Option<bool>,
}

fn role(s: String) -> Result<Role, AdminError> {
    s.parse().map_err(|_| AdminError::Unavailable)
}
fn decode_group(r: yoink_db::Row, threads: Vec<ThreadInline>) -> Result<Group, AdminError> {
    Ok(Group {
        id: r.try_get(0).map_err(|_| AdminError::Unavailable)?,
        title: r.try_get(1).map_err(|_| AdminError::Unavailable)?,
        enabled: r.try_get(2).map_err(|_| AdminError::Unavailable)?,
        auto_grant_role: role(r.try_get(3).map_err(|_| AdminError::Unavailable)?)?,
        allow_pm: r.try_get(4).map_err(|_| AdminError::Unavailable)?,
        nsfw_allowed: r.try_get(5).map_err(|_| AdminError::Unavailable)?,
        storage_chat_id: r.try_get(6).map_err(|_| AdminError::Unavailable)?,
        storage_thread_id: r.try_get(7).map_err(|_| AdminError::Unavailable)?,
        photo_url: r.try_get(8).map_err(|_| AdminError::Unavailable)?,
        created_at: r.try_get(9).map_err(|_| AdminError::Unavailable)?,
        thread_policies: threads,
    })
}
async fn inline_threads(tx: &Transaction<'_>, gid: i64) -> Result<Vec<ThreadInline>, AdminError> {
    let rows = query(include_str!("../sql/admin_group_threads_inline.sql"))
        .bind(gid)
        .fetch_all(&**tx)
        .await
        .map_err(|_| AdminError::Unavailable)?;
    rows.into_iter()
        .map(|r| {
            Ok(ThreadInline {
                id: r.try_get(0).map_err(|_| AdminError::Unavailable)?,
                thread_id: r.try_get(1).map_err(|_| AdminError::Unavailable)?,
                name: r.try_get(2).map_err(|_| AdminError::Unavailable)?,
                enabled: r.try_get(3).map_err(|_| AdminError::Unavailable)?,
            })
        })
        .collect()
}
fn decode_thread(r: yoink_db::Row) -> Result<ThreadPolicy, AdminError> {
    Ok(ThreadPolicy {
        id: r.try_get(0).map_err(|_| AdminError::Unavailable)?,
        group_id: r.try_get(1).map_err(|_| AdminError::Unavailable)?,
        thread_id: r.try_get(2).map_err(|_| AdminError::Unavailable)?,
        name: r.try_get(3).map_err(|_| AdminError::Unavailable)?,
        enabled: r.try_get(4).map_err(|_| AdminError::Unavailable)?,
    })
}
fn decode_member(r: yoink_db::Row) -> Result<MemberPolicy, AdminError> {
    Ok(MemberPolicy {
        user_id: r.try_get(0).map_err(|_| AdminError::Unavailable)?,
        group_id: r.try_get(1).map_err(|_| AdminError::Unavailable)?,
        role_override: r
            .try_get::<Option<String>, _>(2)
            .map_err(|_| AdminError::Unavailable)?
            .map(role)
            .transpose()?,
        allow_pm_override: r.try_get(3).map_err(|_| AdminError::Unavailable)?,
    })
}
impl IdentityStore {
    pub async fn groups_list(&self, actor: i64) -> Result<Vec<Group>, AdminError> {
        self.group_list_op(actor, None).await
    }
    pub async fn group_get(&self, actor: i64, id: i64) -> Result<Group, AdminError> {
        if id >= 0 {
            return Err(AdminError::InvalidInput);
        }
        self.group_list_op(actor, Some(id))
            .await?
            .into_iter()
            .next()
            .ok_or(AdminError::NotFound)
    }
    async fn group_list_op(&self, actor: i64, id: Option<i64>) -> Result<Vec<Group>, AdminError> {
        if actor <= 0 || id.is_some_and(|x| x >= 0) {
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
            admin_auth(&tx, actor, self.owner_id, true, false).await?;
            let rows = match id {
                Some(id) => query(include_str!("../sql/admin_group_get.sql"))
                    .bind(id)
                    .fetch_optional(&*tx)
                    .await
                    .map_err(|_| AdminError::Unavailable)?
                    .into_iter()
                    .collect(),
                None => query(include_str!("../sql/admin_groups_list.sql"))
                    .fetch_all(&*tx)
                    .await
                    .map_err(|_| AdminError::Unavailable)?,
            };
            let mut out = Vec::new();
            for r in rows {
                let gid: i64 = r.try_get(0).map_err(|_| AdminError::Unavailable)?;
                let trs = query(include_str!("../sql/admin_group_threads_inline.sql"))
                    .bind(gid)
                    .fetch_all(&*tx)
                    .await
                    .map_err(|_| AdminError::Unavailable)?;
                let mut threads = Vec::new();
                for t in trs {
                    threads.push(ThreadInline {
                        id: t.try_get(0).map_err(|_| AdminError::Unavailable)?,
                        thread_id: t.try_get(1).map_err(|_| AdminError::Unavailable)?,
                        name: t.try_get(2).map_err(|_| AdminError::Unavailable)?,
                        enabled: t.try_get(3).map_err(|_| AdminError::Unavailable)?,
                    });
                }
                out.push(decode_group(r, threads)?);
            }
            Ok(out)
        }
        .await;
        finish_admin(tx, result).await
    }
    pub async fn group_create(&self, actor: i64, p: &GroupCreate) -> Result<Group, AdminError> {
        if p.id >= 0 || p.title.as_ref().is_some_and(|x| x.chars().count() > 256) {
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
            admin_auth(&tx, actor, self.owner_id, true, false).await?;
            let exists = query(include_str!("../sql/admin_group_exists.sql"))
                .bind(p.id)
                .fetch_one(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .try_get::<bool, _>(0)
                .map_err(|_| AdminError::Unavailable)?;
            if exists {
                return Err(AdminError::Conflict);
            }
            query(include_str!("../sql/admin_group_insert.sql"))
                .bind(p.id)
                .bind(p.title.as_deref())
                .bind(p.auto_grant_role.unwrap_or(Role::User).to_string())
                .bind(p.allow_pm.unwrap_or(true))
                .bind(p.nsfw_allowed.unwrap_or(false))
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            query(include_str!("../sql/membership_group_restore.sql"))
                .bind(p.id)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            let rows = query(include_str!("../sql/admin_group_get.sql"))
                .bind(p.id)
                .fetch_one(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            decode_group(rows, vec![])
        }
        .await;
        let result = finish_admin(tx, result).await;
        drop(c);
        if result.is_ok() {
            self.invalidate();
        }
        result
    }
    pub async fn group_update(
        &self,
        actor: i64,
        id: i64,
        p: &GroupPatch,
    ) -> Result<Group, AdminError> {
        if id >= 0
            || p.storage_chat_id.flatten().is_some_and(|x| x == 0)
            || p.storage_thread_id.flatten().is_some_and(|x| x <= 0)
            || p.title
                .as_ref()
                .is_some_and(|x| x.as_ref().is_some_and(|v| v.chars().count() > 256))
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
            admin_auth(&tx, actor, self.owner_id, true, false).await?;
            if !query(include_str!("../sql/admin_group_exists.sql"))
                .bind(id)
                .fetch_one(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .try_get::<bool, _>(0)
                .map_err(|_| AdminError::Unavailable)?
            {
                return Err(AdminError::NotFound);
            }
            query(include_str!("../sql/admin_group_update.sql"))
                .bind(p.title.as_ref().and_then(|x| x.as_deref()))
                .bind(p.title.is_some())
                .bind(p.enabled)
                .bind(p.auto_grant_role.map(|v| v.to_string()))
                .bind(p.allow_pm)
                .bind(p.nsfw_allowed)
                .bind(p.storage_chat_id.flatten())
                .bind(p.storage_chat_id.is_some())
                .bind(p.storage_thread_id.flatten())
                .bind(p.storage_thread_id.is_some())
                .bind(id)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            let r = query(include_str!("../sql/admin_group_get.sql"))
                .bind(id)
                .fetch_one(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            decode_group(r, inline_threads(&tx, id).await?)
        }
        .await;
        let result = finish_admin(tx, result).await;
        drop(c);
        if result.is_ok() {
            self.invalidate();
        }
        result
    }
    pub async fn group_delete(&self, actor: i64, id: i64) -> Result<(), AdminError> {
        if id >= 0 {
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
            admin_auth(&tx, actor, self.owner_id, true, false).await?;
            let n = query(include_str!("../sql/admin_group_delete.sql"))
                .bind(id)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .rows_affected();
            if n == 0 {
                return Err(AdminError::NotFound);
            }
            query(include_str!("../sql/membership_group_suppress.sql"))
                .bind(id)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            query(include_str!("../sql/membership_group_fence.sql"))
                .bind(id)
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
    pub async fn threads_list(&self, actor: i64, id: i64) -> Result<Vec<ThreadPolicy>, AdminError> {
        if id >= 0 {
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
            admin_auth(&tx, actor, self.owner_id, true, false).await?;
            if !query(include_str!("../sql/admin_group_exists.sql"))
                .bind(id)
                .fetch_one(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .try_get::<bool, _>(0)
                .map_err(|_| AdminError::Unavailable)?
            {
                return Err(AdminError::NotFound);
            }
            query(include_str!("../sql/admin_threads_list.sql"))
                .bind(id)
                .fetch_all(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .into_iter()
                .map(decode_thread)
                .collect()
        }
        .await;
        finish_admin(tx, result).await
    }
    pub async fn thread_upsert(
        &self,
        actor: i64,
        id: i64,
        p: &ThreadPatch,
    ) -> Result<ThreadPolicy, AdminError> {
        if id >= 0
            || p.thread_id.is_some_and(|x| x <= 0)
            || p.name.as_ref().is_some_and(|x| x.chars().count() > 256)
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
            admin_auth(&tx, actor, self.owner_id, true, false).await?;
            if !query(include_str!("../sql/admin_group_exists.sql"))
                .bind(id)
                .fetch_one(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .try_get::<bool, _>(0)
                .map_err(|_| AdminError::Unavailable)?
            {
                return Err(AdminError::NotFound);
            }
            query(include_str!("../sql/admin_thread_upsert.sql"))
                .bind(id)
                .bind(p.thread_id)
                .bind(p.name.as_deref())
                .bind(p.enabled)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            decode_thread(
                query(include_str!("../sql/admin_thread_get.sql"))
                    .bind(id)
                    .bind(p.thread_id)
                    .fetch_one(&*tx)
                    .await
                    .map_err(|_| AdminError::Unavailable)?,
            )
        }
        .await;
        let result = finish_admin(tx, result).await;
        drop(c);
        if result.is_ok() {
            self.invalidate();
        }
        result
    }
    pub async fn thread_delete(&self, actor: i64, gid: i64, pid: i64) -> Result<(), AdminError> {
        if gid >= 0 || pid <= 0 {
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
            admin_auth(&tx, actor, self.owner_id, true, false).await?;
            let n = query(include_str!("../sql/admin_thread_delete.sql"))
                .bind(pid)
                .bind(gid)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .rows_affected();
            if n == 0 {
                return Err(AdminError::NotFound);
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
    pub async fn members_list(&self, actor: i64, id: i64) -> Result<Vec<MemberPolicy>, AdminError> {
        if id >= 0 {
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
            admin_auth(&tx, actor, self.owner_id, true, false).await?;
            if !query(include_str!("../sql/admin_group_exists.sql"))
                .bind(id)
                .fetch_one(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .try_get::<bool, _>(0)
                .map_err(|_| AdminError::Unavailable)?
            {
                return Err(AdminError::NotFound);
            }
            query(include_str!("../sql/admin_members_list.sql"))
                .bind(id)
                .fetch_all(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .into_iter()
                .map(decode_member)
                .collect()
        }
        .await;
        finish_admin(tx, result).await
    }
    pub async fn member_upsert(
        &self,
        actor: i64,
        gid: i64,
        uid: i64,
        p: &MemberPatch,
    ) -> Result<MemberPolicy, AdminError> {
        if gid >= 0 || uid <= 0 {
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
            admin_auth(&tx, actor, self.owner_id, true, false).await?;
            if !query(include_str!("../sql/admin_group_exists.sql"))
                .bind(gid)
                .fetch_one(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .try_get::<bool, _>(0)
                .map_err(|_| AdminError::Unavailable)?
            {
                return Err(AdminError::NotFound);
            }
            if !query(include_str!("../sql/admin_user_exists.sql"))
                .bind(uid)
                .fetch_one(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .try_get::<bool, _>(0)
                .map_err(|_| AdminError::Unavailable)?
            {
                return Err(AdminError::NotFound);
            }
            query(include_str!("../sql/admin_member_upsert.sql"))
                .bind(uid)
                .bind(gid)
                .bind(p.role_override.map(|v| v.to_string()))
                .bind(p.allow_pm_override)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            decode_member(
                query(include_str!("../sql/admin_member_get.sql"))
                    .bind(gid)
                    .bind(uid)
                    .fetch_one(&*tx)
                    .await
                    .map_err(|_| AdminError::Unavailable)?,
            )
        }
        .await;
        let result = finish_admin(tx, result).await;
        drop(c);
        if result.is_ok() {
            self.invalidate();
        }
        result
    }
    pub async fn member_delete(&self, actor: i64, gid: i64, uid: i64) -> Result<(), AdminError> {
        if gid >= 0 || uid <= 0 {
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
            admin_auth(&tx, actor, self.owner_id, true, false).await?;
            let n = query(include_str!("../sql/admin_member_delete.sql"))
                .bind(gid)
                .bind(uid)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .rows_affected();
            if n == 0 {
                return Err(AdminError::NotFound);
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
