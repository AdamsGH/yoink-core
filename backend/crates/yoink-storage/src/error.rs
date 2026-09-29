use thiserror::Error;
#[derive(Debug, Error, Clone, Copy, Eq, PartialEq)]
pub enum StoreError {
    #[error("invalid storage input")]
    InvalidInput,
    #[error("idempotency key conflicts with an existing request")]
    Conflict,
    #[error("pending operation capacity has been reached")]
    Capacity,
    #[error("operation or lease is no longer available")]
    NotFoundOrStale,
    #[error("storage operation is not authorized")]
    Unauthorized,
    #[error("storage database operation failed")]
    Database,
    #[error("storage schema is missing or incompatible")]
    Schema,
    #[error("stored request data is invalid")]
    InvalidStoredData,
}
