pub use crate::streaming::{DeltaSink, Phase, stream_contract};
use crate::{Error, FutureResult, invalid};
use serde_json::{Value, json};
use std::{fmt, time::Duration};
use tokio_util::sync::CancellationToken;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    OpenAi,
    Anthropic,
    Gemini,
}
#[derive(Clone, PartialEq, Eq)]
pub struct Route {
    pub shape: Shape,
    pub base: String,
    pub key: String,
    pub model: String,
    pub via: &'static str,
}
impl Route {
    pub fn is_byok(&self) -> bool {
        self.via == "byok" || self.via.starts_with("byok_")
    }
}
impl fmt::Debug for Route {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Route")
            .field("shape", &self.shape)
            .field("model", &self.model)
            .field("via", &self.via)
            .finish_non_exhaustive()
    }
}
pub trait Completion: Send + Sync {
    /// Real streaming or an explicit capability error; never simulated from complete().
    fn stream<'a>(
        &'a self,
        _routes: &'a [Route],
        _prompt: &'a str,
        _sink: &'a dyn DeltaSink,
        _cancel: CancellationToken,
    ) -> FutureResult<'a, String> {
        Box::pin(async { Err(Error::Code("streaming_unavailable")) })
    }

    fn complete<'a>(
        &'a self,
        routes: &'a [Route],
        prompt: &'a str,
        cancel: CancellationToken,
    ) -> FutureResult<'a, String>;
    fn models<'a>(
        &'a self,
        _route: &'a Route,
        _cancel: CancellationToken,
    ) -> FutureResult<'a, Vec<Model>> {
        Box::pin(async { Err(Error::Code("models_unavailable")) })
    }
}
pub fn byok(
    provider: &str,
    base: Option<&str>,
    key: String,
    model: String,
) -> Result<Route, Error> {
    let (shape, default) = match provider {
        "openai" => (Shape::OpenAi, "https://api.openai.com/v1"),
        "openrouter" => (Shape::OpenAi, "https://openrouter.ai/api/v1"),
        "perplexity" => (Shape::OpenAi, "https://api.perplexity.ai"),
        "anthropic" => (Shape::Anthropic, "https://api.anthropic.com/v1"),
        "gemini" => (
            Shape::OpenAi,
            "https://generativelanguage.googleapis.com/v1beta/openai",
        ),
        "custom_openai" => (Shape::OpenAi, ""),
        "custom_anthropic" => (Shape::Anthropic, ""),
        _ => return Err(invalid()),
    };
    let base = base.filter(|s| !s.is_empty()).unwrap_or(default);
    let base = if provider == "gemini" && base.trim_end_matches('/').ends_with("/v1beta") {
        format!("{}/openai", base.trim_end_matches('/'))
    } else {
        base.to_owned()
    };
    let url = url::Url::parse(&base).map_err(|_| invalid())?;
    // Custom BYOK must not become an SSRF proxy. DNS/IP confinement must also be
    // enforced by deployment egress; redirects and proxy inheritance are disabled.
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none_or(|h| {
            h == "localhost" || h.ends_with(".localhost") || h.parse::<std::net::IpAddr>().is_ok()
        })
        || base.len() > 512
        || model.is_empty()
        || model.len() > 128
        || key.is_empty()
        || key.len() > 8192
    {
        return Err(invalid());
    }
    Ok(Route {
        shape,
        base: base.trim_end_matches('/').into(),
        key,
        model,
        via: match provider {
            "openrouter" => "byok_openrouter",
            "perplexity" => "byok_perplexity",
            _ => "byok",
        },
    })
}
pub type ProviderContract = (String, Vec<(&'static str, String)>, Value);
pub fn contract(route: &Route, prompt: &str) -> Result<ProviderContract, Error> {
    let base = route.base.trim_end_matches('/');
    Ok(match route.shape {
        Shape::OpenAi => (
            format!("{base}/chat/completions"),
            vec![("authorization", format!("Bearer {}", route.key))],
            json!({"model":route.model,"messages":[{"role":"user","content":prompt}],"stream":false}),
        ),
        Shape::Anthropic => (
            if base.ends_with("/v1/messages") {
                base.into()
            } else if base.ends_with("/v1") || !route.is_byok() {
                format!("{base}/messages")
            } else {
                format!("{base}/v1/messages")
            },
            vec![
                ("x-api-key", route.key.clone()),
                ("anthropic-version", "2023-06-01".into()),
            ],
            json!({"model":route.model,"max_tokens":8192,"messages":[{"role":"user","content":prompt}],"stream":false}),
        ),
        Shape::Gemini => {
            if !route
                .model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            {
                return Err(invalid());
            }
            (
                format!("{base}/models/{}:generateContent", route.model),
                vec![("x-goog-api-key", route.key.clone())],
                json!({"contents":[{"role":"user","parts":[{"text":prompt}]}]}),
            )
        }
    })
}
pub fn decode(shape: Shape, v: &Value) -> Result<String, Error> {
    let text = match shape {
        Shape::OpenAi => v
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(str::to_owned),
        Shape::Anthropic => v.get("content").and_then(Value::as_array).map(|a| {
            a.iter()
                .filter(|v| v["type"] == "text")
                .filter_map(|v| v["text"].as_str())
                .collect::<String>()
        }),
        Shape::Gemini => v
            .pointer("/candidates/0/content/parts")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v["text"].as_str())
                    .collect::<String>()
            }),
    }
    .ok_or(Error::Code("parse_failed"))?;
    if text.trim().is_empty() {
        Err(Error::Code("empty_response"))
    } else {
        Ok(text)
    }
}
pub fn status(code: u16) -> Error {
    Error::Code(match code {
        401 | 403 => "auth_failed",
        402 => "insufficient_credits",
        404 => "model_not_found",
        429 => "provider_rate_limited",
        500..=599 => "provider_unavailable",
        _ => "llm_error",
    })
}
/// Errors from explicit, fixture-only additional DER trust anchors.
#[cfg(feature = "test-fixtures")]
#[derive(Debug, thiserror::Error)]
pub enum FixtureTrustError {
    #[error("provide 1..=8 additional DER certificates (received {0})")]
    RootCount(usize),
    #[error("DER certificate {index} must contain 1..=16384 bytes (received {bytes})")]
    RootSize { index: usize, bytes: usize },
    #[error(
        "certificate {index} is not a valid DER trust anchor; supply an X.509 certificate, not PEM or a private key"
    )]
    InvalidRoot { index: usize },
    #[error("invalid completion budgets: timeout must be 1ns..=300s and max_bytes 1024..=4194304")]
    InvalidBudgets,
    #[error("could not build fixture TLS client: {0}")]
    Client(#[source] reqwest::Error),
}

