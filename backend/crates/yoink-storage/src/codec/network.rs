use serde::{Deserialize, Serialize};
use yoink_domain::{
    CookieReference, CredentialVersion, NetworkSelection, ProxyReference, ProxySource,
};

use crate::StoreError;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NetworkSelectionData {
    #[serde(default)]
    cookie: Option<CookieData>,
    #[serde(default)]
    proxy: Option<ProxyData>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CookieData {
    id: i32,
    owner_id: u64,
    domain: String,
    is_pool: bool,
    version: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProxyData {
    source: ProxySourceData,
    version: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ProxySourceData {
    Personal { actor_id: u64 },
    System,
}

impl From<&NetworkSelection> for NetworkSelectionData {
    fn from(selection: &NetworkSelection) -> Self {
        Self {
            cookie: selection.cookie().map(|cookie| CookieData {
                id: cookie.id(),
                owner_id: cookie.owner_id(),
                domain: cookie.domain().to_owned(),
                is_pool: cookie.is_pool(),
                version: cookie.version().as_str().to_owned(),
            }),
            proxy: selection.proxy().map(|proxy| ProxyData {
                source: match proxy.source() {
                    ProxySource::Personal { actor_id } => ProxySourceData::Personal { actor_id },
                    ProxySource::System => ProxySourceData::System,
                },
                version: proxy.version().as_str().to_owned(),
            }),
        }
    }
}

impl TryFrom<NetworkSelectionData> for NetworkSelection {
    type Error = StoreError;

    fn try_from(data: NetworkSelectionData) -> Result<Self, Self::Error> {
        let cookie = data
            .cookie
            .map(|cookie| {
                let version = CredentialVersion::new(cookie.version)
                    .map_err(|_| StoreError::InvalidStoredData)?;
                CookieReference::new(
                    cookie.id,
                    cookie.owner_id,
                    cookie.domain,
                    cookie.is_pool,
                    version,
                )
                .map_err(|_| StoreError::InvalidStoredData)
            })
            .transpose()?;
        let proxy = data
            .proxy
            .map(|proxy| {
                let source = match proxy.source {
                    ProxySourceData::Personal { actor_id } => ProxySource::Personal { actor_id },
                    ProxySourceData::System => ProxySource::System,
                };
                let version = CredentialVersion::new(proxy.version)
                    .map_err(|_| StoreError::InvalidStoredData)?;
                ProxyReference::new(source, version).map_err(|_| StoreError::InvalidStoredData)
            })
            .transpose()?;
        Ok(Self::new(cookie, proxy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{decode, encode};
    use yoink_domain::{DownloadParams, DownloadRequest, MediaMode};

    fn request() -> DownloadRequest {
        DownloadRequest::new(
            42,
            "https://example.com/video",
            MediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .unwrap()
    }
    fn selection(pool: bool, source: ProxySource) -> NetworkSelection {
        NetworkSelection::new(
            Some(
                CookieReference::new(
                    7,
                    if pool { 99 } else { 42 },
                    "example.com",
                    pool,
                    CredentialVersion::new("a".repeat(64)).unwrap(),
                )
                .unwrap(),
            ),
            Some(
                ProxyReference::new(source, CredentialVersion::new("b".repeat(64)).unwrap())
                    .unwrap(),
            ),
        )
    }
    #[test]
    fn network_references_round_trip_without_secret_fields() {
        for (pool, source) in [
            (false, ProxySource::Personal { actor_id: 42 }),
            (true, ProxySource::System),
        ] {
            let original = request().with_network(selection(pool, source)).unwrap();
            let value = encode(&original).unwrap();
            assert_eq!(decode(value.clone()).unwrap(), original);
            let text = value.to_string();
            for forbidden in ["cookie_jar", "proxy_url", "content", "password"] {
                assert!(!text.contains(forbidden));
            }
        }
    }
    #[test]
    fn legacy_network_is_none_and_malformed_or_foreign_references_fail_closed() {
        let mut old = encode(&request()).unwrap();
        old.as_object_mut().unwrap().remove("network");
        assert_eq!(decode(old).unwrap().network(), &NetworkSelection::NONE);
        let original = encode(
            &request()
                .with_network(selection(false, ProxySource::Personal { actor_id: 42 }))
                .unwrap(),
        )
        .unwrap();
        for path in [
            vec!["network", "cookie", "owner_id"],
            vec!["network", "proxy", "source", "actor_id"],
        ] {
            let mut bad = original.clone();
            let mut value = &mut bad;
            for key in path {
                value = &mut value[key];
            }
            *value = serde_json::json!(99);
            assert_eq!(decode(bad), Err(StoreError::InvalidStoredData));
        }
        let mut bad = original.clone();
        bad["network"]["cookie"]["version"] = serde_json::json!("raw-cookie-secret");
        assert_eq!(decode(bad), Err(StoreError::InvalidStoredData));
        let mut bad = original.clone();
        bad["network"]["proxy"]["proxy_url"] = serde_json::json!("http://private.example");
        assert_eq!(decode(bad), Err(StoreError::InvalidStoredData));
        let mut bad = original;
        bad["network"]["cookie"]["domain"] = serde_json::json!(".foreign.example");
        assert_eq!(decode(bad), Err(StoreError::InvalidStoredData));
    }
}
