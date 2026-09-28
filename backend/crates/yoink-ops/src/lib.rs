mod archive;
mod pipeline;
#[cfg(test)]
mod tests;

use std::{path::PathBuf, time::Duration};
use tokio_util::sync::CancellationToken;
use yoink_storage::Store;

pub use yoink_db::publication::PublicationError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Publication(#[from] PublicationError),
    #[error("backup source must be an owned private regular file")]
    Source,
    #[error("backup is truncated, corrupt, unsupported, or exceeds the byte limit")]
    Archive,
    #[error("native database unavailable; check process ownership and private path permissions")]
    Database,
    #[error(
        "database schema is not the current verified schema; migrate the source explicitly before backup"
    )]
    Schema,
    #[error("native snapshot failed; check free space and source health")]
    Snapshot,
    #[error("database integrity check failed")]
    Integrity,
    #[error("database foreign-key validation failed")]
    ForeignKey,
    #[error("native checkpoint did not produce a standalone database")]
    Checkpoint,
    #[error("backup/restore storage operation failed")]
    Storage,
    #[error("backup/restore cancelled before publication")]
    Cancelled,
    #[error("backup/restore worker failed; inspect the destination before retrying")]
    Task,
    #[error("max_bytes and acquire_timeout must be positive, acquire_timeout at most 300 seconds")]
    Options,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Report {
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub max_bytes: u64,
    pub acquire_timeout: Duration,
    #[cfg(test)]
    pause: Option<std::sync::Arc<tests::Pause>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024 * 1024 * 1024,
            acquire_timeout: Duration::from_secs(30),
            #[cfg(test)]
            pause: None,
        }
    }
}

impl Options {
    fn validate(&self) -> Result<()> {
        if self.max_bytes == 0
            || self.acquire_timeout.is_zero()
            || self.acquire_timeout > Duration::from_secs(300)
        {
            return Err(Error::Options);
        }
        Ok(())
    }
}

/// Snapshot an owned Store. Ordinary writes wait during the native snapshot phase.
/// Dropping the caller requests cancellation; the worker retains resources until safe cleanup.
pub async fn backup(
    store: Store,
    destination: PathBuf,
    options: Options,
    cancel: CancellationToken,
) -> Result<Report> {
    options.validate()?;
    let cancel = cancel.child_token();
    let _cancel_on_drop = cancel.clone().drop_guard();
    tokio::spawn(pipeline::backup(store, destination, options, cancel))
        .await
        .map_err(|_| Error::Task)?
}

/// Restore only an checksum-validated backup envelope, never a raw live DB/WAL copy.
/// The destination must not exist. No runtime setting or deployment is switched.
pub async fn restore(
    source: PathBuf,
    destination: PathBuf,
    options: Options,
    cancel: CancellationToken,
) -> Result<Report> {
    options.validate()?;
    let cancel = cancel.child_token();
    let _cancel_on_drop = cancel.clone().drop_guard();
    tokio::spawn(pipeline::restore(source, destination, options, cancel))
        .await
        .map_err(|_| Error::Task)?
}

pub(crate) fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
