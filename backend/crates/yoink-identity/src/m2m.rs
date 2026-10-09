use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use turso::transaction::TransactionBehavior;
use yoink_db::Transaction;
use yoink_db::query;

use super::{IdentityStore, admin::AdminError};

// Intentionally has no Debug implementation: key material and authorization state
// should not accidentally enter diagnostics.
pub struct M2mKey {
    pub scopes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct M2mUser {
    pub id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub role: String,
    pub language: String,
    pub is_blocked: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct M2mGroup {
    pub id: i64,
    pub title: Option<String>,
    pub enabled: bool,
    pub auto_grant_role: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct M2mEvent {
    pub id: i64,
    pub plugin: String,
    pub event_type: String,
    pub user_id: Option<i64>,
    pub group_id: Option<i64>,
    pub payload: Option<Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct M2mCounts {
    pub total_users: i64,
    pub total_groups: i64,
    pub active_groups: i64,
}

pub struct M2mEventCreate<'a> {
    pub plugin: &'a str,
    pub event_type: &'a str,
    pub user_id: Option<i64>,
    pub group_id: Option<i64>,
    pub thread_id: Option<i64>,
    pub payload: Option<&'a Value>,
}

fn role(value: String) -> Result<String, AdminError> {
    if matches!(
        value.as_str(),
        "owner" | "admin" | "moderator" | "user" | "restricted" | "banned"
    ) {
        Ok(value)
    } else {
        Err(AdminError::Unavailable)
    }
}

fn decode_user(row: yoink_db::Row, now: DateTime<Utc>) -> Result<M2mUser, AdminError> {
    let role_value: String = row.try_get(3).map_err(|_| AdminError::Unavailable)?;
    let ban_until: Option<DateTime<Utc>> = row.try_get(6).map_err(|_| AdminError::Unavailable)?;
    Ok(M2mUser {
        id: row.try_get(0).map_err(|_| AdminError::Unavailable)?,
        username: row.try_get(1).map_err(|_| AdminError::Unavailable)?,
        first_name: row.try_get(2).map_err(|_| AdminError::Unavailable)?,
        role: role(role_value.clone())?,
        language: row.try_get(4).map_err(|_| AdminError::Unavailable)?,
        is_blocked: role_value == "banned" || ban_until.is_some_and(|until| until > now),
        created_at: row.try_get(5).map_err(|_| AdminError::Unavailable)?,
    })
}

fn decode_event(row: yoink_db::Row) -> Result<M2mEvent, AdminError> {
    let payload: Option<Value> = row.try_get(5).map_err(|_| AdminError::Unavailable)?;
    let payload = payload.filter(|value| !value.is_null());
    if payload.as_ref().is_some_and(|value| !value.is_object()) {
        return Err(AdminError::Unavailable);
    }
    Ok(M2mEvent {
        id: row.try_get(0).map_err(|_| AdminError::Unavailable)?,
        plugin: row.try_get(1).map_err(|_| AdminError::Unavailable)?,
        event_type: row.try_get(2).map_err(|_| AdminError::Unavailable)?,
        user_id: row.try_get(3).map_err(|_| AdminError::Unavailable)?,
        group_id: row.try_get(4).map_err(|_| AdminError::Unavailable)?,
        payload,
        created_at: row.try_get(6).map_err(|_| AdminError::Unavailable)?,
    })
}

async fn db_now(tx: &Transaction<'_>) -> Result<DateTime<Utc>, AdminError> {
    query(include_str!("sql/database_now.sql"))
        .fetch_one(&**tx)
        .await
        .map_err(|_| AdminError::Unavailable)?
        .try_get(0)
        .map_err(|_| AdminError::Unavailable)
}

impl IdentityStore {
    /// Validates and records use in one fresh write transaction. None means an
    /// absent, unknown, revoked, or expired credential; corrupt rows are errors.
    pub async fn m2m_authenticate(&self, raw_key: &str) -> Result<Option<M2mKey>, AdminError> {
        if raw_key.is_empty() {
            return Ok(None);
        }
        let digest = format!("{:x}", Sha256::digest(raw_key.as_bytes()));
        let mut connection = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            let Some(row) = query(include_str!("m2m_auth.sql"))
                .bind(&digest)
                .fetch_optional(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
            else {
                return Ok(None);
            };
            let stored_hash: String = row.try_get(0).map_err(|_| AdminError::Unavailable)?;
            if stored_hash != digest {
                return Err(AdminError::Unavailable);
            }
            let raw_scopes: String = row.try_get(1).map_err(|_| AdminError::Unavailable)?;
            let scopes: Vec<String> =
                serde_json::from_str(&raw_scopes).map_err(|_| AdminError::Unavailable)?;
            let revoked: bool = row.try_get(2).map_err(|_| AdminError::Unavailable)?;
            let expires_at: Option<DateTime<Utc>> =
                row.try_get(3).map_err(|_| AdminError::Unavailable)?;
            let now = db_now(&tx).await?;
            if revoked || expires_at.is_some_and(|expiry| expiry <= now) {
                return Ok(None);
            }
            query(include_str!("m2m_last_used.sql"))
                .bind(now)
                .bind(&digest)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            Ok(Some(M2mKey { scopes }))
        }
        .await;
        match result {
            Ok(value) => {
                tx.commit().await.map_err(|_| AdminError::Unavailable)?;
                Ok(value)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }

    pub async fn m2m_counts(&self) -> Result<M2mCounts, AdminError> {
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            let row = query(include_str!("m2m_counts.sql"))
                .fetch_one(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            Ok(M2mCounts {
                total_users: row.try_get(0).map_err(|_| AdminError::Unavailable)?,
                total_groups: row.try_get(1).map_err(|_| AdminError::Unavailable)?,
                active_groups: row.try_get(2).map_err(|_| AdminError::Unavailable)?,
            })
        }
        .await;
        finish(tx, result).await
    }

    pub async fn m2m_users(
        &self,
        role_filter: Option<&str>,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<M2mUser>, AdminError> {
        if offset < 0 || !(1..=200).contains(&limit) {
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
            let now = db_now(&tx).await?;
            let rows = query(include_str!("m2m_users.sql"))
                .bind(role_filter)
                .bind(limit)
                .bind(offset)
                .fetch_all(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            rows.into_iter().map(|row| decode_user(row, now)).collect()
        }
        .await;
        finish(tx, result).await
    }

    pub async fn m2m_user(&self, id: i64) -> Result<Option<M2mUser>, AdminError> {
        if id <= 0 {
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
            let now = db_now(&tx).await?;
            query(include_str!("m2m_user.sql"))
                .bind(id)
                .fetch_optional(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .map(|row| decode_user(row, now))
                .transpose()
        }
        .await;
        finish(tx, result).await
    }

    pub async fn m2m_groups(&self, enabled_only: bool) -> Result<Vec<M2mGroup>, AdminError> {
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            query(include_str!("m2m_groups.sql"))
                .bind(enabled_only)
                .fetch_all(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .into_iter()
                .map(|row| {
                    Ok(M2mGroup {
                        id: row.try_get(0).map_err(|_| AdminError::Unavailable)?,
                        title: row.try_get(1).map_err(|_| AdminError::Unavailable)?,
                        enabled: row.try_get(2).map_err(|_| AdminError::Unavailable)?,
                        auto_grant_role: role(
                            row.try_get(3).map_err(|_| AdminError::Unavailable)?,
                        )?,
                        created_at: row.try_get(4).map_err(|_| AdminError::Unavailable)?,
                    })
                })
                .collect()
        }
        .await;
        finish(tx, result).await
    }

    pub async fn m2m_event_create(
        &self,
        event: M2mEventCreate<'_>,
    ) -> Result<M2mEvent, AdminError> {
        if event.plugin.chars().count() > 32
            || event.event_type.chars().count() > 64
            || event.payload.is_some_and(|value| !value.is_object())
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
            let now = db_now(&tx).await?;
            let payload = event
                .payload
                .map(serde_json::to_string)
                .transpose()
                .map_err(|_| AdminError::InvalidInput)?;
            query(include_str!("m2m_events_insert.sql"))
                .bind(event.plugin)
                .bind(event.event_type)
                .bind(event.user_id)
                .bind(event.group_id)
                .bind(event.thread_id)
                .bind(payload)
                .bind(now)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            decode_event(
                query(include_str!("m2m_event_created.sql"))
                    .fetch_one(&*tx)
                    .await
                    .map_err(|_| AdminError::Unavailable)?,
            )
        }
        .await;
        finish(tx, result).await
    }

    pub async fn m2m_events(
        &self,
        plugin: Option<&str>,
        event_type: Option<&str>,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<M2mEvent>, AdminError> {
        if offset < 0 || !(1..=200).contains(&limit) {
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
            query(include_str!("m2m_events.sql"))
                .bind(plugin)
                .bind(event_type)
                .bind(limit)
                .bind(offset)
                .fetch_all(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .into_iter()
                .map(decode_event)
                .collect()
        }
        .await;
        finish(tx, result).await
    }
}

async fn finish<T>(tx: Transaction<'_>, result: Result<T, AdminError>) -> Result<T, AdminError> {
    match result {
        Ok(value) => {
            tx.commit().await.map_err(|_| AdminError::Unavailable)?;
            Ok(value)
        }
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}
