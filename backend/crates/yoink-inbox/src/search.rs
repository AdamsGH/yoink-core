//! PostgreSQL ILIKE's pattern grammar (% / _ / backslash escape) evaluated
//! before pagination in Rust: SQLite lower/LIKE is ASCII-only. Unicode lower
//! follows the source's non-Turkic language-independent contract; no FTS,
//! substring escaping, accent stripping or casefold expansion is introduced.
use crate::{Error, Result};
use regex::Regex;
/// PostgreSQL C.UTF-8 simple (one-codepoint), not contextual/full lowercase.
pub fn lower(text: &str) -> String {
    text.chars()
        .map(|c| c.to_lowercase().next().unwrap())
        .collect()
}
pub fn pattern(search: &str) -> Result<Regex> {
    let mut out = String::from("(?s)\\A");
    let mut escape = false;
    for c in lower(&format!("%{search}%")).chars() {
        if escape {
            out.push_str(&regex::escape(&c.to_string()));
            escape = false;
        } else {
            match c {
                '\\' => escape = true,
                '%' => out.push_str(".*"),
                '_' => out.push('.'),
                _ => out.push_str(&regex::escape(&c.to_string())),
            }
        }
    }
    if escape {
        return Err(Error::Invalid);
    }
    out.push_str("\\z");
    Regex::new(&out).map_err(|_| Error::Invalid)
}
pub fn filter(
    rows: Vec<yoink_db::Row>,
    search: Option<&str>,
    fields: &[&str],
) -> Result<Vec<yoink_db::Row>> {
    let Some(s) = search.filter(|s| !s.is_empty()) else {
        return Ok(rows);
    };
    let re = pattern(s)?;
    let mut matches = Vec::new();
    for r in rows {
        let mut matched = false;
        for field in fields {
            if let Some(s) = r.try_get::<Option<String>, _>(*field)?
                && re.is_match(&lower(&s))
            {
                matched = true;
                break;
            }
        }
        if matched {
            matches.push(r)
        }
    }
    Ok(matches)
}
pub fn validate(search: Option<&str>) -> Result<()> {
    if search.is_some_and(|s| !(2..=128).contains(&s.chars().count())) {
        Err(Error::Invalid)
    } else {
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn postgres_oracle_matrix() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ilike.json")).unwrap();
        for c in cases.as_array().unwrap() {
            let s = c[0].as_str().unwrap();
            let v = c[1].as_str().unwrap();
            assert_eq!(
                pattern(s).unwrap().is_match(&lower(v)),
                c[2].as_bool().unwrap(),
                "{s}: {v}"
            );
        }
    }
    #[test]
    fn unicode_and_patterns() {
        for (s, v, pass) in [
            ("прИв", "ПРИВЕТ", true),
            ("é_c", "Éxc", true),
            ("é_c", "Éxxc", false),
            (r"a\%b", "a%b", true),
            ("a%b", "a---b", true),
            (r"a\_b", "a_b", true),
            ("ss", "ß", false),
            ("σ", "Σ", true),
        ] {
            assert_eq!(pattern(s).unwrap().is_match(&lower(v)), pass, "{s}")
        }
    }
}
