use serde_json::Value;
use yoink_identity::{IdentityStore, MenuContact, MenuContactObservation};

/// Called before polling acknowledges the original Telegram update.
pub(crate) async fn observe(
    store: &IdentityStore,
    source_id: i64,
    update: &Value,
) -> Result<(), ()> {
    let Some(update_id) = update
        .get("update_id")
        .and_then(Value::as_u64)
        .and_then(|id| u32::try_from(id).ok())
    else {
        return Ok(());
    };

    if let Some(event) = update.get("my_chat_member") {
        let chat = &event["chat"];
        let new = &event["new_chat_member"];
        let Some(chat_id) = chat["id"].as_i64().filter(|id| *id > 0) else {
            return Ok(());
        };
        if chat["type"] == "private"
            && matches!(new["status"].as_str(), Some("member" | "kicked"))
            && new["user"]["id"].as_i64() == Some(source_id)
            && new["user"]["is_bot"] == true
            && event["old_chat_member"]["user"]["id"].as_i64() == Some(source_id)
            && event["from"]["id"].as_i64() == Some(chat_id)
            && event["from"]["is_bot"] == false
            && let Some(date) = event["date"]
                .as_i64()
                .filter(|date| *date >= 0)
                .and_then(|date| date.checked_mul(1_000_000))
        {
            store
                .observe_menu_contact(
                    MenuContact {
                        source_id,
                        chat_id,
                        user_id: 0,
                    },
                    MenuContactObservation {
                        occurred_at: date,
                        update_id,
                    },
                    new["status"] == "member",
                )
                .await
                .map_err(|_| ())?;
        }
        return Ok(());
    }

    // Callback messages can be old or inaccessible and lack the callback's
    // event date. They cannot prove that a newer contact failure is obsolete.
    let Some(message) = update
        .get("message")
        .or_else(|| update.get("edited_message"))
    else {
        return Ok(());
    };
    let Some(chat_id) = message["chat"]["id"].as_i64().filter(|id| *id != 0) else {
        return Ok(());
    };
    let Some(user_id) = message["from"]["id"].as_i64().filter(|id| *id > 0) else {
        return Ok(());
    };
    if message["from"]["is_bot"] != false
        || message.get("sender_chat").is_some_and(|v| !v.is_null())
    {
        return Ok(());
    }
    let private = message["chat"]["type"] == "private" && chat_id == user_id;
    let group = matches!(
        message["chat"]["type"].as_str(),
        Some("group" | "supergroup")
    ) && chat_id < 0;
    if !private && !group {
        return Ok(());
    }
    let date = if update.get("edited_message").is_some() {
        &message["edit_date"]
    } else {
        &message["date"]
    };
    if let Some(date) = date
        .as_i64()
        .filter(|date| *date >= 0)
        .and_then(|date| date.checked_mul(1_000_000))
    {
        restore(
            store,
            source_id,
            chat_id,
            group.then_some(user_id),
            MenuContactObservation {
                occurred_at: date,
                update_id,
            },
        )
        .await?;
    }
    Ok(())
}

