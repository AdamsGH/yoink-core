use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;
use yoink_domain::DownloadRequest;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgressStage {
    Fetching,
    Processing,
    WritingArtifact,
}
impl ProgressStage {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Fetching => "fetching",
            Self::Processing => "processing",
            Self::WritingArtifact => "writing_artifact",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Queued,
    Running,
    Delivering,
    #[serde(rename = "delivery_unknown")]
    DeliveryUnknown,
    DeliveryPartial,
    Succeeded,
    Failed,
    Cancelled,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdmissionPolicy {
    pub max_attempts: u32,
    pub retry_delay: Duration,
    pub max_pending_global: u32,
    pub max_pending_actor: u32,
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
    pub cache_bypassed: bool,
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
            .field("cache_bypassed", &self.cache_bypassed)
            .field("lease_expires_at", &self.lease_expires_at)
            .finish()
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseHeartbeat {
    pub lease_expires_at: DateTime<Utc>,
    pub cancel_requested: bool,
}

#[derive(Clone, Debug)]
pub struct OperationRecord {
    pub operation_id: Uuid,
    pub actor_id: u64,
    pub target: Option<yoink_domain::DeliveryTarget>,
    pub status: OperationStatus,
    pub attempt: u32,
    pub max_attempts: u32,
    pub cancel_requested: bool,
    pub cache_bypassed: bool,
    pub available_at: DateTime<Utc>,
    pub delivered_message_id: Option<i64>,
    pub planned_artifact_count: Option<u32>,
    pub delivered_artifact_count: u32,
    pub delivery_started_at: Option<DateTime<Utc>>,
    pub progress_percent: Option<u8>,
    pub progress_stage: Option<ProgressStage>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
