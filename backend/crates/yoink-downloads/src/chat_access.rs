use uuid::Uuid;
use yoink_access::{AccessPolicy, AccessScope, Role};
use yoink_domain::DeliveryTarget;
use yoink_storage::OperationRecord;

use crate::{DownloadError, DownloadService, map_store};

impl DownloadService {
    /// Applies to admission, execution, delivery and progress, including persisted jobs.
    pub fn with_group_access(mut self, enabled: bool) -> Self {
        self.groups_enabled = enabled;
        self
    }

    pub async fn authorize_chat(
        &self,
        actor_id: u64,
        target: DeliveryTarget,
    ) -> Result<(), DownloadError> {
        self.authorize_target(actor_id, Some(target))
            .await
            .map(|_| ())
    }

    /// Transport queries must not publish a private or other-topic operation into this chat.
    pub async fn status_in_chat(
        &self,
        actor_id: u64,
        id: Uuid,
        current: DeliveryTarget,
    ) -> Result<Option<OperationRecord>, DownloadError> {
        self.authorize_chat(actor_id, current).await?;
        let record = self.store.get(actor_id, id).await.map_err(map_store)?;
        let Some(record) = record else {
            return Ok(None);
        };
        let matches = match record.target {
            Some(target) => {
                target.bot_id() == current.bot_id()
                    && target.chat_id() == current.chat_id()
                    && target.thread_id() == current.thread_id()
            }
            None => current.chat_id() == actor_id as i64 && current.thread_id().is_none(),
        };
        if matches { Ok(Some(record)) } else { Ok(None) }
    }

    pub async fn cancel_in_chat(
        &self,
        actor_id: u64,
        id: Uuid,
        current: DeliveryTarget,
    ) -> Result<bool, DownloadError> {
        if self.status_in_chat(actor_id, id, current).await?.is_none() {
            return Ok(false);
        }
        self.store
            .request_cancel(actor_id, id)
            .await
            .map_err(map_store)
    }

    pub(super) async fn authorize_target(
        &self,
        actor_id: u64,
        target: Option<DeliveryTarget>,
    ) -> Result<yoink_identity::IdentitySnapshot, DownloadError> {
        if actor_id == 0 || actor_id > i64::MAX as u64 {
            return Err(DownloadError::Denied);
        }
        if target.is_some_and(|target| {
            target.bot_id() != self.delivery.bot_id()
                || (target.chat_id() > 0
                    && (target.chat_id() != actor_id as i64 || target.thread_id().is_some()))
                || (target.chat_id() < 0 && !self.groups_enabled)
                || target
                    .thread_id()
                    .is_some_and(|id| id > i64::from(i32::MAX))
        }) {
            return Err(DownloadError::Denied);
        }
        let group = target
            .filter(|target| target.chat_id() < 0)
            .map(|target| (target.chat_id(), target.thread_id()));
        let snapshot = self
            .identities
            .snapshot(actor_id as i64, group)
            .await
            .map_err(|_| DownloadError::Database)?
            .ok_or(DownloadError::Denied)?;
        let policy = AccessPolicy {
            min_role: Role::User,
            scopes: vec![AccessScope::Private, AccessScope::Group],
            check_group_enabled: true,
            check_thread_policy: true,
        };
        if snapshot.authorize(&policy, None).decision.allowed {
            Ok(snapshot)
        } else {
            Err(DownloadError::Denied)
        }
    }
}
