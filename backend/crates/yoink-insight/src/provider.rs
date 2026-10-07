pub use crate::streaming::{DeltaSink, stream_contract};
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
            Shape::Gemini,
            "https://generativelanguage.googleapis.com/v1beta",
        ),
        "custom_openai" => (Shape::OpenAi, ""),
        "custom_anthropic" => (Shape::Anthropic, ""),
        _ => return Err(invalid()),
    };
    let base = base.filter(|s| !s.is_empty()).unwrap_or(default);
    let url = url::Url::parse(base).map_err(|_| invalid())?;
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
        via: "byok",
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
            format!("{base}/messages"),
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
pub struct HttpCompletion {
    pub(crate) client: reqwest::Client,
    pub(crate) timeout: Duration,
    pub(crate) max_bytes: usize,
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
                            if !retry {
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
        if r.via != "byok" {
            return Ok(self.client.clone());
        }
        let url = url::Url::parse(&r.base).map_err(|_| invalid())?;
        let host = url.host_str().ok_or_else(invalid)?;
        let port = url.port_or_known_default().ok_or_else(invalid)?;
        let addresses = tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| Error::Code("provider_unavailable"))?
            .collect::<Vec<_>>();
        if addresses.is_empty()
            || addresses.len() > 32
            || addresses.iter().any(|a| !public_ip(a.ip()))
        {
            return Err(Error::Code("unsafe_endpoint"));
        }
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(self.timeout)
            .resolve_to_addrs(host, &addresses)
            .build()
            .map_err(|_| Error::Code("provider_unavailable"))
    }
    async fn catalogue(&self, r: &Route) -> Result<Vec<Model>, Error> {
        let client = self.route_client(r).await?;
        let endpoint = format!("{}/models", r.base.trim_end_matches('/'));
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
