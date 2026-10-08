//! Insight use cases and product-owned adapters. Content retrieval is delegated to
//! a confined worker; provider credentials never cross the frontend boundary.
pub mod content;
pub mod http;
pub mod inbox;
mod management;
pub mod models;
mod stats;
pub mod worker_content;
pub use management::{ByokTest, ByokUpdate, ModelProbeOptions, SettingsUpdate};
pub mod oauth;
pub mod prompts;
pub mod provider;
pub mod research;
pub mod service;
pub mod telegram;
pub use service::{Config, Insight, Request, Summary};
use std::{future::Future, pin::Pin};
pub type FutureResult<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Code(&'static str),
    #[error("database_error")]
    Database,
}
impl From<yoink_db::Error> for Error {
    fn from(_: yoink_db::Error) -> Self {
        Self::Database
    }
}
impl From<yoink_identity::IdentityError> for Error {
    fn from(_: yoink_identity::IdentityError) -> Self {
        Self::Code("access_denied")
    }
}
pub(crate) fn invalid() -> Error {
    Error::Code("invalid_request")
}

mod streaming;
