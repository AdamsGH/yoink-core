use base64::Engine as _;
use chrono::{DateTime, Utc};
use rand::{RngCore, rngs::OsRng};
use serde::Serialize;
use sha2::{Digest, Sha256};
use turso::transaction::{Transaction, TransactionBehavior};
use yoink_db::query;

use super::{AdminError, IdentityStore, admin_auth, finish_admin};

pub const API_KEY_SCOPES: &[&str] = &[
    "users:read",
    "users:write",
    "groups:read",
    "groups:write",
    "downloads:read",
    "settings:read",
    "settings:write",
    "events:read",
    "events:write",
    "health:read",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiKeyCreate {
    pub name: String,
    pub scopes: Option<Vec<String>>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ApiKey {
    pub id: i64,
    pub name: String,
    pub prefix: String,
    pub scopes: Vec<String>,
    pub created_by: i64,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct CreatedApiKey {
    pub id: i64,
    pub name: String,
    pub key: String,
    pub prefix: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl IdentityStore {
    pub async fn api_keys_authorized(&self, actor: i64) -> Result<(), AdminError> {
        if actor <= 0 {
            return Err(AdminError::InvalidInput);
        }
        let mut connection = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = admin_auth(&tx, actor, self.owner_id, true, false)
            .await
            .map(|_| ());
        finish_admin(tx, result).await
    }

    pub async fn api_keys_list(&self, actor: i64) -> Result<Vec<ApiKey>, AdminError> {
        if actor <= 0 {
            return Err(AdminError::InvalidInput);
        }
        let mut connection = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            admin_auth(&tx, actor, self.owner_id, true, false).await?;
            let rows = query(include_str!("api_keys_list.sql"))
                .fetch_all(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            rows.into_iter().map(decode_api_key).collect()
        }
        .await;
        finish_admin(tx, result).await
    }

    pub async fn api_key_create(
        &self,
        actor: i64,
        input: &ApiKeyCreate,
    ) -> Result<CreatedApiKey, AdminError> {
        if actor <= 0 || input.name.chars().count() > 128 {
            return Err(AdminError::InvalidInput);
        }
        let mut connection = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            admin_auth(&tx, actor, self.owner_id, true, false).await?;
            let scopes = input.scopes.clone().unwrap_or_else(|| vec!["*".into()]);
            if scopes.iter().any(|scope| !valid_scope(scope)) {
                return Err(AdminError::InvalidScope);
            }
            let scopes_json =
                serde_json::to_string(&scopes).map_err(|_| AdminError::InvalidInput)?;
            let mut random = [0u8; 32];
            let mut os_rng = OsRng;
            os_rng
                .try_fill_bytes(&mut random)
                .map_err(|_| AdminError::Unavailable)?;
            let raw_key = format!(
                "yoink_{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random)
            );
            let digest = format!("{:x}", Sha256::digest(raw_key.as_bytes()));
            let prefix = raw_key.chars().take(8).collect::<String>();
            query(include_str!("api_keys_insert.sql"))
                .bind(&input.name)
                .bind(&digest)
                .bind(&prefix)
                .bind(&scopes_json)
                .bind(actor)
                .bind(input.expires_at)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            let row = query(include_str!("api_keys_created.sql"))
                .bind(&digest)
                .fetch_one(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            let stored = decode_api_key(row)?;
            Ok(CreatedApiKey {
                id: stored.id,
                name: stored.name,
                key: raw_key,
                prefix: stored.prefix,
                scopes: stored.scopes,
                expires_at: stored.expires_at,
                created_at: stored.created_at,
            })
        }
        .await;
        finish_admin(tx, result).await
    }

    pub async fn api_key_revoke(&self, actor: i64, key_id: i64) -> Result<(), AdminError> {
        if actor <= 0 || key_id <= 0 {
            return Err(AdminError::InvalidInput);
        }
        let mut connection = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            admin_auth(&tx, actor, self.owner_id, true, false).await?;
            let changed = query(include_str!("api_keys_revoke.sql"))
                .bind(key_id)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?
                .rows_affected();
            if changed == 0 {
                return Err(AdminError::NotFound);
            }
            Ok(())
        }
        .await;
        finish_admin(tx, result).await
    }
}

fn valid_scope(scope: &str) -> bool {
    scope == "*" || scope.ends_with(":*") || API_KEY_SCOPES.contains(&scope)
}

fn decode_api_key(row: yoink_db::Row) -> Result<ApiKey, AdminError> {
    let raw_scopes: String = row.try_get(3).map_err(|_| AdminError::Unavailable)?;
    let scopes: Vec<String> =
        serde_json::from_str(&raw_scopes).map_err(|_| AdminError::Unavailable)?;
    if scopes.iter().any(|scope| !valid_scope(scope)) {
        return Err(AdminError::Unavailable);
    }
    Ok(ApiKey {
        id: row.try_get(0).map_err(|_| AdminError::Unavailable)?,
        name: row.try_get(1).map_err(|_| AdminError::Unavailable)?,
        prefix: row.try_get(2).map_err(|_| AdminError::Unavailable)?,
        scopes,
        created_by: row.try_get(4).map_err(|_| AdminError::Unavailable)?,
        last_used_at: row.try_get(5).map_err(|_| AdminError::Unavailable)?,
        expires_at: row.try_get(6).map_err(|_| AdminError::Unavailable)?,
        revoked: row.try_get(7).map_err(|_| AdminError::Unavailable)?,
        created_at: row.try_get(8).map_err(|_| AdminError::Unavailable)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permits_known_scopes_and_legacy_wildcards() {
        assert!(valid_scope("*"));
        assert!(valid_scope("users:read"));
        assert!(valid_scope("users:*"));
        assert!(!valid_scope("unknown:read"));
        assert!(!valid_scope(""));
    }
}
