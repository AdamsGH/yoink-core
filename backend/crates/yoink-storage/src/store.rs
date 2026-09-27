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
    model::{Admission, AdmissionPolicy, Claim, LeaseHeartbeat, OperationRecord, OperationStatus},
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

        let applied = sqlx::query(include_str!("../sql/verify_migration_history.sql"))
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
        self.admit_bounded(
            request,
            idempotency_key,
            AdmissionPolicy {
                max_attempts,
                retry_delay: Duration::ZERO,
                max_pending_global: 1000,
                max_pending_actor: 100,
            },
        )
        .await
    }

    pub async fn admit_bounded(
        &self,
        request: &DownloadRequest,
        idempotency_key: &str,
        policy: AdmissionPolicy,
    ) -> Result<Admission, StoreError> {
        validate_key(idempotency_key)?;
        validate_policy(policy)?;
        let request_json = codec::encode(request)?;
        let mut tx = self.pool.begin().await.map_err(|_| StoreError::Database)?;
        sqlx::query(include_str!("../sql/admit_lock.sql"))
            .execute(&mut *tx)
            .await
            .map_err(|_| StoreError::Database)?;
        let existing = sqlx::query(include_str!("../sql/admit_existing.sql"))
            .bind(request.actor_id() as i64)
            .bind(idempotency_key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| StoreError::Database)?;
        if let Some(row) = existing {
            let current: serde_json::Value =
                row.try_get("request").map_err(|_| StoreError::Database)?;
            if current != request_json {
                return Err(StoreError::Conflict);
            }
            let admission = Admission {
                operation_id: row.try_get("id").map_err(|_| StoreError::Database)?,
                created: false,
            };
            tx.commit().await.map_err(|_| StoreError::Database)?;
            return Ok(admission);
        }
        let counts = sqlx::query(include_str!("../sql/admit_counts.sql"))
            .bind(request.actor_id() as i64)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| StoreError::Database)?;
        let global: i64 = counts
            .try_get("global_count")
            .map_err(|_| StoreError::Database)?;
        let actor: i64 = counts
            .try_get("actor_count")
            .map_err(|_| StoreError::Database)?;
        if global >= policy.max_pending_global as i64 || actor >= policy.max_pending_actor as i64 {
            return Err(StoreError::Capacity);
        }
        let id = sqlx::query(include_str!("../sql/admit_insert.sql"))
            .bind(request.actor_id() as i64)
            .bind(idempotency_key)
            .bind(&request_json)
            .bind(policy.max_attempts as i32)
            .bind(policy.retry_delay.as_secs() as i32)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| StoreError::Database)?
            .try_get("id")
            .map_err(|_| StoreError::Database)?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(Admission {
            operation_id: id,
            created: true,
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
        Ok(self.heartbeat_state(claim, lease).await?.lease_expires_at)
    }

    pub async fn heartbeat_state(
        &self,
        claim: &Claim,
        lease: Duration,
    ) -> Result<LeaseHeartbeat, StoreError> {
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
        Ok(LeaseHeartbeat {
            lease_expires_at: row
                .try_get("lease_expires_at")
                .map_err(|_| StoreError::Database)?,
            cancel_requested: row
                .try_get("cancel_requested")
                .map_err(|_| StoreError::Database)?,
        })
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
                "delivering" => OperationStatus::Delivering,
                "delivery_unknown" => OperationStatus::DeliveryUnknown,
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
                available_at: row
                    .try_get("available_at")
                    .map_err(|_| StoreError::Database)?,
                delivered_message_id: row
                    .try_get("delivered_message_id")
                    .map_err(|_| StoreError::Database)?,
                delivery_started_at: row
                    .try_get("delivery_started_at")
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

    pub async fn begin_delivery(&self, claim: &Claim) -> Result<(), StoreError> {
        validate_claim(claim)?;
        let result = sqlx::query(include_str!("../sql/begin_delivery.sql"))
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

    pub async fn complete_delivery(
        &self,
        claim: &Claim,
        message_id: i32,
    ) -> Result<(), StoreError> {
        validate_claim(claim)?;
        if message_id <= 0 {
            return Err(StoreError::InvalidInput);
        }
        let result = sqlx::query(include_str!("../sql/complete_delivery.sql"))
            .bind(claim.operation_id)
            .bind(claim.attempt as i32)
            .bind(claim.lease_token)
            .bind(&claim.worker_id)
            .bind(message_id)
            .execute(&self.pool)
            .await
            .map_err(|_| StoreError::Database)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFoundOrStale);
        }
        Ok(())
    }

    pub async fn delivery_unknown(&self, claim: &Claim) -> Result<(), StoreError> {
        validate_claim(claim)?;
        let result = sqlx::query(include_str!("../sql/delivery_unknown.sql"))
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

    pub async fn reject_delivery(&self, claim: &Claim) -> Result<(), StoreError> {
        validate_claim(claim)?;
        let result = sqlx::query(include_str!("../sql/reject_delivery.sql"))
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

fn validate_policy(policy: AdmissionPolicy) -> Result<(), StoreError> {
    if !(1..=100).contains(&policy.max_attempts)
        || policy.retry_delay.subsec_nanos() != 0
        || policy.retry_delay > Duration::from_secs(300)
        || !(1..=10_000).contains(&policy.max_pending_global)
        || !(1..=10_000).contains(&policy.max_pending_actor)
    {
        return Err(StoreError::InvalidInput);
    }
    Ok(())
}
