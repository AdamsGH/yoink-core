//! Incremental SSE consumption. This never splits a buffered completion into
//! artificial deltas. The awaited sink applies backpressure to network reads.
use super::provider::{HttpCompletion, Route, Shape, contract, status};
use crate::{Error, FutureResult, invalid};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
#[derive(Debug)]
pub struct StreamDiagnostic<'a> {
    pub attempt: usize,
    pub shape: Shape,
    pub route: &'static str,
    pub outcome: &'a str,
    pub end_reason: &'static str,
    pub http_status: Option<u16>,
    pub wire_bytes: usize,
    pub text_bytes: usize,
    pub deltas: usize,
    pub elapsed_ms: u64,
    pub first_delta_ms: Option<u64>,
}
/// Coordinator-owned delivery/Telegram editing. Text is plain untrusted model
/// output, NOT Telegram HTML. Sink failures abort and prohibit provider fallback.
pub trait DeltaSink: Send + Sync {
    /// Metadata only. Implementations must not add credentials or generated text.
    fn diagnostic(&self, _event: &StreamDiagnostic<'_>) {}
    fn delta<'a>(&'a self, text: &'a str) -> FutureResult<'a, ()>;
    fn phase<'a>(
        &'a self,
        _phase: Phase,
        _source: &'a str,
        _alias: Option<&'a str>,
    ) -> FutureResult<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Fetching,
    Researching,
    Generating,
    Cached,
}
/// Wire contract for an actual streaming request, not buffered generation.
pub fn stream_contract(
    route: &Route,
    prompt: &str,
) -> Result<super::provider::ProviderContract, Error> {
    let (mut endpoint, mut headers, mut body) = contract(route, prompt)?;
    match route.shape {
        Shape::OpenAi => body["stream"] = Value::Bool(true),
        Shape::Anthropic => {
            body["stream"] = Value::Bool(true);
            body["max_tokens"] = 4096.into();
        }
        Shape::Gemini => {
            endpoint = endpoint.replace(":generateContent", ":streamGenerateContent?alt=sse");
        }
    }
    headers.push(("accept", "text/event-stream".into()));
    Ok((endpoint, headers, body))
}
/// Byte-oriented line framing keeps split UTF-8 characters intact. Wire bytes,
/// including comments and ignored events, are bounded (not just generated text).
pub(crate) struct SseDecoder {
    pending: Vec<u8>,
    data: Vec<String>,
    bytes: usize,
    max_bytes: usize,
}
impl SseDecoder {
    pub(crate) fn new(max_bytes: usize) -> Self {
        Self {
            pending: Vec::new(),
            data: Vec::new(),
            bytes: 0,
            max_bytes,
        }
    }
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, Error> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .ok_or(Error::Code("response_too_large"))?;
        if self.bytes > self.max_bytes {
            return Err(Error::Code("response_too_large"));
        }
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();
        let mut consumed = 0;
        while let Some(end) = self.pending[consumed..].iter().position(|b| *b == b'\n') {
            let end = consumed + end;
            let line = std::str::from_utf8(&self.pending[consumed..end])
                .map_err(|_| Error::Code("parse_failed"))?
                .trim_end_matches('\r');
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(self.data.join("\n"));
                    self.data.clear();
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data
                    .push(data.strip_prefix(' ').unwrap_or(data).to_owned());
            }
            consumed = end + 1;
        }
        self.pending.drain(..consumed);
        Ok(events)
    }
    pub(crate) fn finish(&mut self) -> Result<Vec<String>, Error> {
        // Bound the actual wire, not these synthetic line delimiters at EOF.
        self.max_bytes = self.max_bytes.saturating_add(2);
        self.push(b"\n\n")
    }
}
struct StreamState {
    text: String,
    terminal: bool,
    emitted: bool,
    started: std::time::Instant,
    first_delta_ms: Option<u64>,
    deltas: usize,
    wire_bytes: usize,
    end_reason: &'static str,
    http_status: Option<u16>,
}
impl StreamState {
    async fn event(
        &mut self,
        shape: Shape,
        event: &str,
        sink: &dyn DeltaSink,
    ) -> Result<(), Error> {
        if self.terminal {
            return Ok(());
        }
        if event.trim() == "[DONE]" {
            if shape != Shape::OpenAi {
                return Err(Error::Code("parse_failed"));
            }
            self.terminal = true;
            self.end_reason = "done";
            return Ok(());
        }
        let value: Value = serde_json::from_str(event).map_err(|_| Error::Code("parse_failed"))?;
        if value.get("error").is_some() || value["type"] == "error" {
            return Err(Error::Code("llm_error"));
        }
        let (delta, terminal) = match shape {
            Shape::OpenAi => {
                let choice = value.pointer("/choices/0");
                let text = choice
                    .and_then(|c| c.pointer("/delta/content"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let finish = choice
                    .and_then(|c| c.get("finish_reason"))
                    .and_then(Value::as_str);
                self.end_reason = match finish {
                    Some("stop") => "stop",
                    Some("length") => "length",
                    Some("content_filter") => "content_filter",
                    Some(_) => "other",
                    None => self.end_reason,
                };
                if finish == Some("length") {
                    return Err(Error::Code("response_truncated"));
                }
                if finish == Some("content_filter") {
                    return Err(Error::Code("content_blocked"));
                }
                (text, finish.is_some())
            }
            Shape::Anthropic => {
                if value["type"] == "message_delta" {
                    self.end_reason = match value["delta"]["stop_reason"].as_str() {
                        Some("max_tokens") => "max_tokens",
                        Some("end_turn") => "end_turn",
                        Some("stop_sequence") => "stop_sequence",
                        Some(_) => "other",
                        None => self.end_reason,
                    };
                    if self.end_reason == "max_tokens" {
                        return Err(Error::Code("response_truncated"));
                    }
                }
                let text = if value["type"] == "content_block_delta"
                    && value["delta"]["type"] == "text_delta"
                {
                    value["delta"]["text"].as_str().unwrap_or("").to_owned()
                } else {
                    String::new()
                };
                (text, value["type"] == "message_stop")
            }
            Shape::Gemini => {
                if value.pointer("/promptFeedback/blockReason").is_some() {
                    return Err(Error::Code("content_blocked"));
                }
                let candidate = value.pointer("/candidates/0");
                let finish = candidate
                    .and_then(|c| c.get("finishReason"))
                    .and_then(Value::as_str);
                if let Some(reason) = finish {
                    self.end_reason = match reason {
                        "STOP" => "stop",
                        "MAX_TOKENS" => "max_tokens",
                        _ => "other",
                    };
                }
                if finish == Some("MAX_TOKENS") {
                    return Err(Error::Code("response_truncated"));
                }
                if finish.is_some_and(|f| !matches!(f, "STOP" | "MAX_TOKENS")) {
                    return Err(Error::Code("content_blocked"));
                }
                let text = candidate
                    .and_then(|c| c.pointer("/content/parts"))
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|p| p["text"].as_str())
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                (text, finish.is_some())
            }
        };
        if !delta.is_empty() {
            // Mark before invoking a sink: delivery failure or authorization
            // failure may not be hidden by switching to a different provider.
            self.emitted = true;
            self.deltas += 1;
            self.first_delta_ms
                .get_or_insert_with(|| self.started.elapsed().as_millis() as u64);
            sink.delta(&delta).await?;
            self.text.push_str(&delta);
        }
        self.terminal = terminal;
        Ok(())
    }
}
impl HttpCompletion {
    async fn stream_attempt(
        &self,
        route: &Route,
        prompt: &str,
        sink: &dyn DeltaSink,
        state: &mut StreamState,
    ) -> Result<String, Error> {
        let (endpoint, headers, body) = stream_contract(route, prompt)?;
        let client = self.route_client(route).await?;
        let mut request = client
            .post(endpoint)
            .header("content-type", "application/json")
            .body(body.to_string());
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| Error::Code("provider_unavailable"))?;
        state.http_status = Some(response.status().as_u16());
        if !response.status().is_success() {
            return Err(status(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|size| size > self.max_bytes as u64)
        {
            return Err(Error::Code("response_too_large"));
        }
        let mut decoder = SseDecoder::new(self.max_bytes);
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::Code("provider_unavailable"))?
        {
            state.wire_bytes = state.wire_bytes.saturating_add(chunk.len());
            for event in decoder.push(&chunk)? {
                state.event(route.shape, &event, sink).await?;
            }
            if state.terminal {
                break;
            }
        }
        if !state.terminal {
            for event in decoder.finish()? {
                state.event(route.shape, &event, sink).await?;
            }
        }
        if !state.terminal {
            return Err(Error::Code("stream_incomplete"));
        }
        if state.text.trim().is_empty() {
            return Err(Error::Code("empty_response"));
        }
        Ok(std::mem::take(&mut state.text))
    }
    pub(crate) async fn stream_routes(
        &self,
        routes: &[Route],
        prompt: &str,
        sink: &dyn DeltaSink,
        cancel: CancellationToken,
    ) -> Result<String, Error> {
        if prompt.len() > 1024 * 1024 {
            return Err(invalid());
        }
        let deadline = tokio::time::Instant::now() + self.timeout;
        let mut last = Error::Code("no_route");
        for (attempt, route) in routes.iter().enumerate() {
            let mut state = StreamState {
                text: String::new(),
                terminal: false,
                emitted: false,
                started: std::time::Instant::now(),
                first_delta_ms: None,
                deltas: 0,
                wire_bytes: 0,
                end_reason: "absent",
                http_status: None,
            };
            let (result, interrupted) = tokio::select! {
                biased;
                _ = cancel.cancelled() => (Err(Error::Code("cancelled")), Some("cancelled")),
                result = tokio::time::timeout_at(deadline, self.stream_attempt(route, prompt, sink, &mut state)) => match result {
                    Ok(result) => (result, None),
                    Err(_) => (Err(Error::Code("provider_unavailable")), Some("timeout")),
                }
            };
            let outcome = interrupted
                .map(str::to_owned)
                .unwrap_or_else(|| match &result {
                    Ok(_) => "ok".into(),
                    Err(error) => error.to_string(),
                });
            sink.diagnostic(&StreamDiagnostic {
                attempt: attempt + 1,
                shape: route.shape,
                route: route.via,
                outcome: &outcome,
                end_reason: state.end_reason,
                http_status: state.http_status,
                wire_bytes: state.wire_bytes,
                text_bytes: result.as_ref().map_or(state.text.len(), String::len),
                deltas: state.deltas,
                elapsed_ms: state.started.elapsed().as_millis() as u64,
                first_delta_ms: state.first_delta_ms,
            });
            match result {
                Ok(text) => return Ok(text),
                Err(error) => {
                    // Executable services/tldr.py retries gateway errors only
                    // before any text is emitted. A BYOK route has no fallback.
                    if interrupted.is_some() || state.emitted || route.is_byok() {
                        return Err(error);
                    }
                    last = error;
                }
            }
        }
        Err(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoder_bounds_ignored_wire_and_rejects_invalid_utf8() {
        let mut decoder = SseDecoder::new(8);
        assert!(matches!(
            decoder.push(b":ignored comment\n"),
            Err(Error::Code("response_too_large"))
        ));
        let mut decoder = SseDecoder::new(100);
        assert!(matches!(
            decoder.push(b"data: \xff\n\n"),
            Err(Error::Code("parse_failed"))
        ));
        let mut decoder = SseDecoder::new(100);
        assert_eq!(
            decoder
                .push(b":comment\r\ndata: one\r\ndata: two\r\n\r\n")
                .unwrap(),
            vec!["one\ntwo"]
        );
        assert_eq!(decoder.push(b"data: eof").unwrap(), Vec::<String>::new());
        assert_eq!(decoder.finish().unwrap(), vec!["eof"]);
    }
}
