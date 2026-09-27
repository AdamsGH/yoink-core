use std::fmt;

use thiserror::Error;

/// Validated lowercase SHA-256 version token. Never exposes its value in Debug.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct CredentialVersion(String);

impl CredentialVersion {
    pub fn new(value: impl Into<String>) -> Result<Self, NetworkReferenceError> {
        let value = value.into();
        if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(NetworkReferenceError::InvalidVersion);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CredentialVersion([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct CookieReference {
    id: i32,
    owner_id: u64,
    domain: String,
    is_pool: bool,
    version: CredentialVersion,
}

impl CookieReference {
    pub fn new(
        id: i32,
        owner_id: u64,
        domain: impl Into<String>,
        is_pool: bool,
        version: CredentialVersion,
    ) -> Result<Self, NetworkReferenceError> {
        let domain = domain.into();
        if id <= 0 || owner_id == 0 || owner_id > i64::MAX as u64 || !valid_domain(&domain) {
            return Err(NetworkReferenceError::InvalidCookie);
        }
        Ok(Self {
            id,
            owner_id,
            domain,
            is_pool,
            version,
        })
    }
    pub const fn id(&self) -> i32 {
        self.id
    }
    pub const fn owner_id(&self) -> u64 {
        self.owner_id
    }
    pub fn domain(&self) -> &str {
        &self.domain
    }
    pub const fn is_pool(&self) -> bool {
        self.is_pool
    }
    pub const fn version(&self) -> &CredentialVersion {
        &self.version
    }
}
impl fmt::Debug for CookieReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CookieReference")
            .field("id", &self.id)
            .field("owner_id", &self.owner_id)
            .field("domain", &self.domain)
            .field("is_pool", &self.is_pool)
            .field("version", &self.version)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProxySource {
    Personal { actor_id: u64 },
    System,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProxyReference {
    source: ProxySource,
    version: CredentialVersion,
}
impl ProxyReference {
    pub fn new(
        source: ProxySource,
        version: CredentialVersion,
    ) -> Result<Self, NetworkReferenceError> {
        if matches!(source, ProxySource::Personal { actor_id } if actor_id == 0 || actor_id > i64::MAX as u64)
        {
            return Err(NetworkReferenceError::InvalidProxy);
        }
        Ok(Self { source, version })
    }
    pub const fn source(&self) -> ProxySource {
        self.source
    }
    pub const fn version(&self) -> &CredentialVersion {
        &self.version
    }
}
impl fmt::Debug for ProxyReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyReference")
            .field("source", &self.source)
            .field("version", &self.version)
            .finish()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkSelection {
    cookie: Option<CookieReference>,
    proxy: Option<ProxyReference>,
}
impl NetworkSelection {
    pub const NONE: Self = Self {
        cookie: None,
        proxy: None,
    };
    pub const fn new(cookie: Option<CookieReference>, proxy: Option<ProxyReference>) -> Self {
        Self { cookie, proxy }
    }
    pub const fn cookie(&self) -> Option<&CookieReference> {
        self.cookie.as_ref()
    }
    pub const fn proxy(&self) -> Option<&ProxyReference> {
        self.proxy.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum NetworkReferenceError {
    #[error("credential version must be 64 hexadecimal SHA-256 characters")]
    InvalidVersion,
    #[error("cookie reference is invalid")]
    InvalidCookie,
    #[error("proxy reference is invalid")]
    InvalidProxy,
    #[error("personal network reference belongs to a different actor")]
    ActorMismatch,
}

impl NetworkSelection {
    pub fn validate_actor(&self, actor_id: u64) -> Result<(), NetworkReferenceError> {
        if self
            .cookie
            .as_ref()
            .is_some_and(|c| !c.is_pool && c.owner_id != actor_id)
            || matches!(self.proxy.as_ref().map(|p| p.source), Some(ProxySource::Personal { actor_id: owner }) if owner != actor_id)
        {
            return Err(NetworkReferenceError::ActorMismatch);
        }
        Ok(())
    }
}

fn valid_domain(value: &str) -> bool {
    value.len() <= 253
        && !value.is_empty()
        && !value.starts_with('.')
        && !value.ends_with('.')
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DownloadParams, DownloadRequest, MediaMode};

    fn version(value: &str) -> CredentialVersion {
        CredentialVersion::new(value).unwrap()
    }

    #[test]
    fn validates_references_and_redacts_versions() {
        assert!(CredentialVersion::new("bad").is_err());
        assert!(CredentialVersion::new("g".repeat(64)).is_err());
        assert!(
            CookieReference::new(0, 42, "example.com", false, version(&"a".repeat(64))).is_err()
        );
        assert!(
            CookieReference::new(1, 42, "Example.com", false, version(&"a".repeat(64))).is_err()
        );
        let secret = "a".repeat(64);
        let reference = ProxyReference::new(ProxySource::System, version(&secret)).unwrap();
        assert!(!format!("{reference:?}").contains(&secret));
        assert!(!format!("{:?}", version(&secret)).contains(&secret));
    }

    #[test]
    fn personal_references_must_belong_to_request_actor() {
        let request = DownloadRequest::new(
            42,
            "https://example.test",
            MediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .unwrap();
        let cookie =
            CookieReference::new(7, 99, "example.test", false, version(&"b".repeat(64))).unwrap();
        assert_eq!(
            request
                .clone()
                .with_network(NetworkSelection::new(Some(cookie), None)),
            Err(crate::RequestValidationError::NetworkActorMismatch)
        );
        let proxy = ProxyReference::new(
            ProxySource::Personal { actor_id: 99 },
            version(&"c".repeat(64)),
        )
        .unwrap();
        assert_eq!(
            request.with_network(NetworkSelection::new(None, Some(proxy))),
            Err(crate::RequestValidationError::NetworkActorMismatch)
        );
    }

    #[test]
    fn pool_references_can_be_owned_by_another_actor() {
        let request = DownloadRequest::new(
            42,
            "https://example.test",
            MediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .unwrap();
        let pool =
            CookieReference::new(7, 99, "example.test", true, version(&"d".repeat(64))).unwrap();
        assert!(
            request
                .with_network(NetworkSelection::new(Some(pool), None))
                .is_ok()
        );
    }
}
