use chrono::{DateTime, Utc};
use serde_json::Value;
use yoink_identity::TrustedProfile;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MembershipUpdate {
    pub update_id: u32,
    pub occurred_at: DateTime<Utc>,
    pub group_id: i64,
    pub title: Option<String>,
    pub bot_subject: bool,
    pub profile: TrustedProfile,
    pub status: String,
    pub present: bool,
    pub tag: Option<String>,
}

impl MembershipUpdate {
    pub(crate) fn into_observation(self, source_id: i64) -> yoink_identity::MembershipObservation {
        use yoink_identity::{MemberStatus, MembershipObservation, MembershipSubject, ObservedTag};
        let status = match self.status.as_str() {
            "creator" => MemberStatus::Creator,
            "administrator" => MemberStatus::Administrator,
            "member" => MemberStatus::Member,
            "restricted" => MemberStatus::Restricted,
            "left" => MemberStatus::Left,
            "kicked" => MemberStatus::Kicked,
            _ => unreachable!("parser validates member status"),
        };
        MembershipObservation {
            source_id,
            update_id: self.update_id,
            occurred_at: self.occurred_at,
            group_id: self.group_id,
            title: self.title,
            subject: if self.bot_subject {
                MembershipSubject::Bot
            } else {
                MembershipSubject::User(self.profile)
            },
            status,
            present: self.present,
            tag: if self.bot_subject {
                ObservedTag::Unknown
            } else {
                ObservedTag::Known(self.tag)
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InvalidMembershipUpdate;

/// Read the original envelope: teloxide 0.17 drops member tags during decoding.
pub(crate) fn parse(
    update: &Value,
    bot_id: i64,
) -> Result<Option<MembershipUpdate>, InvalidMembershipUpdate> {
    let bot = update.get("my_chat_member");
    let user = update.get("chat_member");
    let (event, bot_subject) = match (bot, user) {
        (Some(_), Some(_)) => return Err(InvalidMembershipUpdate),
        (Some(event), None) => (event, true),
        (None, Some(event)) => (event, false),
        (None, None) => return Ok(None),
    };
    let update_id = update
        .get("update_id")
        .and_then(Value::as_u64)
        .and_then(|id| u32::try_from(id).ok())
        .ok_or(InvalidMembershipUpdate)?;
    let chat = event.get("chat").ok_or(InvalidMembershipUpdate)?;
    match chat.get("type").and_then(Value::as_str) {
        Some("private" | "channel") => return Ok(None),
        Some("group" | "supergroup") => {}
        _ => return Err(InvalidMembershipUpdate),
    }
    let group_id = chat
        .get("id")
        .and_then(Value::as_i64)
        .filter(|id| *id < 0)
        .ok_or(InvalidMembershipUpdate)?;
    let title = optional_text(chat, "title")?;
    if title
        .as_ref()
        .is_some_and(|title| title.chars().count() > 256)
    {
        return Err(InvalidMembershipUpdate);
    }
    let occurred_at = event
        .get("date")
        .and_then(Value::as_i64)
        .filter(|date| *date >= 0)
        .and_then(|date| DateTime::from_timestamp(date, 0))
        .ok_or(InvalidMembershipUpdate)?;
    let new = event
        .get("new_chat_member")
        .ok_or(InvalidMembershipUpdate)?;
    let old = event
        .get("old_chat_member")
        .ok_or(InvalidMembershipUpdate)?;
    let subject = new.get("user").ok_or(InvalidMembershipUpdate)?;
    let user_id = subject
        .get("id")
        .and_then(Value::as_i64)
        .filter(|id| *id > 0)
        .ok_or(InvalidMembershipUpdate)?;
    if old
        .get("user")
        .and_then(|u| u.get("id"))
        .and_then(Value::as_i64)
        != Some(user_id)
    {
        return Err(InvalidMembershipUpdate);
    }
    let is_bot = subject
        .get("is_bot")
        .and_then(Value::as_bool)
        .ok_or(InvalidMembershipUpdate)?;
    if bot_subject {
        if user_id != bot_id || !is_bot {
            return Err(InvalidMembershipUpdate);
        }
    } else if is_bot {
        // Membership of another bot is not a human account or this bot's lifecycle.
        return Ok(None);
    }
    let status = new
        .get("status")
        .and_then(Value::as_str)
        .ok_or(InvalidMembershipUpdate)?;
    let present = match status {
        "creator" | "administrator" | "member" => true,
        "restricted" => new
            .get("is_member")
            .and_then(Value::as_bool)
            .ok_or(InvalidMembershipUpdate)?,
        "left" | "kicked" => false,
        _ => return Err(InvalidMembershipUpdate),
    };
    let tag = optional_text(new, "tag")?.filter(|tag| !tag.is_empty());
    Ok(Some(MembershipUpdate {
        update_id,
        occurred_at,
        group_id,
        title,
        bot_subject,
        profile: TrustedProfile {
            user_id,
            username: optional_text(subject, "username")?,
            first_name: optional_text(subject, "first_name")?,
            photo_url: None,
        },
        status: status.to_owned(),
        present,
        tag,
    }))
}

fn optional_text(object: &Value, key: &str) -> Result<Option<String>, InvalidMembershipUpdate> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(InvalidMembershipUpdate),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn update(status: &str) -> Value {
        json!({
            "update_id": 42,
            "chat_member": {
                "chat": {"id": -77, "type": "supergroup", "title": "Group"},
                "date": 1700000000,
                "from": {"id": 999, "is_bot": false, "first_name": "Actor"},
                "old_chat_member": {"user": {"id": 123}, "status": "left"},
                "new_chat_member": {
                    "user": {"id": 123, "is_bot": false, "first_name": "Subject"},
                    "status": status,
                    "tag": "Загрузки"
                }
            }
        })
    }

    #[test]
    fn private_blocks_and_channel_lifecycle_are_not_group_memberships() {
        for (chat_type, chat_id) in [("private", 123), ("channel", -77)] {
            let mut value = update("member");
            let event = value
                .as_object_mut()
                .unwrap()
                .remove("chat_member")
                .unwrap();
            value["my_chat_member"] = event;
            value["my_chat_member"]["chat"]["type"] = json!(chat_type);
            value["my_chat_member"]["chat"]["id"] = json!(chat_id);
            assert_eq!(parse(&value, 7).unwrap(), None);
        }
    }

    #[test]
    fn subject_and_tag_come_from_original_new_member_not_actor() {
        let result = parse(&update("member"), 7).unwrap().unwrap();
        assert_eq!(result.profile.user_id, 123);
        assert_eq!(result.tag.as_deref(), Some("Загрузки"));
        assert_eq!(result.occurred_at.timestamp(), 1700000000);
        assert!(result.present);
        assert!(!result.bot_subject);
    }

    #[test]
    fn restricted_presence_is_explicit() {
        let mut value = update("restricted");
        assert_eq!(parse(&value, 7), Err(InvalidMembershipUpdate));
        value["chat_member"]["new_chat_member"]["is_member"] = json!(false);
        assert!(!parse(&value, 7).unwrap().unwrap().present);
        value["chat_member"]["new_chat_member"]["is_member"] = json!(true);
        assert!(parse(&value, 7).unwrap().unwrap().present);
    }

    #[test]
    fn missing_or_empty_tag_is_authoritative_clear_not_old_tag() {
        let mut value = update("member");
        value["chat_member"]["old_chat_member"]["tag"] = json!("Old");
        value["chat_member"]["new_chat_member"]
            .as_object_mut()
            .unwrap()
            .remove("tag");
        assert_eq!(parse(&value, 7).unwrap().unwrap().tag, None);
        value["chat_member"]["new_chat_member"]["tag"] = json!("");
        assert_eq!(parse(&value, 7).unwrap().unwrap().tag, None);
        value["chat_member"]["new_chat_member"]["tag"] = json!(123);
        assert_eq!(parse(&value, 7), Err(InvalidMembershipUpdate));
    }

    #[test]
    fn rejects_ambiguous_or_inconsistent_subjects_and_invalid_chat_types() {
        let mut value = update("member");
        value["chat_member"]["old_chat_member"]["user"]["id"] = json!(124);
        assert_eq!(parse(&value, 7), Err(InvalidMembershipUpdate));
        let mut value = update("member");
        value["chat_member"]["chat"]["type"] = json!("invalid");
        assert_eq!(parse(&value, 7), Err(InvalidMembershipUpdate));
        let mut value = update("member");
        value["my_chat_member"] = value["chat_member"].clone();
        assert_eq!(parse(&value, 7), Err(InvalidMembershipUpdate));
    }

    #[test]
    fn bot_lifecycle_requires_this_bot_and_other_bots_are_ignored() {
        let mut value = update("member");
        value["chat_member"]["new_chat_member"]["user"]["is_bot"] = json!(true);
        assert_eq!(parse(&value, 7), Ok(None));
        let event = value
            .as_object_mut()
            .unwrap()
            .remove("chat_member")
            .unwrap();
        value["my_chat_member"] = event;
        assert_eq!(parse(&value, 7), Err(InvalidMembershipUpdate));
        assert!(parse(&value, 123).unwrap().unwrap().bot_subject);
    }

    #[test]
    fn unrelated_updates_are_unchanged() {
        assert_eq!(parse(&json!({"update_id": 42, "message": {}}), 7), Ok(None));
    }
}
