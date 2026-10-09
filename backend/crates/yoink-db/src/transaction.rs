use std::ops::Deref;

use turso::transaction::{DropBehavior, TransactionBehavior};

/// Keeps drop protection through terminal I/O without scheduling a redundant
/// rollback when Turso has already left the explicit transaction.
pub struct Transaction<'a> {
    inner: turso::transaction::Transaction<'a>,
}

impl<'a> Transaction<'a> {
    pub async fn new(
        connection: &'a mut turso::Connection,
        behavior: TransactionBehavior,
    ) -> turso::Result<Self> {
        Ok(Self {
            inner: turso::transaction::Transaction::new(connection, behavior).await?,
        })
    }

    pub async fn commit(self) -> turso::Result<()> {
        // Keep this wrapper alive while execution is Pending or returns an
        // error; the SDK's consuming commit cannot normalize either drop path.
        self.inner.execute("COMMIT", ()).await?;
        Ok(())
    }

    pub async fn rollback(self) -> turso::Result<()> {
        if !self.inner.is_autocommit()? {
            self.inner.execute("ROLLBACK", ()).await?;
        }
        Ok(())
    }
}

impl Deref for Transaction<'_> {
    type Target = turso::Connection;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        // Turso 0.7.2 changes autocommit before COMMIT I/O finishes. Its default
        // Drop would install a rollback marker that can poison future queries.
        if matches!(self.inner.is_autocommit(), Ok(true)) {
            self.inner.set_drop_behavior(DropBehavior::Ignore);
        }
    }
}
