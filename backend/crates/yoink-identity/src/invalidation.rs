//! Transport-neutral committed identity/access change notifications.
use crate::{IdentityError, IdentityStore};
use yoink_db::query;

impl IdentityStore {
    /// Bounded/coalesced signal only: subscribers must reload state, never cache
    /// the role or locale supplied by a mutation request. No receiver is required.
    pub fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.changes.subscribe()
    }

    pub(crate) fn invalidate(&self) {
        self.changes.send_replace(());
    }

    pub async fn access_groups_after(&self, after: i64) -> Result<Vec<i64>, IdentityError> {
        query(include_str!("sql/access_groups.sql"))
            .bind(after)
            .fetch_all(&self.database)
            .await?
            .into_iter()
            .map(|row| row.try_get(0).map_err(|_| IdentityError::Database))
            .collect()
    }

    /// Keyset-paged identity/chat targets, including blocked users and disabled
    /// groups. No authentication filter: consumers need to revoke stale state.
    /// Each registered user is considered in every known group, since auto-grant
    /// applies even without an explicit membership policy.
    pub async fn access_targets_after(
        &self,
        after: (i64, i64),
    ) -> Result<Vec<(i64, i64)>, IdentityError> {
        query(include_str!("sql/access_targets.sql"))
            .bind(after.0)
            .bind(after.1)
            .fetch_all(&self.database)
            .await?
            .into_iter()
            .map(|row| {
                Ok((
                    row.try_get(0).map_err(|_| IdentityError::Database)?,
                    row.try_get(1).map_err(|_| IdentityError::Database)?,
                ))
            })
            .collect()
    }
}
