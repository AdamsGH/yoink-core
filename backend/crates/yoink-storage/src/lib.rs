//! Native Turso operation admission and fenced execution leases.
mod cache;
mod cache_key;
mod codec;
mod cut;
mod deferred_media;
mod error;
pub use deferred_media::{DeferredMediaState, TelegramMediaAdmission};
mod model;
mod progress;
mod quality;
mod quota;
mod schema;
mod store;
pub use cache::CachedBatch;
pub use cache_key::{MediaCacheKey, media_cache_key};
pub use cut::{CutSession, CutStage};
pub use error::StoreError;
pub use model::{
    Admission, AdmissionPolicy, Claim, ClaimPolicy, LeaseHeartbeat, OperationRecord,
    OperationStatus, ProgressStage, QuotaLimits,
};
pub use progress::{ProgressMessage, ProgressNotice};
pub use quality::{QualityChoice, QualitySession, QualityStage};
pub use store::{StopDeliveryReason, Store};

mod text;
pub use text::{TextEffect, TextIdentity, TextOutcome};
