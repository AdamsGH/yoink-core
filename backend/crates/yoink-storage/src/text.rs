//! Durable single-attempt text effects, independent of download claims.
use crate::{Store, StoreError};
use serde::{Deserialize, Serialize};
use yoink_db::{Executor, query};

/// Immutable identity. `message` is the incoming message for sends and the
/// confirmed bot message for edits. The trusted bot/topic are never inferred.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextIdentity {
    pub bot: i64,
    pub chat: i64,
    pub topic: Option<i32>,
    pub message: i32,
    pub actor: i64,
    pub item: Option<i64>,
    pub edit: bool,
    pub text: String,
    /// Immutable application snapshot (not just its rendered projection).
    pub payload: serde_json::Value,
}
impl TextIdentity {
    pub fn valid(&self) -> bool {
        self.bot > 0
            && self.chat != 0
            && self.actor > 0
            && self.message > 0
            && self.topic.is_none_or(|v| v > 0)
            && self.item.is_none_or(|v| v > 0)
            && serde_json::to_vec(&self.payload).is_ok_and(|v| v.len() <= 65536)
            && !self.text.is_empty()
            && self.text.encode_utf16().count() <= 4096
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextOutcome {
    /// Durable immutable command result; no external attempt has been admitted.
    Prepared,
    Unknown,
    Confirmed(i32),
    Sealed,
}
#[derive(Clone, Debug)]
pub struct TextEffect {
    pub token: String,
    pub identity: Option<TextIdentity>,
    pub outcome: TextOutcome,
    pub bound: bool,
}
fn decode(row: yoink_db::Row) -> Result<TextEffect, StoreError> {
    let identity: Option<String> = row.try_get("identity").map_err(|_| StoreError::Database)?;
    let receipt: Option<i32> = row.try_get("receipt").map_err(|_| StoreError::Database)?;
    let state: String = row.try_get("state").map_err(|_| StoreError::Database)?;
    Ok(TextEffect {
        token: row.try_get("token").map_err(|_| StoreError::Database)?,
        identity: identity
            .map(|v| serde_json::from_str(&v).map_err(|_| StoreError::InvalidStoredData))
            .transpose()?,
        outcome: match state.as_str() {
            "confirmed" => TextOutcome::Confirmed(receipt.ok_or(StoreError::InvalidStoredData)?),
            "sealed" => TextOutcome::Sealed,
            "prepared" => TextOutcome::Prepared,
            "sending" | "unknown" => TextOutcome::Unknown,
            _ => return Err(StoreError::InvalidStoredData),
        },
        bound: row
            .try_get::<i64, _>("bound")
            .map_err(|_| StoreError::Database)?
            == 1,
    })
}
impl Store {
    /// Commit sending before exactly one external attempt. False NEVER permits a
    /// send, even when the previous process died before reaching the API.
    pub async fn begin_text(
        &self,
        token: &str,
        identity: &TextIdentity,
    ) -> Result<bool, StoreError> {
        if token.is_empty() || token.len() > 256 || !identity.valid() {
            return Err(StoreError::InvalidInput);
        }
        let payload = serde_json::to_string(identity).map_err(|_| StoreError::InvalidInput)?;
        let c = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        c.batch("BEGIN IMMEDIATE")
            .await
            .map_err(|_| StoreError::Database)?;
        let result = async {
            let changed = query("INSERT INTO runtime_text_effects(token,identity,state) VALUES(?,?,'sending') ON CONFLICT(token) DO NOTHING")
                .bind(token).bind(&payload).execute(&*c).await.map_err(|_| StoreError::Database)?.rows_affected() == 1;
            let row = query("SELECT * FROM runtime_text_effects WHERE token=?").bind(token).fetch_one(&*c).await.map_err(|_| StoreError::Database)?;
            let effect = decode(row)?;
            if effect.outcome != TextOutcome::Sealed && effect.identity.as_ref() != Some(identity) { return Err(StoreError::Conflict); }
            let admitted = query("UPDATE runtime_text_effects SET state='sending' WHERE token=? AND state='prepared' AND identity=?").bind(token).bind(&payload).execute(&*c).await.map_err(|_| StoreError::Database)?.rows_affected()==1;
            Ok(changed || admitted)
        }.await;
        match result {
            Ok(v) => {
                c.batch("COMMIT").await.map_err(|_| StoreError::Database)?;
                Ok(v)
            }
            Err(e) => {
                let _ = c.batch("ROLLBACK").await;
                Err(e)
            }
        }
    }
    /// Caller owns the transaction. This is durable intent only; begin_text is
    /// still required before the sole external attempt. Used to atomically
    /// persist a rendered command result with its application mutation.
    pub async fn prepare_text_on<E: Executor>(
        c: &E,
        token: &str,
        identity: &TextIdentity,
    ) -> Result<(), StoreError> {
        if token.is_empty() || token.len() > 256 || !identity.valid() {
            return Err(StoreError::InvalidInput);
        }
        let raw = serde_json::to_string(identity).map_err(|_| StoreError::InvalidInput)?;
        query("INSERT INTO runtime_text_effects(token,identity,state) VALUES(?,?,'prepared') ON CONFLICT(token) DO NOTHING").bind(token).bind(raw).execute(c).await.map_err(|_| StoreError::Database)?;
        let effect = Self::text_effect_on(c, token)
            .await?
            .ok_or(StoreError::Database)?;
        if effect.identity.as_ref() != Some(identity) {
            return Err(StoreError::Conflict);
        }
        Ok(())
    }
    pub async fn text_effect_on<E: Executor>(
        c: &E,
        token: &str,
    ) -> Result<Option<TextEffect>, StoreError> {
        query("SELECT * FROM runtime_text_effects WHERE token=?")
            .bind(token)
            .fetch_optional(c)
            .await
            .map_err(|_| StoreError::Database)?
            .map(decode)
            .transpose()
    }
    pub async fn text_effect(&self, token: &str) -> Result<Option<TextEffect>, StoreError> {
        query("SELECT * FROM runtime_text_effects WHERE token=?")
            .bind(token)
            .fetch_optional(&self.database)
            .await
            .map_err(|_| StoreError::Database)?
            .map(decode)
            .transpose()
    }
    /// Atomic absence proof: inserts a permanent tombstone which late dispatch
    /// cannot cross. Existing sending rows remain unknown, never not-sent.
    pub async fn seal_text_absence(&self, token: &str) -> Result<TextEffect, StoreError> {
        if token.is_empty() || token.len() > 256 {
            return Err(StoreError::InvalidInput);
        }
        // A prepared intent is definite non-admission; sealing must also fence
        // its later prepared -> sending CAS.
        query("INSERT INTO runtime_text_effects(token,state) VALUES(?,'sealed') ON CONFLICT(token) DO UPDATE SET state='sealed',identity=NULL WHERE runtime_text_effects.state='prepared'").bind(token).execute(&self.database).await.map_err(|_| StoreError::Database)?;
        self.text_effect(token).await?.ok_or(StoreError::Database)
    }
    /// Records evidence only. No fresh grant is required after a confirmed effect.
    pub async fn confirm_text(&self, token: &str, message: i32) -> Result<(), StoreError> {
        if message <= 0 {
            return Err(StoreError::InvalidInput);
        }
        let changed = query("UPDATE runtime_text_effects SET state='confirmed',receipt=? WHERE token=? AND (state IN ('sending','unknown') OR (state='confirmed' AND receipt=?))")
            .bind(message).bind(token).bind(message).execute(&self.database).await.map_err(|_| StoreError::Database)?.rows_affected();
        if changed == 1 {
            Ok(())
        } else {
            Err(StoreError::Conflict)
        }
    }
    pub async fn unknown_text(&self, token: &str) -> Result<(), StoreError> {
        query("UPDATE runtime_text_effects SET state='unknown' WHERE token=? AND state='sending'")
            .bind(token)
            .execute(&self.database)
            .await
            .map_err(|_| StoreError::Database)?;
        Ok(())
    }
    pub async fn unbound_text_receipts(&self) -> Result<Vec<TextEffect>, StoreError> {
        query(
            "SELECT * FROM runtime_text_effects WHERE state='confirmed' AND bound=0 ORDER BY token",
        )
        .fetch_all(&self.database)
        .await
        .map_err(|_| StoreError::Database)?
        .into_iter()
        .map(decode)
        .collect()
    }
    /// Bounded transport-specific receipt recovery; completion edits and read
    /// commands cannot starve initial-item binding recovery.
    pub async fn unbound_text_receipts_page(
        &self,
        bot: i64,
        limit: usize,
    ) -> Result<Vec<TextEffect>, StoreError> {
        if bot <= 0 || !(1..=256).contains(&limit) {
            return Err(StoreError::InvalidInput);
        }
        query("SELECT * FROM runtime_text_effects WHERE state='confirmed' AND bound=0 AND json_extract(identity,'$.bot')=? AND json_extract(identity,'$.edit')=0 AND json_extract(identity,'$.item') IS NOT NULL AND coalesce(json_extract(identity,'$.payload.bind_item'),1)=1 ORDER BY token LIMIT ?")
            .bind(bot).bind(limit as i64).fetch_all(&self.database).await.map_err(|_| StoreError::Database)?.into_iter().map(decode).collect()
    }
    pub async fn mark_text_bound(&self, token: &str) -> Result<(), StoreError> {
        query("UPDATE runtime_text_effects SET bound=1 WHERE token=? AND state='confirmed'")
            .bind(token)
            .execute(&self.database)
            .await
            .map_err(|_| StoreError::Database)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, time::Duration};
    async fn fixture() -> (tempfile::TempDir, Arc<Store>) {
        let tmp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let store = Arc::new(
            Store::open(&tmp.path().join("ledger.db"), true, Duration::from_secs(5))
                .await
                .unwrap(),
        );
        store.migrate().await.unwrap();
        (tmp, store)
    }
    fn identity() -> TextIdentity {
        TextIdentity {
            bot: 123,
            chat: 42,
            topic: None,
            message: 10,
            actor: 42,
            item: Some(1),
            edit: false,
            payload: serde_json::json!({}),
            text: "saved".into(),
        }
    }
    #[tokio::test]
    async fn competing_dispatchers_have_exactly_one_admission_and_payload_conflicts() {
        let (_tmp, store) = fixture().await;
        let i = identity();
        let (a, b) = tokio::join!(store.begin_text("token", &i), store.begin_text("token", &i));
        assert_ne!(a.unwrap(), b.unwrap());
        for changed in [
            TextIdentity {
                payload: serde_json::json!({"classification":"changed"}),
                ..i.clone()
            },
            TextIdentity {
                actor: 43,
                ..i.clone()
            },
            TextIdentity {
                chat: 43,
                ..i.clone()
            },
            TextIdentity {
                topic: Some(7),
                ..i.clone()
            },
            TextIdentity {
                text: "changed".into(),
                ..i.clone()
            },
            TextIdentity {
                item: Some(2),
                ..i.clone()
            },
        ] {
            assert_eq!(
                store.begin_text("token", &changed).await,
                Err(StoreError::Conflict)
            );
        }
        assert_eq!(
            store.text_effect("token").await.unwrap().unwrap().outcome,
            TextOutcome::Unknown
        );
    }
    #[tokio::test]
    async fn seal_and_late_dispatch_are_atomic_and_sending_is_never_absence() {
        let (_tmp, store) = fixture().await;
        let i = identity();
        assert_eq!(
            store.seal_text_absence("sealed").await.unwrap().outcome,
            TextOutcome::Sealed
        );
        assert!(!store.begin_text("sealed", &i).await.unwrap());
        assert!(store.begin_text("sending", &i).await.unwrap());
        assert_eq!(
            store.seal_text_absence("sending").await.unwrap().outcome,
            TextOutcome::Unknown
        );
        let (send, seal) = tokio::join!(
            store.begin_text("race", &i),
            store.seal_text_absence("race")
        );
        assert_eq!(send.unwrap(), seal.unwrap().outcome == TextOutcome::Unknown);
    }
    #[tokio::test]
    async fn confirmation_conflicts_and_unknown_cannot_downgrade_receipts() {
        let (_tmp, store) = fixture().await;
        store.begin_text("confirmed", &identity()).await.unwrap();
        store.confirm_text("confirmed", 99).await.unwrap();
        store.confirm_text("confirmed", 99).await.unwrap();
        assert_eq!(
            store.confirm_text("confirmed", 100).await,
            Err(StoreError::Conflict)
        );
        store.unknown_text("confirmed").await.unwrap();
        assert_eq!(
            store
                .text_effect("confirmed")
                .await
                .unwrap()
                .unwrap()
                .outcome,
            TextOutcome::Confirmed(99)
        );
        assert_eq!(store.unbound_text_receipts().await.unwrap().len(), 1);
        store.mark_text_bound("confirmed").await.unwrap();
        assert!(store.unbound_text_receipts().await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn plain_text_utf16_bounds_reject_before_admission() {
        let (_tmp, store) = fixture().await;
        for text in [String::new(), "😀".repeat(2049)] {
            assert_eq!(
                store
                    .begin_text("bad", &TextIdentity { text, ..identity() })
                    .await,
                Err(StoreError::InvalidInput)
            );
        }
        assert!(store.text_effect("bad").await.unwrap().is_none());
    }
    #[tokio::test]
    async fn prepared_intent_needs_distinct_cas_and_sealing_fences_late_admission() {
        let (_tmp, store) = fixture().await;
        let i = identity();
        {
            let c = store.database().connection().await.unwrap();
            c.batch("BEGIN IMMEDIATE").await.unwrap();
            Store::prepare_text_on(&*c, "prepared", &i).await.unwrap();
            c.batch("COMMIT").await.unwrap();
        }
        assert_eq!(
            store
                .text_effect("prepared")
                .await
                .unwrap()
                .unwrap()
                .outcome,
            TextOutcome::Prepared
        );
        assert_eq!(
            store.confirm_text("prepared", 99).await,
            Err(StoreError::Conflict)
        );
        assert_eq!(
            store
                .begin_text(
                    "prepared",
                    &TextIdentity {
                        text: "changed".into(),
                        ..i.clone()
                    }
                )
                .await,
            Err(StoreError::Conflict)
        );
        let (left, right) = tokio::join!(
            store.begin_text("prepared", &i),
            store.begin_text("prepared", &i)
        );
        assert_ne!(left.unwrap(), right.unwrap());
        {
            let c = store.database().connection().await.unwrap();
            c.batch("BEGIN IMMEDIATE").await.unwrap();
            Store::prepare_text_on(&*c, "prepared-seal", &i)
                .await
                .unwrap();
            c.batch("COMMIT").await.unwrap();
        }
        assert_eq!(
            store
                .seal_text_absence("prepared-seal")
                .await
                .unwrap()
                .outcome,
            TextOutcome::Sealed
        );
        assert!(!store.begin_text("prepared-seal", &i).await.unwrap());
    }
}
