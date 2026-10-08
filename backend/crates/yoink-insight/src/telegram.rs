//! Telegram-specific parsing; shared Bot API delivery remains coordinator-owned.
use crate::{
    Error, Insight, Request, Summary,
    content::normalize,
    invalid,
    prompts::{builtin, csv},
};
use tokio_util::sync::CancellationToken;
pub fn parse(text: &str, known: &[String]) -> Result<Request, Error> {
    let mut words = text.split_whitespace();
    let cmd = words
        .next()
        .ok_or_else(invalid)?
        .trim_start_matches('/')
        .split('@')
        .next()
        .ok_or_else(invalid)?;
    if !matches!(cmd, "about" | "summary" | "tldr") {
        return Err(invalid());
    }
    if cmd == "summary" || cmd == "about" {
        let url = text
            .char_indices()
            .filter(|(_, character)| *character == 'h' || *character == 'H')
            .find_map(|(position, _)| {
                let candidate = &text[position..];
                if !candidate
                    .get(..7)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
                    && !candidate
                        .get(..8)
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
                {
                    return None;
                }
                let candidate = candidate.split_whitespace().next()?;
                normalize(candidate)
                    .ok()
                    .filter(|url| crate::content::video_id(url).is_some())
            })
            .ok_or_else(invalid)?;
        return Ok(Request {
            command: cmd.into(),
            url: url.to_string(),
            alias: None,
            focus: None,
        });
    }
    let url = normalize(words.next().ok_or_else(invalid)?)?.to_string();
    let mut tail = words.map(str::to_owned).collect::<Vec<_>>();
    let alias = if cmd == "tldr"
        && tail.first().is_some_and(|s| {
            builtin(&s.to_lowercase()).is_some() || known.contains(&s.to_lowercase())
        }) {
        Some(tail.remove(0).to_lowercase())
    } else {
        None
    };
    let focus = if tail.is_empty() {
        None
    } else {
        Some(tail.join(" "))
    };
    Ok(Request {
        command: cmd.into(),
        url,
        alias,
        focus,
    })
}
pub async fn execute(
    service: &Insight,
    uid: i64,
    text: &str,
    cancel: CancellationToken,
) -> Result<Summary, Error> {
    let request = command_request(service, uid, text).await?;
    service.summarize(uid, request, cancel).await
}
/// Split plain-text Telegram responses without breaking UTF-8. The shared adapter
/// must use no parse mode here (untrusted model markup is not Telegram HTML).
pub fn messages(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    let mut units = 0;
    for c in text.chars() {
        if units + c.len_utf16() > 4000 {
            chunks.push(std::mem::take(&mut chunk));
            units = 0;
        }
        chunk.push(c);
        units += c.len_utf16();
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    chunks
}

/// Actual incremental execution for the runtime's draft/edit bridge. Awaited
/// sink errors abort generation. Never treat partial drafts as final success;
/// use the returned Summary only after its authorization/accounting commit.
pub async fn execute_stream(
    service: &Insight,
    uid: i64,
    text: &str,
    sink: &dyn crate::provider::DeltaSink,
    cancel: CancellationToken,
) -> Result<Summary, Error> {
    let request = command_request(service, uid, text).await?;
    service.summarize_stream(uid, request, sink, cancel).await
}

async fn command_request(service: &Insight, uid: i64, text: &str) -> Result<Request, Error> {
    let request = parse(text, &[])?;
    if request.command != "tldr" {
        return Ok(request);
    }
    let known = service
        .aliases(uid)
        .await?
        .iter()
        .flat_map(|alias| csv(alias.aliases.as_deref()))
        .collect::<Vec<_>>();
    parse(text, &known)
}
