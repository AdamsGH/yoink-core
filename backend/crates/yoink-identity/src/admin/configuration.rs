//! Core configuration boundary. Imported rows are preserved; only explicitly supplied
//! supported settings are written. Null retains the legacy PATCH no-op semantics.
use serde_json::Value;
use std::collections::BTreeMap;
use yoink_access::Role;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BotSettingsPatch(BTreeMap<String, String>);

impl BotSettingsPatch {
    pub fn parse(body: &Value) -> Result<Self, &'static str> {
        let object = body.as_object().ok_or("Expected a bot-settings object")?;
        let mut patch = BTreeMap::new();
        for (key, value) in object {
            let diagnostic = match key.as_str() {
                "bot_access_mode" => "bot_access_mode: expected open or approved_only",
                "browser_cookies_min_role" => "browser_cookies_min_role: expected a valid role",
                "inline_storage_chat_id" => {
                    "inline_storage_chat_id: expected a negative Telegram chat ID (integer or decimal string)"
                }
                "inline_storage_thread_id" => {
                    "inline_storage_thread_id: expected a positive thread ID (integer or decimal string)"
                }
                _ => {
                    return Err(
                        "Unknown bot setting; supported keys: bot_access_mode, browser_cookies_min_role, inline_storage_chat_id, inline_storage_thread_id. Use /bot-settings/tag-map for tag maps",
                    );
                }
            };
            if value.is_null() {
                continue;
            }
            let text = match key.as_str() {
                "bot_access_mode" => value
                    .as_str()
                    .filter(|v| matches!(*v, "open" | "approved_only"))
                    .ok_or(diagnostic)?
                    .to_owned(),
                "browser_cookies_min_role" => value
                    .as_str()
                    .filter(|v| v.parse::<Role>().is_ok())
                    .ok_or(diagnostic)?
                    .to_owned(),
                _ => {
                    let id = match value {
                        Value::String(text) if !text.is_empty() && text.trim() == text => {
                            text.parse::<i64>().ok()
                        }
                        Value::Number(number) => number.as_i64(),
                        _ => None,
                    }
                    .ok_or(diagnostic)?;
                    if (key == "inline_storage_chat_id" && id >= 0)
                        || (key == "inline_storage_thread_id" && id <= 0)
                    {
                        return Err(diagnostic);
                    }
                    id.to_string()
                }
            };
            patch.insert(key.clone(), text);
        }
        Ok(Self(patch))
    }

    pub fn values(&self) -> &BTreeMap<String, String> {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn frontend_decimal_strings_and_integer_clients_are_equivalent() {
        assert_eq!(
            BotSettingsPatch::parse(
                &json!({"inline_storage_chat_id":"-100123", "inline_storage_thread_id":"42"})
            ),
            BotSettingsPatch::parse(
                &json!({"inline_storage_chat_id":-100123, "inline_storage_thread_id":42})
            )
        );
    }
    #[test]
    fn null_is_noop_and_unknown_null_is_not_silently_accepted() {
        assert!(
            BotSettingsPatch::parse(&json!({"inline_storage_chat_id":null}))
                .unwrap()
                .values()
                .is_empty()
        );
        assert!(
            BotSettingsPatch::parse(&json!({"typo":null}))
                .unwrap_err()
                .contains("supported keys")
        );
    }
    #[test]
    fn incompatible_values_have_field_diagnostics() {
        for value in [
            json!(true),
            json!(1.2),
            json!(0),
            json!(1),
            json!(""),
            json!(" -42"),
            json!("9223372036854775808"),
            json!([]),
        ] {
            assert!(
                BotSettingsPatch::parse(&json!({"inline_storage_chat_id":value}))
                    .unwrap_err()
                    .contains("inline_storage_chat_id")
            );
        }
        assert!(
            BotSettingsPatch::parse(&json!({"bot_access_mode":"closed"}))
                .unwrap_err()
                .contains("approved_only")
        );
    }
}
