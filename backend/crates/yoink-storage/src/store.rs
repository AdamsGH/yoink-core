use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::{
    ConnectOptions, PgPool, Row,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use uuid::Uuid;
use yoink_domain::DownloadRequest;

use crate::{
    codec,
    error::StoreError,
    model::{Admission, Claim, OperationRecord, OperationStatus},
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub struct Store {
    pool: PgPool,
}

impl Store {
    pub async fn connect(
        database_url: &str,
        max_connections: u32,
        acquire_timeout: Duration,
    ) -> Result<Self, StoreError> {
        if max_connections == 0
            || max_connections > 32
            || acquire_timeout.is_zero()
            || acquire_timeout > Duration::from_secs(300)
        {
            return Err(StoreError::InvalidInput);
        }
        let options: PgConnectOptions = database_url.parse().map_err(|_| StoreError::Database)?;
        let options = options
            .options([("search_path", "yoink_runtime,public")])
            .disable_statement_logging();
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(acquire_timeout)
            .connect_with(options)
            .await
            .map_err(|_| StoreError::Database)?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<(), StoreError> {
        sqlx::raw_sql(include_str!("../sql/create_schema.sql"))
            .execute(&self.pool)
            .await
            .map_err(|_| StoreError::Database)?;
        MIGRATOR
            .run(&self.pool)
            .await
            .map_err(|_| StoreError::Schema)
    }

    pub async fn verify_schema(&self) -> Result<(), StoreError> {
        let relations = sqlx::query(include_str!("../sql/verify_schema.sql"))
            .fetch_one(&self.pool)
            .await
            .map_err(|_| StoreError::Schema)?;
        let has_migrations: bool = relations
            .try_get("has_migrations")
            .map_err(|_| StoreError::Schema)?;
        let has_operations: bool = relations
            .try_get("has_operations")
            .map_err(|_| StoreError::Schema)?;
        if !has_migrations || !has_operations {
            return Err(StoreError::Schema);
        }

        let applied = sqlx::query("SELECT version, checksum, success FROM yoink_runtime._sqlx_migrations ORDER BY version")
            .fetch_all(&self.pool)
            .await
            .map_err(|_| StoreError::Schema)?;
        let expected = MIGRATOR.iter().collect::<Vec<_>>();
        if applied.len() != expected.len() {
            return Err(StoreError::Schema);
        }
        for (row, migration) in applied.iter().zip(expected) {
            let version: i64 = row.try_get("version").map_err(|_| StoreError::Schema)?;
            let checksum: Vec<u8> = row.try_get("checksum").map_err(|_| StoreError::Schema)?;
            let success: bool = row.try_get("success").map_err(|_| StoreError::Schema)?;
            if !success
                || version != migration.version
                || checksum.as_slice() != migration.checksum.as_ref()
            {
                return Err(StoreError::Schema);
            }
        }
        Ok(())
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn admit(
        &self,
        request: &DownloadRequest,
        idempotency_key: &str,
        max_attempts: u32,
    ) -> Result<Admission, StoreError> {
        validate_key(idempotency_key)?;
        if max_attempts == 0 || max_attempts > 100 {
            return Err(StoreError::InvalidInput);
        }
        let request_json = codec::encode(request)?;
        let mut tx = self.pool.begin().await.map_err(|_| StoreError::Database)?;
        let inserted = sqlx::query(include_str!("../sql/admit_insert.sql"))
            .bind(request.actor_id() as i64)
            .bind(idempotency_key)
            .bind(&request_json)
            .bind(max_attempts as i32)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| StoreError::Database)?;
        let (id, created) = if let Some(row) = inserted {
            (row.try_get("id").map_err(|_| StoreError::Database)?, true)
        } else {
            let row = sqlx::query(include_str!("../sql/admit_existing.sql"))
                .bind(request.actor_id() as i64)
                .bind(idempotency_key)
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| StoreError::Database)?;
            let existing: serde_json::Value =
                row.try_get("request").map_err(|_| StoreError::Database)?;
            if existing != request_json {
                return Err(StoreError::Conflict);
            }
            (row.try_get("id").map_err(|_| StoreError::Database)?, false)
        };
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(Admission {
            operation_id: id,
            created,
        })
    }

    pub async fn claim(
        &self,
        worker_id: &str,
        lease: Duration,
    ) -> Result<Option<Claim>, StoreError> {
        validate_worker(worker_id)?;
        validate_lease(lease)?;
        sqlx::query(include_str!("../sql/expire_leases.sql"))
            .execute(&self.pool)
            .await
            .map_err(|_| StoreError::Database)?;
        let row = sqlx::query(include_str!("../sql/claim.sql"))
            .bind(worker_id)
            .bind(lease.as_secs() as i64)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| StoreError::Database)?;
        row.map(|row| {
            let request: serde_json::Value =
                row.try_get("request").map_err(|_| StoreError::Database)?;
            Ok(Claim {
                operation_id: row.try_get("id").map_err(|_| StoreError::Database)?,
                attempt: row
                    .try_get::<i32, _>("attempt")
                    .map_err(|_| StoreError::Database)? as u32,
                lease_token: row
                    .try_get("lease_token")
                    .map_err(|_| StoreError::Database)?,
                worker_id: row.try_get("worker_id").map_err(|_| StoreError::Database)?,
                request: codec::decode(request)?,
                cancel_requested: row
                    .try_get("cancel_requested")
                    .map_err(|_| StoreError::Database)?,
                lease_expires_at: row
                    .try_get::<DateTime<Utc>, _>("lease_expires_at")
                    .map_err(|_| StoreError::Database)?,
            })
        })
        .transpose()
    }

    pub async fn heartbeat(
        &self,
        claim: &Claim,
        lease: Duration,
    ) -> Result<DateTime<Utc>, StoreError> {
        validate_claim(claim)?;
        validate_lease(lease)?;
        let row = sqlx::query(include_str!("../sql/heartbeat.sql"))
            .bind(claim.operation_id)
            .bind(claim.attempt as i32)
            .bind(claim.lease_token)
            .bind(&claim.worker_id)
            .bind(lease.as_secs() as i64)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| StoreError::Database)?
            .ok_or(StoreError::NotFoundOrStale)?;
        row.try_get("lease_expires_at")
            .map_err(|_| StoreError::Database)
    }

    pub async fn complete(&self, claim: &Claim) -> Result<(), StoreError> {
        validate_claim(claim)?;
        let result = sqlx::query(include_str!("../sql/complete.sql"))
            .bind(claim.operation_id)
            .bind(claim.attempt as i32)
            .bind(claim.lease_token)
            .bind(&claim.worker_id)
            .execute(&self.pool)
            .await
            .map_err(|_| StoreError::Database)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFoundOrStale);
        }
        Ok(())
    }

    pub async fn fail(&self, claim: &Claim, retryable: bool) -> Result<(), StoreError> {
        validate_claim(claim)?;
        let row = sqlx::query(include_str!("../sql/fail.sql"))
            .bind(claim.operation_id)
            .bind(claim.attempt as i32)
            .bind(claim.lease_token)
            .bind(&claim.worker_id)
            .bind(retryable)
            .execute(&self.pool)
            .await
            .map_err(|_| StoreError::Database)?;
        if row.rows_affected() == 0 {
            return Err(StoreError::NotFoundOrStale);
        }
        Ok(())
    }

    pub async fn request_cancel(
        &self,
        actor_id: u64,
        operation_id: Uuid,
    ) -> Result<bool, StoreError> {
        if actor_id == 0 || actor_id > i64::MAX as u64 {
            return Err(StoreError::InvalidInput);
        }
        let row = sqlx::query(include_str!("../sql/cancel.sql"))
            .bind(actor_id as i64)
            .bind(operation_id)
            .execute(&self.pool)
            .await
            .map_err(|_| StoreError::Database)?;
        Ok(row.rows_affected() > 0)
    }

    pub async fn acknowledge_cancel(&self, claim: &Claim) -> Result<(), StoreError> {
        validate_claim(claim)?;
        let row = sqlx::query(include_str!("../sql/ack_cancel.sql"))
            .bind(claim.operation_id)
            .bind(claim.attempt as i32)
            .bind(claim.lease_token)
            .bind(&claim.worker_id)
            .execute(&self.pool)
            .await
            .map_err(|_| StoreError::Database)?;
        if row.rows_affected() == 0 {
            return Err(StoreError::NotFoundOrStale);
        }
        Ok(())
    }

    pub async fn get(
        &self,
        actor_id: u64,
        operation_id: Uuid,
    ) -> Result<Option<OperationRecord>, StoreError> {
        if actor_id == 0 || actor_id > i64::MAX as u64 {
            return Err(StoreError::InvalidInput);
        }
        let row = sqlx::query(include_str!("../sql/get_operation.sql"))
            .bind(actor_id as i64)
            .bind(operation_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| StoreError::Database)?;
        row.map(|row| {
            let status: String = row.try_get("status").map_err(|_| StoreError::Database)?;
            let status = match status.as_str() {
                "queued" => OperationStatus::Queued,
                "running" => OperationStatus::Running,
                "succeeded" => OperationStatus::Succeeded,
                "failed" => OperationStatus::Failed,
                "cancelled" => OperationStatus::Cancelled,
                _ => return Err(StoreError::InvalidStoredData),
            };
            Ok(OperationRecord {
                operation_id: row.try_get("id").map_err(|_| StoreError::Database)?,
                actor_id: row
                    .try_get::<i64, _>("actor_id")
                    .map_err(|_| StoreError::Database)? as u64,
                status,
                attempt: row
                    .try_get::<i32, _>("attempt")
                    .map_err(|_| StoreError::Database)? as u32,
                max_attempts: row
                    .try_get::<i32, _>("max_attempts")
                    .map_err(|_| StoreError::Database)? as u32,
                cancel_requested: row
                    .try_get("cancel_requested")
                    .map_err(|_| StoreError::Database)?,
                created_at: row
                    .try_get("created_at")
                    .map_err(|_| StoreError::Database)?,
                updated_at: row
                    .try_get("updated_at")
                    .map_err(|_| StoreError::Database)?,
            })
        })
        .transpose()
    }
}

fn validate_key(key: &str) -> Result<(), StoreError> {
    if key.is_empty()
        || key.len() > 128
        || !key.is_ascii()
        || key.bytes().any(|b| b.is_ascii_control())
    {
        Err(StoreError::InvalidInput)
    } else {
        Ok(())
    }
}

fn validate_worker(worker_id: &str) -> Result<(), StoreError> {
    if worker_id.is_empty()
        || worker_id.len() > 128
        || !worker_id.is_ascii()
        || worker_id.bytes().any(|b| b.is_ascii_control())
    {
        Err(StoreError::InvalidInput)
    } else {
        Ok(())
    }
}

fn validate_lease(lease: Duration) -> Result<(), StoreError> {
    if lease < Duration::from_secs(1) || lease > Duration::from_secs(300) {
        Err(StoreError::InvalidInput)
    } else {
        Ok(())
    }
}

fn validate_claim(claim: &Claim) -> Result<(), StoreError> {
    validate_worker(&claim.worker_id)?;
    if claim.attempt == 0 || claim.attempt > i32::MAX as u32 {
        Err(StoreError::InvalidInput)
    } else {
        Ok(())
    }
}
