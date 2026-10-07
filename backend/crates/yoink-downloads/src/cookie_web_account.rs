//! Source-backed Netscape account hints and pool deduplication keys.
use std::collections::BTreeMap;
pub(super) fn values(content: &str) -> BTreeMap<&str, &str> {
    content
        .lines()
        .filter(|l| l.starts_with("#HttpOnly_") || !l.starts_with('#'))
        .filter_map(|l| {
            let f = l.split('\t').collect::<Vec<_>>();
            (f.len() == 7).then(|| (f[5], f[6]))
        })
        .collect()
}
fn site(domain: &str, base: &str) -> bool {
    domain == base || domain.ends_with(&format!(".{base}"))
}
pub(super) fn label(domain: &str, content: &str) -> Option<String> {
    let v = values(content);
    let get = |key| v.get(key).copied();
    let authenticated = |keys: &[&str]| {
        keys.iter()
            .any(|k| v.contains_key(k))
            .then(|| "authenticated".to_owned())
    };
    if site(domain, "youtube.com") || site(domain, "google.com") {
        if let Some(h) = get("HSID") {
            let mut result = format!("HSID:{}", h.chars().take(7).collect::<String>());
            if let Some(tz) =
                get("PREF").and_then(|p| p.split('&').find_map(|v| v.strip_prefix("tz=")))
            {
                result.push_str(&format!(" ({})", tz.replace('.', "/")));
            }
            return Some(result.chars().take(128).collect());
        }
        return authenticated(&["SAPISID", "__Secure-1PSID", "SID"]);
    }
    let (identity, hints) = if site(domain, "instagram.com") {
        ("ds_user_id", vec!["sessionid"])
    } else if site(domain, "twitter.com") || site(domain, "x.com") {
        ("twid", vec!["auth_token"])
    } else if site(domain, "tiktok.com") {
        ("uid_tt", vec!["sid_tt"])
    } else if site(domain, "facebook.com") {
        ("c_user", vec!["xs"])
    } else if site(domain, "reddit.com") {
        return authenticated(&["token_v2", "reddit_session"]);
    } else {
        return v
            .keys()
            .any(|k| {
                [
                    "sessionid",
                    "session",
                    "auth_token",
                    "access_token",
                    "token",
                    "sid",
                ]
                .contains(&k.to_ascii_lowercase().as_str())
            })
            .then(|| "authenticated".into());
    };
    if let Some(uid) = get(identity) {
        let uid = uid.replace("u%3D", "").replace("u=", "");
        return Some(format!(
            "uid:{}",
            uid.chars()
                .take(if identity == "uid_tt" { 16 } else { 124 })
                .collect::<String>()
        ));
    }
    authenticated(&hints)
}
pub(super) fn raw_session(domain: &str, content: &str) -> Option<String> {
    let v = values(content);
    let keys: &[&str] = if site(domain, "youtube.com") || site(domain, "google.com") {
        &["SAPISID", "__Secure-3PAPISID"]
    } else if site(domain, "instagram.com") {
        &["ds_user_id", "sessionid"]
    } else if site(domain, "twitter.com") || site(domain, "x.com") {
        &["twid", "auth_token"]
    } else if site(domain, "tiktok.com") {
        &["uid_tt", "sid_tt"]
    } else if site(domain, "facebook.com") {
        &["c_user", "xs"]
    } else {
        &[
            "sessionid",
            "session_id",
            "auth_token",
            "access_token",
            "sid",
        ]
    };
    keys.iter().find_map(|k| v.get(k)).map(|v| (*v).to_owned())
}
pub(super) fn session(domain: &str, content: &str) -> Option<String> {
    raw_session(domain, content)
        .map(|v| format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(v.as_bytes())))
}
