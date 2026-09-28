//! Explicit PostgreSQL-to-Turso offline import support.
//!
//! This crate is intentionally not depended on by runtime services.
pub mod catalog;
pub mod config;
pub mod destination;
pub mod error;
mod pipeline;
pub mod source;

pub use config::ImportConfig;
pub use error::{ImportError, Result};

/// Import orchestration. Source sequence state is observed outside PostgreSQL's
/// MVCC snapshot; serial high-water preservation is therefore conservative, not
/// a snapshot-consistent guarantee.
pub async fn import(config: ImportConfig) -> Result<ImportReport> {
    config.validate()?;
    tokio::time::timeout(config.total_timeout, pipeline::execute(config))
        .await
        .map_err(|_| ImportError::Deadline)?
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImportReport {
    pub tables: usize,
    pub rows: u64,
}
