use crate::Error;
use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
use std::fmt;

pub(super) fn messages(bytes: &[u8], max: usize) -> Result<Vec<Value>, Error> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let messages = Export(max)
        .deserialize(&mut decoder)
        .map_err(|_| Error::Invalid)?;
    decoder.end().map_err(|_| Error::Invalid)?;
    Ok(messages)
}

#[derive(Deserialize)]
#[serde(field_identifier)]
enum Field {
    #[serde(rename = "messages")]
    Messages,
    #[serde(other)]
    Other,
}

struct Export(usize);
impl<'de> DeserializeSeed<'de> for Export {
    type Value = Vec<Value>;
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for Export {
    type Value = Vec<Value>;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a Desktop export object")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut messages = None;
        while let Some(field) = map.next_key::<Field>()? {
            match field {
                Field::Messages => {
                    if messages.is_some() {
                        return Err(de::Error::duplicate_field("messages"));
                    }
                    messages = Some(map.next_value_seed(Messages(self.0))?);
                }
                Field::Other => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        messages.ok_or_else(|| de::Error::missing_field("messages"))
    }
}

struct Messages(usize);
impl<'de> DeserializeSeed<'de> for Messages {
    type Value = Vec<Value>;
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for Messages {
    type Value = Vec<Value>;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a bounded message array")
    }
    fn visit_seq<S: SeqAccess<'de>>(self, mut sequence: S) -> Result<Self::Value, S::Error> {
        let mut messages = Vec::new();
        while messages.len() < self.0 {
            let Some(message) = sequence.next_element::<Value>()? else {
                return Ok(messages);
            };
            messages.push(message);
        }
        if sequence.next_element::<IgnoredAny>()?.is_some() {
            return Err(de::Error::custom("message limit exceeded"));
        }
        Ok(messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_fields_are_ignored_and_message_retention_is_bounded() {
        let bytes = br#"{"name":{"unused":[1,2,3]},"messages":[{"id":1}],"unused":[{"x":"y"}]}"#;
        assert_eq!(
            messages(bytes, 1).unwrap(),
            vec![serde_json::json!({"id":1})]
        );
        for bytes in [
            br#"{"messages":[{},{}]}"#.as_slice(),
            br#"{"messages":[],"messages":[]}"#,
            br#"{"messages":[]}{}"#,
            br#"{"messages":{}}"#,
            br#"{}"#,
        ] {
            assert!(matches!(messages(bytes, 1), Err(Error::Invalid)));
        }
    }
}
