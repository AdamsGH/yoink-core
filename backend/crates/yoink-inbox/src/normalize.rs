use crate::{Error, Result};
use url::Url;

pub fn normalize_url(raw: &str) -> Result<String> {
    let raw = raw.trim();
    let input = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("https://{raw}")
    };
    let mut url = Url::parse(&input).map_err(|_| Error::Invalid)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(Error::Invalid);
    }
    url.set_username("").map_err(|_| Error::Invalid)?;
    url.set_password(None).map_err(|_| Error::Invalid)?;
    url.set_fragment(None);
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, v)| !v.is_empty() && !tracking(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    pairs.sort();
    url.set_query(None);
    if !pairs.is_empty() {
        url.query_pairs_mut().extend_pairs(pairs);
    }
    if url.path().len() > 1 {
        let path = url.path().trim_end_matches('/').to_owned();
        url.set_path(&path);
    }
    // urllib preserves the distinction between an empty path and '/'.
    let value = url.to_string();
    let authority_end = input.find("://").unwrap() + 3;
    let original_path = input[authority_end..]
        .find(['/', '?', '#'])
        .map(|n| &input[authority_end + n..]);
    if original_path.is_none_or(|p| !p.starts_with('/')) {
        let end = value.find("://").unwrap() + 3;
        if let Some(pos) = value[end..].find('/') {
            let pos = end + pos;
            return Ok(format!("{}{}", &value[..pos], &value[pos + 1..]));
        }
    }
    Ok(value)
}
fn tracking(key: &str) -> bool {
    let key = key.to_lowercase();
    [
        "gclid", "fbclid", "yclid", "msclkid", "dclid", "igshid", "ref", "ref_src", "ref_url",
        "feature", "si", "spm",
    ]
    .contains(&key.as_str())
        || ["utm_", "mc_", "_hsenc", "_hsmi", "hsCtaTracking"]
            .iter()
            .any(|p| key.starts_with(p))
}
pub fn repo_ref(raw: &str) -> Option<(String, String)> {
    let u = Url::parse(raw).ok()?;
    if !["github.com", "www.github.com"].contains(&u.host_str()?) {
        return None;
    }
    let mut parts = u.path_segments()?.filter(|v| !v.is_empty());
    let owner = parts.next()?;
    let repo = parts.next()?.trim_end_matches(".git");
    if [
        "orgs",
        "apps",
        "marketplace",
        "settings",
        "topics",
        "users",
        "search",
    ]
    .contains(&owner)
        || repo.is_empty()
    {
        return None;
    }
    Some((owner.to_owned(), repo.to_owned()))
}
pub fn detect_kind(raw: &str) -> &'static str {
    let Ok(u) = Url::parse(raw) else {
        return "link";
    };
    if ["github.com", "www.github.com"].contains(&u.host_str().unwrap_or(""))
        && u.path_segments()
            .is_some_and(|p| p.filter(|s| !s.is_empty()).count() >= 2)
    {
        return "github_repo";
    }
    if [
        "youtube.com",
        "www.youtube.com",
        "youtu.be",
        "m.youtube.com",
        "vimeo.com",
        "www.vimeo.com",
    ]
    .contains(&u.host_str().unwrap_or(""))
    {
        "video"
    } else {
        "link"
    }
}
pub fn category_key(name: &str) -> String {
    name.chars()
        .filter(|c| ![' ', '_', '-'].contains(c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Router/schema _slugify: ASCII alphanumerics, runs of everything else -> '-'.
pub fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c)
        } else if !s.ends_with('-') {
            s.push('-')
        }
    }
    s.trim_matches('-').chars().take(64).collect()
}
/// Exact executable legacy bounded candidate sequence, including truncation
/// before the fallback '-x'. Only module constants may supply SQL identifiers.
pub(crate) async fn unique_slug<E: yoink_db::Executor>(
    c: &E,
    table: &str,
    owner: &str,
    user: i64,
    base: &str,
    exclude: Option<i64>,
) -> Result<String> {
    for suffix in std::iter::once(String::new()).chain((2..20).map(|i| format!("-{i}"))) {
        let candidate = format!("{base}{suffix}")
            .chars()
            .take(64)
            .collect::<String>();
        let sql = format!("SELECT id FROM {table} WHERE {owner}=? AND slug=? AND id<>?");
        if yoink_db::query(&sql)
            .bind(user)
            .bind(&candidate)
            .bind(exclude.unwrap_or(-1))
            .fetch_optional(c)
            .await?
            .is_none()
        {
            return Ok(candidate);
        }
    }
    let candidate = format!("{base}-x");
    if candidate.chars().count() > 64 {
        return Err(Error::Invalid);
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalization() {
        assert_eq!(
            normalize_url(" HTTPS://Example.COM:443/a///?b=2&utm_source=x&a=1#frag ").unwrap(),
            "https://example.com/a?a=1&b=2"
        );
        assert_eq!(normalize_url("example.com").unwrap(), "https://example.com");
        assert_eq!(
            normalize_url("https://example.com/").unwrap(),
            "https://example.com/"
        );
    }
    #[test]
    fn keys() {
        assert_eq!(category_key("Machine_Learn-ing"), "machinelearning");
    }
    #[test]
    fn github() {
        assert_eq!(
            repo_ref("https://github.com/A/B.git/tree/main"),
            Some(("A".into(), "B".into()))
        );
        assert!(repo_ref("https://github.com/settings/profile").is_none());
    }
    #[test]
    fn rejects_credentials_schemes() {
        assert!(normalize_url("file:///etc/passwd").is_err());
        assert_eq!(
            normalize_url("https://secret:token@example.com/a").unwrap(),
            "https://example.com/a"
        );
    }
}