async fn restore(
    store: &IdentityStore,
    source_id: i64,
    chat_id: i64,
    user: Option<i64>,
    event: MenuContactObservation,
) -> Result<(), ()> {
    store
        .restore_menu_contact(
            MenuContact {
                source_id,
                chat_id,
                user_id: 0,
            },
            event,
        )
        .await
        .map_err(|_| ())?;
    if let Some(user_id) = user {
        store
            .restore_menu_contact(
                MenuContact {
                    source_id,
                    chat_id,
                    user_id,
                },
                event,
            )
            .await
            .map_err(|_| ())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    async fn setup() -> (tempfile::TempDir, yoink_storage::Store, IdentityStore) {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let db = yoink_storage::Store::open(
            &dir.path().join("contacts.db"),
            true,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        db.migrate().await.unwrap();
        let store = IdentityStore::new(db.database().clone(), Some(42), false);
        (dir, db, store)
    }

    fn contact() -> MenuContact {
        MenuContact {
            source_id: 123456,
            chat_id: 43,
            user_id: 0,
        }
    }

    async fn reject(store: &IdentityStore) {
        let lease = store.begin_menu_contact(contact()).await.unwrap().unwrap();
        store.reject_menu_contact(lease, true).await.unwrap();
    }

    fn message(update_id: u32) -> Value {
        json!({ "update_id": update_id, "message": {
            "date": 100 + update_id, "message_id": update_id,
            "chat": { "id": 43, "type": "private" },
            "from": { "id": 43, "is_bot": false }, "text": "/start",
        }})
    }

    fn membership(update_id: u32, status: &str) -> Value {
        json!({ "update_id": update_id, "my_chat_member": {
            "date": 100 + update_id, "chat": { "id": 43, "type": "private" },
            "from": { "id": 43, "is_bot": false },
            "old_chat_member": { "user": { "id": 123456, "is_bot": true }, "status": "kicked" },
            "new_chat_member": { "user": { "id": 123456, "is_bot": true }, "status": status },
        }})
    }

    #[tokio::test]
    async fn trusted_messages_restore_contacts_but_replays_do_not() {
        let (_dir, _db, store) = setup().await;
        reject(&store).await;
        observe(&store, 123456, &message(1)).await.unwrap();
        assert!(store.begin_menu_contact(contact()).await.unwrap().is_some());
        reject(&store).await;
        observe(&store, 123456, &message(1)).await.unwrap();
        assert!(store.begin_menu_contact(contact()).await.unwrap().is_none());
        observe(&store, 123456, &message(2)).await.unwrap();
        assert!(store.begin_menu_contact(contact()).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn private_block_and_unblock_events_are_scoped_and_ordered() {
        let (_dir, _db, store) = setup().await;
        observe(&store, 123456, &membership(2, "kicked"))
            .await
            .unwrap();
        assert!(store.begin_menu_contact(contact()).await.unwrap().is_none());
        observe(&store, 123456, &membership(1, "member"))
            .await
            .unwrap();
        assert!(store.begin_menu_contact(contact()).await.unwrap().is_none());
        observe(&store, 123456, &membership(3, "member"))
            .await
            .unwrap();
        assert!(store.begin_menu_contact(contact()).await.unwrap().is_some());
        observe(&store, 123456, &membership(2, "kicked"))
            .await
            .unwrap();
        assert!(store.begin_menu_contact(contact()).await.unwrap().is_some());
        reject(&store).await;
        observe(&store, 654321, &membership(4, "member"))
            .await
            .unwrap();
        assert!(store.begin_menu_contact(contact()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn callback_inline_and_invalid_actor_updates_do_not_restore_private_contact() {
        let (_dir, _db, store) = setup().await;
        reject(&store).await;
        let mut wrong_actor = message(1);
        wrong_actor["message"]["from"]["id"] = json!(44);
        let mut bot_actor = message(2);
        bot_actor["message"]["from"]["is_bot"] = json!(true);
        for raw in [
            wrong_actor,
            bot_actor,
            json!({"update_id": 3, "callback_query": {"from": {"id": 43}, "message": message(3)["message"]}}),
            json!({"update_id": 4, "inline_query": {"from": {"id": 43}}}),
        ] {
            observe(&store, 123456, &raw).await.unwrap();
            assert!(store.begin_menu_contact(contact()).await.unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn group_message_recovers_only_its_chat_and_sending_member() {
        let (_dir, _db, store) = setup().await;
        store
            .observe_membership(&yoink_identity::MembershipObservation {
                source_id: 123456,
                group_id: -100,
                update_id: 0,
                occurred_at: chrono::DateTime::from_timestamp(100, 0).unwrap(),
                title: None,
                subject: yoink_identity::MembershipSubject::Bot,
                status: yoink_identity::MemberStatus::Member,
                present: true,
                tag: yoink_identity::ObservedTag::Unknown,
            })
            .await
            .unwrap();
        let base = MenuContact {
            chat_id: -100,
            ..contact()
        };
        let sender = MenuContact {
            user_id: 43,
            ..base
        };
        let other = MenuContact {
            user_id: 44,
            ..base
        };
        for target in [sender, other, base] {
            let lease = store.begin_menu_contact(target).await.unwrap().unwrap();
            store.reject_menu_contact(lease, false).await.unwrap();
        }
        let mut raw = message(1);
        raw["message"]["chat"] = json!({"id": -100, "type": "supergroup"});
        observe(&store, 123456, &raw).await.unwrap();
        assert!(store.begin_menu_contact(sender).await.unwrap().is_some());
        assert!(store.begin_menu_contact(other).await.unwrap().is_none());
    }
}