pub struct HttpCompletion {
    pub(crate) client: reqwest::Client,
    pub(crate) timeout: Duration,
    pub(crate) max_bytes: usize,
    operator_openrouter_proxy: Option<(String, reqwest::Client)>,
    #[cfg(feature = "test-fixtures")]
    additional_roots: Vec<reqwest::Certificate>,
}
impl HttpCompletion {
    pub fn new(timeout: Duration, max_bytes: usize) -> Result<Self, Error> {
        if timeout.is_zero()
            || timeout > Duration::from_secs(300)
            || !(1024..=4 * 1024 * 1024).contains(&max_bytes)
        {
            return Err(invalid());
        }
        Ok(Self {
            operator_openrouter_proxy: None,
            #[cfg(feature = "test-fixtures")]
            additional_roots: Vec::new(),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(timeout)
                .build()
                .map_err(|_| invalid())?,
            timeout,
            max_bytes,
        })
    }
    /// Add bounded DER roots only in explicitly feature-enabled fixture builds.
    /// System roots, hostname verification, DNS policy and budgets remain intact.
    /// No environment variables, resolver overrides or alternate origins are used.
    #[cfg(feature = "test-fixtures")]
    pub fn with_additional_roots(
        timeout: Duration,
        max_bytes: usize,
        roots_der: &[&[u8]],
    ) -> Result<Self, FixtureTrustError> {
        if roots_der.is_empty() || roots_der.len() > 8 {
            return Err(FixtureTrustError::RootCount(roots_der.len()));
        }
        let mut roots = Vec::with_capacity(roots_der.len());
        for (index, der) in roots_der.iter().enumerate() {
            if der.is_empty() || der.len() > 16384 {
                return Err(FixtureTrustError::RootSize {
                    index,
                    bytes: der.len(),
                });
            }
            let root = reqwest::Certificate::from_der(der)
                .map_err(|_| FixtureTrustError::InvalidRoot { index })?;
            // reqwest's rustls DER parsing is deferred until client construction.
            reqwest::Client::builder()
                .no_proxy()
                .add_root_certificate(root.clone())
                .build()
                .map_err(|_| FixtureTrustError::InvalidRoot { index })?;
            roots.push(root);
        }
        if timeout.is_zero()
            || timeout > Duration::from_secs(300)
            || !(1024..=4 * 1024 * 1024).contains(&max_bytes)
        {
            return Err(FixtureTrustError::InvalidBudgets);
        }
        let mut completion = Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(timeout)
                .build()
                .map_err(FixtureTrustError::Client)?,
            timeout,
            max_bytes,
            operator_openrouter_proxy: None,
            additional_roots: roots,
        };
        completion.client = completion
            .client_builder()
            .build()
            .map_err(FixtureTrustError::Client)?;
        Ok(completion)
    }

    fn client_builder(&self) -> reqwest::ClientBuilder {
        let builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(self.timeout);
        #[cfg(feature = "test-fixtures")]
        let builder = self.additional_roots.iter().fold(builder, |builder, root| {
            builder.add_root_certificate(root.clone())
        });
        builder
    }

    /// Explicit operator-only proxy for an exact configured OpenRouter base.
    /// BYOK always uses pinned public DNS, no proxy and no redirects.
    pub fn with_operator_openrouter_proxy(
        mut self,
        base: &str,
        proxy: &str,
    ) -> Result<Self, Error> {
        let base_url = url::Url::parse(base).map_err(|_| invalid())?;
        let proxy_url = url::Url::parse(proxy).map_err(|_| invalid())?;
        if base_url.scheme() != "https"
            || base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || base.len() > 2048
            || !matches!(proxy_url.scheme(), "http" | "https" | "socks5" | "socks5h")
            || proxy_url.host_str().is_none()
            || proxy.len() > 2048
        {
            return Err(invalid());
        }
        let client = self
            .client_builder()
            .proxy(reqwest::Proxy::all(proxy).map_err(|_| invalid())?)
            .build()
            .map_err(|_| invalid())?;
        self.operator_openrouter_proxy = Some((base.trim_end_matches('/').into(), client));
        Ok(self)
    }
    async fn attempt(&self, r: &Route, prompt: &str) -> Result<String, Error> {
        let (endpoint, headers, body) = contract(r, prompt)?;
        let client = self.route_client(r).await?;
        let mut request = client
            .post(endpoint)
            .header("content-type", "application/json")
            .body(body.to_string());
        for (name, value) in headers {
            request = request.header(name, value)
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| Error::Code("provider_unavailable"))?;
        if !response.status().is_success() {
            return Err(status(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|n| n > self.max_bytes as u64)
        {
            return Err(Error::Code("response_too_large"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::Code("provider_unavailable"))?
        {
            if bytes.len() + chunk.len() > self.max_bytes {
                return Err(Error::Code("response_too_large"));
            }
            bytes.extend_from_slice(&chunk)
        }
        decode(
            r.shape,
            &serde_json::from_slice::<Value>(&bytes).map_err(|_| Error::Code("parse_failed"))?,
        )
    }
}
impl Completion for HttpCompletion {
    fn stream<'a>(
        &'a self,
        routes: &'a [Route],
        prompt: &'a str,
        sink: &'a dyn DeltaSink,
        cancel: CancellationToken,
    ) -> FutureResult<'a, String> {
        Box::pin(self.stream_routes(routes, prompt, sink, cancel))
    }

    fn models<'a>(
        &'a self,
        route: &'a Route,
        cancel: CancellationToken,
    ) -> FutureResult<'a, Vec<Model>> {
        Box::pin(async move {
            tokio::select! {biased;_=cancel.cancelled()=>Err(Error::Code("cancelled")),r=tokio::time::timeout(self.timeout,self.catalogue(route))=>r.unwrap_or(Err(Error::Code("provider_unavailable")))}
        })
    }
    fn complete<'a>(
        &'a self,
        routes: &'a [Route],
        prompt: &'a str,
        cancel: CancellationToken,
    ) -> FutureResult<'a, String> {
        Box::pin(async move {
            if prompt.len() > 1024 * 1024 {
                return Err(invalid());
            }
            let operation = async {
                let mut last = Error::Code("no_route");
                for route in routes {
                    match self.attempt(route, prompt).await {
                        Ok(text) => return Ok(text),
                        Err(e) => {
                            let retry = matches!(
                                e,
                                Error::Code("provider_unavailable" | "model_not_found")
                            );
                            last = e;
                            if route.is_byok() || !retry {
                                break;
                            }
                        }
                    }
                }
                Err(last)
            };
            tokio::select! {biased;_=cancel.cancelled()=>Err(Error::Code("cancelled")),r=tokio::time::timeout(self.timeout,operation)=>r.unwrap_or(Err(Error::Code("provider_unavailable")))}
        })
    }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Model {
    pub id: String,
    pub supports_websearch: bool,
}
/// Reject private, multicast, documentation and transition addresses for BYOK.
/// Pinned DNS answers below close DNS-rebinding between validation and connect.
pub fn public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v) => {
            let o = v.octets();
            !v.is_private()
                && !v.is_loopback()
                && !v.is_link_local()
                && !v.is_unspecified()
                && !v.is_multicast()
                && !v.is_broadcast()
                && !v.is_documentation()
                && o[0] != 0
                && o[0] < 224
                && !(o[0] == 100 && (64..=127).contains(&o[1]))
                && !(o[0] == 198 && (o[1] == 18 || o[1] == 19))
                && !(o[0] == 192 && o[1] == 0 && o[2] == 0)
        }
        std::net::IpAddr::V6(v) => {
            let s = v.segments();
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && (s[1] == 0xdb8 || s[1] <= 0x1ff))
                && s[0] != 0x2002
        }
    }
}
impl HttpCompletion {
    pub(crate) async fn route_client(&self, r: &Route) -> Result<reqwest::Client, Error> {
        if !r.is_byok() {
            if let Some((base, client)) = &self.operator_openrouter_proxy
                && r.base.trim_end_matches('/') == base
            {
                return Ok(client.clone());
            }
            return Ok(self.client.clone());
        }
        let url = url::Url::parse(&r.base).map_err(|_| invalid())?;
        let host = url.host_str().ok_or_else(invalid)?;
        let port = url.port_or_known_default().ok_or_else(invalid)?;
        let addresses = tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| Error::Code("provider_unavailable"))?
            .collect::<Vec<_>>();
        self.pinned_client(host, &addresses)
    }

    fn pinned_client(
        &self,
        host: &str,
        addresses: &[std::net::SocketAddr],
    ) -> Result<reqwest::Client, Error> {
        if addresses.is_empty()
            || addresses.len() > 32
            || addresses.iter().any(|a| !public_ip(a.ip()))
        {
            return Err(Error::Code("unsafe_endpoint"));
        }
        self.client_builder()
            .resolve_to_addrs(host, addresses)
            .build()
            .map_err(|_| Error::Code("provider_unavailable"))
    }
    async fn catalogue(&self, r: &Route) -> Result<Vec<Model>, Error> {
        let client = self.route_client(r).await?;
        let base = r.base.trim_end_matches('/');
        let host = url::Url::parse(base)
            .map_err(|_| invalid())?
            .host_str()
            .unwrap_or("")
            .to_owned();
        if r.via.ends_with("openrouter") || host == "openrouter.ai" {
            let response = client
                .get(format!("{base}/key"))
                .bearer_auth(&r.key)
                .send()
                .await
                .map_err(|_| Error::Code("provider_unavailable"))?;
            if !response.status().is_success() {
                return Err(status(response.status().as_u16()));
            }
        }
        if r.via.ends_with("perplexity") || host == "api.perplexity.ai" {
            let response = client.post(format!("{base}/chat/completions")).bearer_auth(&r.key).json(&json!({"model":"sonar","messages":[{"role":"user","content":"ping"}],"max_tokens":1,"stream":false})).send().await.map_err(|_| Error::Code("provider_unavailable"))?;
            if !response.status().is_success() {
                return Err(status(response.status().as_u16()));
            }
            return Ok([
                "sonar",
                "sonar-pro",
                "sonar-reasoning",
                "sonar-reasoning-pro",
                "sonar-deep-research",
            ]
            .into_iter()
            .map(|id| Model {
                id: id.into(),
                supports_websearch: true,
            })
            .collect());
        }
        let catalogue_base = if r.shape == Shape::Anthropic {
            base.strip_suffix("/messages").unwrap_or(base)
        } else {
            base
        };
        let endpoint =
            if r.shape == Shape::Anthropic && r.is_byok() && !catalogue_base.ends_with("/v1") {
                format!("{catalogue_base}/v1/models")
            } else {
                format!("{catalogue_base}/models")
            };
        let mut cursor: Option<String> = None;
        let mut seen = std::collections::BTreeSet::new();
        let mut models = Vec::new();
        let mut total_bytes = 0usize;
        // A single deadline covers all pages; tokens never become arbitrary URLs.
        for _ in 0..20 {
            let mut request = client.get(&endpoint);
            if let Some(token) = &cursor {
                request = request.query(&[(
                    if r.shape == Shape::Gemini {
                        "pageToken"
                    } else {
                        "after_id"
                    },
                    token,
                )]);
            }
            request = match r.shape {
                Shape::OpenAi if r.key.is_empty() => request,
                Shape::OpenAi => request.header("authorization", format!("Bearer {}", r.key)),
                Shape::Anthropic => request
                    .header("x-api-key", &r.key)
                    .header("anthropic-version", "2023-06-01"),
                Shape::Gemini => request.header("x-goog-api-key", &r.key),
            };
            let mut response = request
                .send()
                .await
                .map_err(|_| Error::Code("provider_unavailable"))?;
            if !response.status().is_success() {
                return Err(status(response.status().as_u16()));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| Error::Code("provider_unavailable"))?
            {
                total_bytes = total_bytes
                    .checked_add(chunk.len())
                    .ok_or(Error::Code("response_too_large"))?;
                if total_bytes > self.max_bytes {
                    return Err(Error::Code("response_too_large"));
                }
                bytes.extend_from_slice(&chunk);
            }
            let body: Value =
                serde_json::from_slice(&bytes).map_err(|_| Error::Code("parse_failed"))?;
            let rows = body[if r.shape == Shape::Gemini {
                "models"
            } else {
                "data"
            }]
            .as_array()
            .ok_or(Error::Code("parse_failed"))?;
            for v in rows {
                if r.shape == Shape::Gemini
                    && !v["supportedGenerationMethods"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|m| m == "generateContent"))
                {
                    continue;
                }
                let id = v[if r.shape == Shape::Gemini {
                    "name"
                } else {
                    "id"
                }]
                .as_str()
                .unwrap_or("")
                .trim_start_matches("models/");
                if id.is_empty() || id.len() > 128 {
                    continue;
                }
                let mid = id.to_lowercase();
                let search = v["supported_parameters"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v == "web_search_options"))
                    || mid.starts_with("sonar")
                    || mid.starts_with("perplexity/")
                    || mid.ends_with(":online")
                    || mid.ends_with("-online")
                    || mid.contains("-search-preview");
                // Gemini names alone do not prove executable grounding support.
                models.push(Model {
                    id: id.into(),
                    supports_websearch: search,
                });
                if models.len() > 2000 {
                    return Err(Error::Code("response_too_large"));
                }
            }
            let next = match r.shape {
                Shape::Gemini => body["nextPageToken"].as_str(),
                Shape::Anthropic if body["has_more"] == true => Some(
                    body["last_id"]
                        .as_str()
                        .ok_or(Error::Code("parse_failed"))?,
                ),
                _ => None,
            }
            .filter(|s| !s.is_empty());
            match next {
                None => {
                    models.sort_by(|a, b| a.id.cmp(&b.id));
                    models.dedup_by(|a, b| a.id == b.id);
                    return Ok(models);
                }
                Some(token) if token.len() <= 4096 && seen.insert(token.to_owned()) => {
                    cursor = Some(token.into())
                }
                Some(_) => return Err(Error::Code("catalogue_pagination_invalid")),
            }
        }
        Err(Error::Code("catalogue_pagination_invalid"))
    }
}

#[cfg(all(test, feature = "test-fixtures"))]
#[path = "provider/tests.rs"]
mod tests;
