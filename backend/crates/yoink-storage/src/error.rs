use thiserror::Error;
#[derive(Debug, Error, Clone, Copy, Eq, PartialEq)]
pub enum StoreError {
    #[error("invalid storage input")]
    InvalidInput,
    #[error("idempotency key conflicts with an existing request")]
    Conflict,
    #[error("pending operation capacity has been reached")]
    Capacity,
    #[error("download time quota exceeded; retry in {retry_after_secs} seconds")]
    QuotaExceeded { retry_after_secs: u64 },
    #[error("operation or lease is no longer available")]
    NotFoundOrStale,
    #[error("storage operation is not authorized")]
    Unauthorized,
    #[error("storage database operation failed")]
    Database,
    #[error("storage migration {version} failed during {stage}")]
    Migration { version: i64, stage: &'static str },
    #[error("storage schema is missing or incompatible")]
    Schema,
    #[error("stored request data is invalid")]
    InvalidStoredData,
}
