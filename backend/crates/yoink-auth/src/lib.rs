use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::Serialize;
use serde_json::Value;
use sha2::Sha256;
use std::fmt;
use url::Url;

type HmacSha256 = Hmac<Sha256>;
const MAX_INIT_DATA: usize = 16 * 1024;
const MAX_FIELDS: usize = 64;
const MAX_TOKEN: usize = 8192;

#[derive(Clone)]
pub struct AuthConfig {
    bot_token: String,
    jwt_secret: String,
    max_age_secs: u64,
    future_skew_secs: u64,
    token_ttl_secs: u64,
}

impl fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthConfig")
            .field("bot_token", &"[REDACTED]")
            .field("jwt_secret", &"[REDACTED]")
            .field("max_age_secs", &self.max_age_secs)
            .field("future_skew_secs", &self.future_skew_secs)
            .field("token_ttl_secs", &self.token_ttl_secs)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("authentication failed")]
pub struct AuthError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustedProfile {
    pub user_id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub photo_url: Option<String>,
}

pub struct JwtProfile<'a> {
    pub user_id: i64,
    pub role: &'a str,
    pub username: Option<&'a str>,
    pub first_name: Option<&'a str>,
}

impl AuthConfig {
    pub fn new(
        bot_token: String,
        jwt_secret: String,
        max_age_secs: u64,
        future_skew_secs: u64,
        token_ttl_secs: u64,
    ) -> Result<Self, AuthError> {
        if bot_token.is_empty()
            || bot_token.chars().any(char::is_whitespace)
            || jwt_secret.trim().is_empty()
            || !(1..=86_400).contains(&max_age_secs)
            || future_skew_secs > 300
            || !(60..=604_800).contains(&token_ttl_secs)
        {
            return Err(AuthError);
        }
        Ok(Self {
            bot_token,
            jwt_secret,
            max_age_secs,
            future_skew_secs,
            token_ttl_secs,
        })
    }

    pub fn verify_webapp(
        &self,
        init_data: &str,
        now_secs: i64,
    ) -> Result<TrustedProfile, AuthError> {
        if init_data.is_empty()
            || init_data.len() > MAX_INIT_DATA
            || init_data.chars().any(char::is_control)
        {
            return Err(AuthError);
        }
        let fields = parse_form(init_data)?;
        let signature = fields.get("hash").ok_or(AuthError)?;
        if signature.len() != 64 || !signature.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(AuthError);
        }
        let mut canonical: Vec<_> = fields
            .iter()
            .filter(|(key, _)| key.as_str() != "hash")
            .collect();
        canonical.sort_by_key(|(key, _)| *key);
        let data_check = canonical
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut derivation = HmacSha256::new_from_slice(b"WebAppData").map_err(|_| AuthError)?;
        derivation.update(self.bot_token.as_bytes());
        let secret = derivation.finalize().into_bytes();
        let mut verifier = HmacSha256::new_from_slice(&secret).map_err(|_| AuthError)?;
        verifier.update(data_check.as_bytes());
        let decoded_signature = decode_hex(signature)?;
        verifier
            .verify_slice(&decoded_signature)
            .map_err(|_| AuthError)?;

