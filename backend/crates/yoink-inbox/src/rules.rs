use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Condition {
    pub field: String,
    pub op: String,
    pub value: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    #[serde(rename = "type")]
    pub kind: String,
    pub params: std::collections::BTreeMap<String, String>,
}
pub struct Facts<'a> {
    pub kind: &'a str,
    pub status: &'a str,
    pub url: &'a str,
    pub title: Option<&'a str>,
    pub categories: &'a [String],
    pub language: Option<&'a str>,
}
pub fn string_op(value: Option<&str>, op: &str, target: &str) -> bool {
    let Some(value) = value else {
        return matches!(op, "ne" | "not_contains");
    };
    let value = value.to_lowercase();
    let target = target.to_lowercase();
    match op {
        "eq" => value == target,
        "ne" => value != target,
        "contains" => value.contains(&target),
        "not_contains" => !value.contains(&target),
        _ => true,
    }
}
pub fn matches(c: &Condition, f: &Facts<'_>) -> bool {
    match c.field.as_str() {
        "kind" => string_op(Some(f.kind), &c.op, &c.value),
        "status" => string_op(Some(f.status), &c.op, &c.value),
        "url_contains" | "title_contains" => {
            let op = if c.op == "not_contains" {
                "not_contains"
            } else {
                "contains"
            };
            string_op(
                if c.field == "url_contains" {
                    Some(f.url)
                } else {
                    f.title
                },
                op,
                &c.value,
            )
        }
        "category_name" => {
            let hit = f
                .categories
                .iter()
                .any(|s| s.to_lowercase() == c.value.to_lowercase());
            match c.op.as_str() {
                "eq" => hit,
                "ne" => !hit,
                _ => true,
            }
        }
        "language" => f
            .language
            .is_none_or(|s| string_op(Some(s), &c.op, &c.value)),
        _ => true,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nullable() {
        assert!(string_op(None, "ne", "x"));
        assert!(!string_op(None, "contains", "x"));
    }
    #[test]
    fn casefold() {
        assert!(string_op(Some("Rust"), "eq", "rust"));
        assert!(!string_op(Some("Rust"), "not_contains", "us"));
    }
}
