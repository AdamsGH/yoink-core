use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use yoink_domain::DownloadRequest;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Admission {
    pub operation_id: Uuid,
    pub created: bool,
}

#[derive(Clone)]
pub struct Claim {
    pub operation_id: Uuid,
    pub attempt: u32,
    pub lease_token: Uuid,
    pub worker_id: String,
    pub request: DownloadRequest,
    pub cancel_requested: bool,
    pub lease_expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for Claim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Claim")
            .field("operation_id", &self.operation_id)
            .field("attempt", &self.attempt)
            .field("lease_token", &"[REDACTED]")
            .field("worker_id", &self.worker_id)
            .field("request", &self.request)
            .field("cancel_requested", &self.cancel_requested)
            .field("lease_expires_at", &self.lease_expires_at)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct OperationRecord {
    pub operation_id: Uuid,
    pub actor_id: u64,
    pub status: OperationStatus,
    pub attempt: u32,
    pub max_attempts: u32,
    pub cancel_requested: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
