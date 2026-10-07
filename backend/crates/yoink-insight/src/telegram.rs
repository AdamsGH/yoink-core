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
    let known = service
        .aliases(uid)
        .await?
        .iter()
        .flat_map(|a| csv(a.aliases.as_deref()))
        .collect::<Vec<_>>();
    service.summarize(uid, parse(text, &known)?, cancel).await
}
/// Split plain-text Telegram responses without breaking UTF-8. The shared adapter
/// must use no parse mode here (untrusted model markup is not Telegram HTML).
pub fn messages(text: &str) -> Vec<String> {
    text.chars()
        .collect::<Vec<_>>()
        .chunks(4000)
        .map(|c| c.iter().collect())
        .collect()
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
    let known = service
        .aliases(uid)
        .await?
        .iter()
        .flat_map(|a| csv(a.aliases.as_deref()))
        .collect::<Vec<_>>();
    service
        .summarize_stream(uid, parse(text, &known)?, sink, cancel)
        .await
}