        let auth_date_text = fields.get("auth_date").ok_or(AuthError)?;
        if auth_date_text.is_empty() || !auth_date_text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AuthError);
        }
        let auth_date: i64 = auth_date_text.parse().map_err(|_| AuthError)?;
        if auth_date <= 0 {
            return Err(AuthError);
        }
        let now = i128::from(now_secs);
        let timestamp = i128::from(auth_date);
        if timestamp > now + i128::from(self.future_skew_secs)
            || now - timestamp > i128::from(self.max_age_secs)
        {
            return Err(AuthError);
        }

        let user_text = fields.get("user").ok_or(AuthError)?;
        let user: Value = serde_json::from_str(user_text).map_err(|_| AuthError)?;
        if json_depth(&user, 0) > 8 {
            return Err(AuthError);
        }
        let object = user.as_object().ok_or(AuthError)?;
        let id = object
            .get("id")
            .and_then(Value::as_i64)
            .filter(|id| *id > 0)
            .ok_or(AuthError)?;
        let username = optional_string(object, "username", 64)?;
        let first_name = optional_string(object, "first_name", 256)?;
        let photo_url = optional_string(object, "photo_url", 2048)?;
        if let Some(photo) = &photo_url {
            let parsed = Url::parse(photo).map_err(|_| AuthError)?;
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
            {
                return Err(AuthError);
            }
        }
        Ok(TrustedProfile {
            user_id: id,
            username,
            first_name,
            photo_url,
        })
    }

    pub fn issue_token(&self, profile: JwtProfile<'_>, now_secs: i64) -> Result<String, AuthError> {
        if profile.user_id <= 0
            || profile.role.is_empty()
            || profile.role.len() > 128
            || profile.role.chars().any(char::is_control)
            || !valid_claim_string(profile.username, 64)
            || !valid_claim_string(profile.first_name, 256)
        {
            return Err(AuthError);
        }
        let exp = now_secs
            .checked_add(i64::try_from(self.token_ttl_secs).map_err(|_| AuthError)?)
            .ok_or(AuthError)?;
        let claims = TokenClaims {
            sub: profile.user_id.to_string(),
            role: profile.role,
            exp,
            username: profile.username.filter(|s| !s.is_empty()),
            first_name: profile.first_name.filter(|s| !s.is_empty()),
        };
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).map_err(|_| AuthError)?);
        let signing_input = format!("{header}.{payload}");
        let mut mac =
            HmacSha256::new_from_slice(self.jwt_secret.as_bytes()).map_err(|_| AuthError)?;
        mac.update(signing_input.as_bytes());
        Ok(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        ))
    }

    pub fn verify_token(&self, token: &str, now_secs: i64) -> Result<i64, AuthError> {
        if token.is_empty() || token.len() > MAX_TOKEN {
            return Err(AuthError);
        }
        let mut pieces = token.split('.');
        let (head, payload, signature) = (
            pieces.next().ok_or(AuthError)?,
            pieces.next().ok_or(AuthError)?,
            pieces.next().ok_or(AuthError)?,
        );
        if pieces.next().is_some() || head.is_empty() || payload.is_empty() || signature.is_empty()
        {
            return Err(AuthError);
        }
        let header_bytes = strict_b64(head)?;
        let header: Value = serde_json::from_slice(&header_bytes).map_err(|_| AuthError)?;
        if header.as_object().is_none_or(|h| {
            h.len() != 2
                || h.get("alg").and_then(Value::as_str) != Some("HS256")
                || h.get("typ").and_then(Value::as_str) != Some("JWT")
        }) {
            return Err(AuthError);
        }
        let signature_bytes = strict_b64(signature)?;
        let mut mac =
            HmacSha256::new_from_slice(self.jwt_secret.as_bytes()).map_err(|_| AuthError)?;
        mac.update(format!("{head}.{payload}").as_bytes());
        mac.verify_slice(&signature_bytes).map_err(|_| AuthError)?;
        let payload_bytes = strict_b64(payload)?;
        let claims: Value = serde_json::from_slice(&payload_bytes).map_err(|_| AuthError)?;
        if json_depth(&claims, 0) > 8 {
            return Err(AuthError);
        }
        let claims = claims.as_object().ok_or(AuthError)?;
        let sub = claims.get("sub").and_then(Value::as_str).ok_or(AuthError)?;
        if sub.is_empty()
            || !sub.bytes().all(|b| b.is_ascii_digit())
            || (sub.len() > 1 && sub.starts_with('0'))
        {
            return Err(AuthError);
        }
        let user_id: i64 = sub.parse().map_err(|_| AuthError)?;
        if user_id <= 0 {
            return Err(AuthError);
        }
        let role = claims
            .get("role")
            .and_then(Value::as_str)
            .ok_or(AuthError)?;
        if role.is_empty() || role.len() > 128 || role.chars().any(char::is_control) {
            return Err(AuthError);
        }
        let exp = claims.get("exp").and_then(Value::as_i64).ok_or(AuthError)?;
        if exp <= now_secs {
            return Err(AuthError);
        }
        optional_string(claims, "username", 64)?;
        optional_string(claims, "first_name", 256)?;
        Ok(user_id)
    }
}

#[derive(Serialize)]
struct TokenClaims<'a> {
    sub: String,
    role: &'a str,
    exp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_name: Option<&'a str>,
}

