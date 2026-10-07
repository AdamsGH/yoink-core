//! Source-derived Pydantic field/default contracts. Unknown fields are ignored,
//! scalar strings stay strict; Pydantic's JSON integer/bool coercions are explicit.
//! Persistence-invalid nulls are rejected before SQL rather than becoming a 500.
use crate::{Error, Result, catalog::Catalog};
use serde::Deserialize;
use serde_json::{Map, Value, json};
pub fn string(v: &Value, min: usize, max: usize) -> Result<String> {
    let s = v.as_str().ok_or(Error::Invalid)?;
    if !(min..=max).contains(&s.chars().count()) {
        return Err(Error::Invalid);
    }
    Ok(s.into())
}
pub fn integer(v: &Value) -> Result<i64> {
    if let Some(n) = v.as_i64() {
        return Ok(n);
    }
    if let Some(b) = v.as_bool() {
        return Ok(i64::from(b));
    }
    if let Some(s) = v.as_str() {
        let s = s.trim();
        let chars: Vec<_> = s.chars().collect();
        for (i, c) in chars.iter().enumerate() {
            if *c == '_'
                && (i == 0
                    || i + 1 == chars.len()
                    || !chars[i - 1].is_ascii_digit()
                    || !chars[i + 1].is_ascii_digit())
            {
                return Err(Error::Invalid);
            }
        }
        let normalized = s.replace('_', "");
        let s = normalized.as_str();
        if let Ok(n) = s.parse() {
            return Ok(n);
        }
        // Pydantic accepts decimal-zero integer strings, but not exponents.
        let Some((whole, fraction)) = s.split_once('.') else {
            return Err(Error::Invalid);
        };
        if fraction.is_empty() || !fraction.chars().all(|c| c == '0') {
            return Err(Error::Invalid);
        }
        return whole.parse().map_err(|_| Error::Invalid);
    }
    if let Some(n) = v.as_f64()
        && n.is_finite()
        && n.fract() == 0.0
        && n >= i64::MIN as f64
        && n < (i64::MAX as f64)
    {
        return Ok(n as i64);
    }
    Err(Error::Invalid)
}
pub fn boolean(v: &Value) -> Result<bool> {
    if let Some(b) = v.as_bool() {
        return Ok(b);
    }
    if v.as_f64() == Some(0.0) {
        return Ok(false);
    }
    if v.as_f64() == Some(1.0) {
        return Ok(true);
    }
    if let Some(s) = v.as_str() {
        return match s.to_ascii_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" => Ok(false),
            "1" | "on" | "t" | "true" | "y" | "yes" => Ok(true),
            _ => Err(Error::Invalid),
        };
    }
    Err(Error::Invalid)
}
fn field(v: &Value, k: &str, out: &mut Map<String, Value>, ty: &str, nullable: bool) -> Result<()> {
    let Some(n) = v.get(k) else { return Ok(()) };
    let normalized = if n.is_null() && nullable {
        Value::Null
    } else {
        match ty {
            "name" => json!(string(n, 1, 128)?),
            "slug" => json!(
                string(n, 0, usize::MAX)?
                    .chars()
                    .take(64)
                    .collect::<String>()
            ),
            "string" => json!(string(
                n,
                0,
                match k {
                    "icon" => 32,
                    "color" => 16,
                    _ => usize::MAX,
                }
            )?),
            "int" => {
                let n = integer(n)?;
                if k == "sort_order" && i32::try_from(n).is_err() {
                    return Err(Error::Invalid);
                }
                json!(n)
            }
            "bool" => json!(boolean(n)?),
            "priority" => {
                let n = integer(n)?;
                if !(0..=9999).contains(&n) {
                    return Err(Error::Invalid);
                }
                json!(n)
            }
            "dicts" => {
                let a = n
                    .as_array()
                    .filter(|a| a.iter().all(Value::is_object))
                    .ok_or(Error::Invalid)?;
                json!(a)
            }
            _ => return Err(Error::Invalid),
        }
    };
    out.insert(k.into(), normalized);
    Ok(())
}
pub fn catalogue(v: &Value, kind: Catalog, full: bool) -> Result<Value> {
    if !v.is_object() {
        return Err(Error::Invalid);
    }
    let mut out = Map::new();
    field(v, "name", &mut out, "name", !full)?;
    if full && !out.contains_key("name") {
        return Err(Error::Invalid);
    }
    match kind {
        Catalog::Category | Catalog::Folder => {
            for (k, t) in [
                ("slug", "slug"),
                ("description", "string"),
                ("icon", "string"),
                ("parent_id", "int"),
            ] {
                field(v, k, &mut out, t, true)?;
                if full {
                    out.entry(k).or_insert(Value::Null);
                }
            }
            if kind == Catalog::Category {
                for (k, t) in [("color", "string"), ("shared_with_team_id", "int")] {
                    field(v, k, &mut out, t, true)?;
                    if full {
                        out.entry(k).or_insert(Value::Null);
                    }
                }
            } else {
                field(v, "is_local", &mut out, "bool", false)?;
                if !full {
                    field(v, "is_pinned", &mut out, "bool", false)?;
                    field(v, "sort_order", &mut out, "int", false)?;
                }
            }
        }
        Catalog::Rule => {
            field(v, "enabled", &mut out, "bool", false)?;
            field(v, "priority", &mut out, "priority", false)?;
            field(v, "trigger", &mut out, "string", false)?;
            for k in ["conditions", "actions"] {
                field(v, k, &mut out, "dicts", true)?;
            }
            if full {
                out.entry("enabled").or_insert(json!(true));
                out.entry("priority").or_insert(json!(100));
                out.entry("conditions").or_insert(Value::Null);
                out.entry("actions").or_insert(Value::Null);
                if !out.contains_key("trigger") {
                    return Err(Error::Invalid);
                }
            }
            if out
                .get("trigger")
                .is_some_and(|t| t.as_str().is_none_or(|s| s.chars().count() > 24))
            {
                return Err(Error::Invalid);
            }
        }
    }
    if v.get("slug").is_some_and(|s| s.is_null() || s == "")
        && out.get("name").is_some_and(Value::is_string)
    {
        out.insert(
            "slug".into(),
            json!(crate::normalize::slug(out["name"].as_str().unwrap())),
        );
    }
    // Null names on source RuleUpdate eventually violate NOT NULL. Normalize to
    // a stable validation rejection, never silently replace/default the value.
    if out.get("name").is_some_and(Value::is_null) {
        return Err(Error::Invalid);
    }
    Ok(Value::Object(out))
}
/// Folder PATCH ignores explicit nulls, accepts only these four fields, and
/// leaves slug/parent/icon untouched (unlike PUT's create-shaped replacement).
pub fn folder_patch(v: &Value) -> Result<Value> {
    if !v.is_object() {
        return Err(Error::Invalid);
    }
    let mut out = Map::new();
    for (k, t) in [
        ("name", "name"),
        ("description", "string"),
        ("is_pinned", "bool"),
        ("sort_order", "int"),
    ] {
        field(v, k, &mut out, t, true)?;
    }
    out.retain(|_, v| !v.is_null());
    Ok(Value::Object(out))
}
pub fn team(v: &Value) -> Result<Value> {
    if !v.is_object() {
        return Err(Error::Invalid);
    }
    let mut out = Map::new();
    field(v, "name", &mut out, "name", false)?;
    if !out.contains_key("name") {
        return Err(Error::Invalid);
    }
    for (k, t) in [("slug", "slug"), ("description", "string")] {
        field(v, k, &mut out, t, true)?;
        out.entry(k).or_insert(Value::Null);
    }
    if v.get("slug").is_some_and(|s| s.is_null() || s == "") {
        out.insert(
            "slug".into(),
            json!(crate::normalize::slug(out["name"].as_str().unwrap())),
        );
    }
    Ok(Value::Object(out))
}
pub fn bounded_optional(s: Option<&str>, max: usize) -> Result<()> {
    if s.is_some_and(|s| s.chars().count() > max) {
        Err(Error::Invalid)
    } else {
        Ok(())
    }
}

/// Pydantic JSON int coercion for authenticated HTTP payloads (not identities).
pub fn deserialize_integer<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<i64, D::Error> {
    let v = Value::deserialize(d)?;
    integer(&v).map_err(serde::de::Error::custom)
}
pub fn deserialize_integers<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<i64>, D::Error> {
    let v = Vec::<Value>::deserialize(d)?;
    v.iter()
        .map(|v| integer(v).map_err(serde::de::Error::custom))
        .collect()
}
