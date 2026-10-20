use chrono::Utc;
use serde::{Deserialize, Serialize};
use yoink_db::{Transaction, query};

use crate::{IdentityError, IdentityStore};

/// Transport availability only. These records never grant or revoke authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MenuContact {
    pub source_id: i64,
    pub chat_id: i64,
    /// Zero is the bot's chat scope; positive values are group member scopes.
    pub user_id: i64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct MenuContactObservation {
    /// Unix microseconds, ordered before the Telegram update identifier.
    pub occurred_at: i64,
    pub update_id: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct MenuContactLease {
    contact: MenuContact,
    revision: u64,
    chat_revision: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    revision: u64,
    unavailable: bool,
    observed: Option<MenuContactObservation>,
}

impl MenuContact {
    fn validate(self) -> Result<(), IdentityError> {
        if self.source_id <= 0
            || self.chat_id == 0
            || self.user_id < 0
            || (self.chat_id > 0 && self.user_id != 0)
        {
            return Err(IdentityError::InvalidMenuContact);
        }
        Ok(())
    }

    fn chat(self) -> Self {
        Self { user_id: 0, ..self }
    }

    fn key(self) -> String {
        // The reserved namespace fits the existing 64-character key constraint.
        // Keeping transport metadata here preserves older images' schema rollback.
        format!("mc:{}:{}:{}", self.source_id, self.chat_id, self.user_id)
    }
}

impl State {
    fn advance(&mut self) -> Result<(), IdentityError> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(IdentityError::InvalidMenuContact)?;
        Ok(())
    }
}

async fn read(tx: &Transaction<'_>, contact: MenuContact) -> Result<State, IdentityError> {
    let row = query(include_str!("sql/menu_contact_read.sql"))
        .bind(contact.key())
        .fetch_optional(&**tx)
        .await?;
    match row {
        None => Ok(State::default()),
        Some(row) => serde_json::from_str(&row.try_get::<String, _>(0)?)
            .map_err(|_| IdentityError::InvalidMenuContact),
    }
}

async fn write(
    tx: &Transaction<'_>,
    contact: MenuContact,
    state: &State,
) -> Result<(), IdentityError> {
    query(include_str!("sql/menu_contact_write.sql"))
        .bind(contact.key())
        .bind(serde_json::to_string(state).map_err(|_| IdentityError::InvalidMenuContact)?)
        .bind(Utc::now())
        .execute(&**tx)
        .await?;
    Ok(())
}

impl IdentityStore {
    /// Capture revisions before a request. Never hold a database transaction over I/O.
    pub async fn begin_menu_contact(
        &self,
        contact: MenuContact,
    ) -> Result<Option<MenuContactLease>, IdentityError> {
        contact.validate()?;
        let mut connection = self.database.connection().await?;
        let tx = Transaction::new(
            &mut connection,
            turso::transaction::TransactionBehavior::Deferred,
        )
        .await
        .map_err(|_| IdentityError::Database)?;
        let state = read(&tx, contact).await?;
        let chat = if contact.user_id == 0 {
            State::default()
        } else {
            read(&tx, contact.chat()).await?
        };
        let lease = (!state.unavailable && !chat.unavailable).then_some(MenuContactLease {
            contact,
            revision: state.revision,
            chat_revision: chat.revision,
        });
        tx.commit().await.map_err(|_| IdentityError::Database)?;
        Ok(lease)
    }

    /// A late request cannot invalidate a contact restored by newer inbound evidence.
    pub async fn reject_menu_contact(
        &self,
        lease: MenuContactLease,
        whole_chat: bool,
    ) -> Result<(), IdentityError> {
        let mut connection = self.database.connection().await?;
        let tx = Transaction::new(
            &mut connection,
            turso::transaction::TransactionBehavior::Immediate,
        )
        .await
        .map_err(|_| IdentityError::Database)?;
        let (contact, expected_revision) = if whole_chat && lease.contact.user_id != 0 {
            (lease.contact.chat(), lease.chat_revision)
        } else {
            (lease.contact, lease.revision)
        };
        let mut state = read(&tx, contact).await?;
        if state.revision == expected_revision {
            state.advance()?;
            state.unavailable = true;
            write(&tx, contact, &state).await?;
        }
        tx.commit().await.map_err(|_| IdentityError::Database)?;
        Ok(())
    }

    /// Only trusted Telegram activity calls this, never role/configuration changes.
    pub async fn restore_menu_contact(
        &self,
        contact: MenuContact,
        observation: MenuContactObservation,
    ) -> Result<bool, IdentityError> {
        self.observe_menu_contact(contact, observation, true).await
    }

    pub async fn observe_menu_contact(
        &self,
        contact: MenuContact,
        observation: MenuContactObservation,
        available: bool,
    ) -> Result<bool, IdentityError> {
        contact.validate()?;
        if observation.occurred_at < 0 {
            return Err(IdentityError::InvalidMenuContact);
        }
        let mut connection = self.database.connection().await?;
        let tx = Transaction::new(
            &mut connection,
            turso::transaction::TransactionBehavior::Immediate,
        )
        .await
        .map_err(|_| IdentityError::Database)?;
        let restored = observe(&tx, contact, observation, available).await?;
        tx.commit().await.map_err(|_| IdentityError::Database)?;
        drop(connection);
        if restored {
            self.invalidate();
        }
        Ok(restored)
    }
}

pub(super) async fn observe(
    tx: &Transaction<'_>,
    contact: MenuContact,
    observation: MenuContactObservation,
    available: bool,
) -> Result<bool, IdentityError> {
    let mut state = read(tx, contact).await?;
    if state.observed.is_some_and(|old| observation <= old) {
        return Ok(false);
    }
    let restored = state.unavailable && available;
    state.advance()?;
    state.unavailable = !available;
    state.observed = Some(observation);
    write(tx, contact, &state).await?;
    Ok(restored)
}