fn parse_form(input: &str) -> Result<std::collections::BTreeMap<String, String>, AuthError> {
    let mut map = std::collections::BTreeMap::new();
    let mut count = 0;
    for pair in input.split('&') {
        if pair.is_empty() {
            return Err(AuthError);
        }
        count += 1;
        if count > MAX_FIELDS {
            return Err(AuthError);
        }
        let (key, value) = pair.split_once('=').ok_or(AuthError)?;
        let key = form_decode(key)?;
        let value = form_decode(value)?;
        if key.is_empty()
            || key.contains('=')
            || key.chars().any(char::is_control)
            || value.chars().any(char::is_control)
        {
            return Err(AuthError);
        }
        if map.insert(key, value).is_some() {
            return Err(AuthError);
        }
    }
    Ok(map)
}

fn form_decode(input: &str) -> Result<String, AuthError> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                decoded.push(b' ');
                i += 1;
            }
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err(AuthError);
                }
                let hi = hex_nibble(bytes[i + 1]).ok_or(AuthError)?;
                let lo = hex_nibble(bytes[i + 2]).ok_or(AuthError)?;
                decoded.push((hi << 4) | lo);
                i += 3;
            }
            byte => {
                decoded.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(decoded).map_err(|_| AuthError)
}
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
fn decode_hex(value: &str) -> Result<Vec<u8>, AuthError> {
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| Ok((hex_nibble(p[0]).ok_or(AuthError)? << 4) | hex_nibble(p[1]).ok_or(AuthError)?))
        .collect()
}
fn strict_b64(value: &str) -> Result<Vec<u8>, AuthError> {
    if value.contains('=') {
        return Err(AuthError);
    }
    let decoded = URL_SAFE_NO_PAD.decode(value).map_err(|_| AuthError)?;
    if URL_SAFE_NO_PAD.encode(&decoded) != value {
        return Err(AuthError);
    }
    Ok(decoded)
}
fn optional_string(
    map: &serde_json::Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<String>, AuthError> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.len() <= max && !s.chars().any(char::is_control) => {
            Ok(Some(s.clone()))
        }
        _ => Err(AuthError),
    }
}
fn valid_claim_string(value: Option<&str>, max: usize) -> bool {
    value.is_none_or(|s| s.len() <= max && !s.chars().any(char::is_control))
}
fn json_depth(value: &Value, depth: usize) -> usize {
    match value {
        Value::Array(a) => a
            .iter()
            .map(|v| json_depth(v, depth + 1))
            .max()
            .unwrap_or(depth + 1),
        Value::Object(o) => o
            .values()
            .map(|v| json_depth(v, depth + 1))
            .max()
            .unwrap_or(depth + 1),
        _ => depth,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AuthConfig {
        AuthConfig::new("123456:ABC".into(), "jwt-secret".into(), 3600, 30, 3600).unwrap()
    }
    fn signed(user: &str, auth_date: &str) -> String {
        let fields = format!("auth_date={auth_date}&user={}", form_encode(user));
        let canonical = format!("auth_date={auth_date}\nuser={user}");
        let mut derive = HmacSha256::new_from_slice(b"WebAppData").unwrap();
        derive.update(b"123456:ABC");
        let secret = derive.finalize().into_bytes();
        let mut mac = HmacSha256::new_from_slice(&secret).unwrap();
        mac.update(canonical.as_bytes());
        format!("{fields}&hash={}", hex(&mac.finalize().into_bytes()))
    }
    fn form_encode(s: &str) -> String {
        s.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"{}:,\"".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect()
    }
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
    fn signed_jwt(header: &[u8], payload: &[u8]) -> String {
        let head = URL_SAFE_NO_PAD.encode(header);
        let body = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{head}.{body}");
        let mut mac = HmacSha256::new_from_slice(b"jwt-secret").unwrap();
        mac.update(signing_input.as_bytes());
        format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        )
    }

    #[test]
    fn accepts_independent_known_hmac_vector() {
        let vector = "auth_date=1700000000&user=%7B%22id%22%3A42%2C%22first_name%22%3A%22A%22%7D&hash=b0aba00d6bac7098eba7b32ba2485a593cb80d1000cdf861610d8b440d9bf838";
        let result = config().verify_webapp(vector, 1_700_000_000).unwrap();
        assert_eq!(result.user_id, 42);
        assert_eq!(result.first_name.as_deref(), Some("A"));
    }
    #[test]
    fn form_decodes_once_and_preserves_unicode_plus_and_percent() {
        let user = r#"{"id":7,"username":"é +%252F"}"#;
        let data = signed(user, "1700000000");
        let profile = config().verify_webapp(&data, 1_700_000_000).unwrap();
        assert_eq!(profile.username.as_deref(), Some("é +%252F"));
        assert!(
            config()
                .verify_webapp("auth_date=%GG&user=x&hash=00", 1_700_000_000)
                .is_err()
        );
        assert!(
            config()
                .verify_webapp("auth_date=1&auth_date=1&user=x&hash=00", 1_700_000_000)
                .is_err()
        );
        assert!(
            config()
                .verify_webapp("auth_date=%FF&user=x&hash=00", 1_700_000_000)
                .is_err()
        );
    }
    #[test]
    fn enforces_required_age_and_boundaries() {
        let c = config();
        assert!(
            c.verify_webapp(&signed(r#"{"id":3}"#, "1700000000"), 1_700_003_600)
                .is_ok()
        );
        assert!(
            c.verify_webapp(&signed(r#"{"id":3}"#, "1700000000"), 1_700_003_601)
                .is_err()
        );
        assert!(
            c.verify_webapp(&signed(r#"{"id":3}"#, "1700000030"), 1_700_000_000)
                .is_ok()
        );
        assert!(
            c.verify_webapp(&signed(r#"{"id":3}"#, "1700000031"), 1_700_000_000)
                .is_err()
        );
        assert!(
            c.verify_webapp(&signed(r#"{"id":3}"#, "1700000000"), 1_700_000_000)
                .is_ok()
        );
        assert!(
            c.verify_webapp("user=%7B%22id%22%3A3%7D&hash=00", 1_700_000_000)
                .is_err()
        );
    }
    #[test]
    fn rejects_bad_profile_types_and_photo_urls() {
        let c = config();
        for user in [
            r#"{"id":true}"#,
            r#"{"id":3.0}"#,
            r#"{"id":"3"}"#,
            r#"{"id":0}"#,
            r#"{"id":3,"username":2}"#,
            r#"{"id":3,"photo_url":"https://u:p@example.com/x"}"#,
        ] {
            assert!(
                c.verify_webapp(&signed(user, "1700000000"), 1_700_000_000)
                    .is_err()
            );
        }
    }
    #[test]
    fn jwt_verification_is_strict_and_errors_are_generic() {
        let c = config();
        let token = c
            .issue_token(
                JwtProfile {
                    user_id: 42,
                    role: "user",
                    username: Some("alice"),
                    first_name: Some("Alice"),
                },
                1000,
            )
            .unwrap();
        assert_eq!(c.verify_token(&token, 1001).unwrap(), 42);
        assert!(c.verify_token(&token, 4600).is_err());
        let parts: Vec<_> = token.split('.').collect();
        let bad_payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"-1","role":"user","exp":5000}"#);
        assert!(
            c.verify_token(&format!("{}.{}.{}", parts[0], bad_payload, parts[2]), 1000)
                .is_err()
        );
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
        assert!(
            c.verify_token(&format!("{header}.{}.{}", parts[1], parts[2]), 1000)
                .is_err()
        );
        assert!(
            c.verify_token(
                &signed_jwt(
                    br#"{"alg":"HS256","typ":"JWT"}"#,
                    br#"{"sub":"9223372036854775808","role":"user","exp":5000}"#
                ),
                1000
            )
            .is_err()
        );
        assert!(
            c.verify_token(
                &signed_jwt(
                    br#"{"alg":"HS256","typ":"JWT"}"#,
                    br#"{"sub":"1","role":"user","exp":9223372036854775808}"#
                ),
                1000
            )
            .is_err()
        );
        assert!(
            c.verify_token(
                &signed_jwt(
                    br#"{"alg":"HS256","typ":"JWT"}"#,
                    br#"{"sub":"1","role":"user","exp":1000}"#
                ),
                1000
            )
            .is_err()
        );
        assert!(c.verify_token(&format!("{token}x"), 1000).is_err());
        assert_eq!(format!("{}", AuthError), "authentication failed");
    }
    #[test]
    fn validates_config_and_redacts_secrets() {
        assert!(AuthConfig::new(" ".into(), "secret-value".into(), 1, 0, 60).is_err());
        assert!(AuthConfig::new("bot-value".into(), " ".into(), 1, 0, 60).is_err());
        let debug = format!(
            "{:?}",
            AuthConfig::new("bot-value".into(), "secret-value".into(), 1, 0, 60).unwrap()
        );
        assert!(!debug.contains("bot-value") && !debug.contains("secret-value"));
        assert!(
            config()
                .issue_token(
                    JwtProfile {
                        user_id: 1,
                        role: "user",
                        username: None,
                        first_name: None
                    },
                    i64::MAX
                )
                .is_err()
        );
    }
}
