//! Independent cited research using OpenAI Responses web-search, not source
//! paraphrases or uncited model-memory claims. The route is supplied from current
//! DB settings for every request, never cached alongside user credentials.
use crate::{
    Error, FutureResult,
    content::{Citation, normalize},
    invalid,
    provider::{Route, Shape, status},
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};
use tokio_util::sync::CancellationToken;
pub trait Research: Send + Sync {
    fn search<'a>(
        &'a self,
        route: &'a Route,
        query: &'a str,
        cancel: CancellationToken,
    ) -> FutureResult<'a, Vec<Citation>>;
}
pub fn citations(body: &Value) -> Result<Vec<Citation>, Error> {
    let output = body["output"]
        .as_array()
        .ok_or(Error::Code("research_invalid_response"))?;
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    for item in output {
        for content in item["content"].as_array().into_iter().flatten() {
            let text = content["text"].as_str().unwrap_or("");
            for annotation in content["annotations"].as_array().into_iter().flatten() {
                if annotation["type"] != "url_citation" {
                    continue;
                }
                let Some(raw) = annotation["url"].as_str() else {
                    continue;
                };
                let Ok(url) = normalize(raw) else { continue };
                if !seen.insert(url.to_string()) {
                    continue;
                }
                let chars = text.chars().collect::<Vec<_>>();
                let start = annotation["start_index"].as_u64().unwrap_or(0) as usize;
                let end = annotation["end_index"].as_u64().unwrap_or(start as u64) as usize;
                let left = start.saturating_sub(120).min(chars.len());
                let right = end.saturating_add(120).min(chars.len()).max(left);
                found.push(Citation {
                    url: url.to_string(),
                    title: annotation["title"]
                        .as_str()
                        .unwrap_or(raw)
                        .chars()
                        .take(500)
                        .collect(),
                    text: chars[left..right].iter().take(1000).collect(),
                });
                if found.len() >= 10 {
                    return Ok(found);
                }
            }
        }
    }
    if found.is_empty() {
        Err(Error::Code("research_no_sources"))
    } else {
        Ok(found)
    }
}
pub struct ResponsesResearch {
    client: reqwest::Client,
    timeout: Duration,
    max_bytes: usize,
}
impl ResponsesResearch {
    pub fn new(timeout: Duration, max_bytes: usize) -> Result<Self, Error> {
        if timeout.is_zero()
            || timeout > Duration::from_secs(180)
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
}
impl Research for ResponsesResearch {
    fn search<'a>(
        &'a self,
        route: &'a Route,
        query: &'a str,
        cancel: CancellationToken,
    ) -> FutureResult<'a, Vec<Citation>> {
        Box::pin(async move {
            if route.shape != Shape::OpenAi || route.via == "byok" || query.len() > 20000 {
                return Err(invalid());
            }
            let operation = async {
                let body = json!({"model":route.model,"input":format!("Research the material factual claims independently. Use primary sources where possible, explicitly distinguish verified, contradicted and unresolved claims, cite every external assertion. Do not count the source under review as corroboration.\n{query}"),"tools":[{"type":"web_search_preview"}],"stream":false});
                let mut response = self
                    .client
                    .post(format!("{}/responses", route.base.trim_end_matches('/')))
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {}", route.key))
                    .body(body.to_string())
                    .send()
                    .await
                    .map_err(|_| Error::Code("research_unavailable"))?;
                if !response.status().is_success() {
                    return Err(status(response.status().as_u16()));
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|_| Error::Code("research_unavailable"))?
                {
                    if bytes.len() + chunk.len() > self.max_bytes {
                        return Err(Error::Code("response_too_large"));
                    }
                    bytes.extend_from_slice(&chunk)
                }
                citations(
                    &serde_json::from_slice::<Value>(&bytes)
                        .map_err(|_| Error::Code("research_invalid_response"))?,
                )
            };
            tokio::select! {biased;_=cancel.cancelled()=>Err(Error::Code("cancelled")),r=tokio::time::timeout(self.timeout,operation)=>r.unwrap_or(Err(Error::Code("research_unavailable")))}
        })
    }
}
