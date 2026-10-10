//! CommonMark rendering and UTF-16 page boundaries owned by the Telegram adapter.
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use serde_json::{Value, json};
use url::Url;
use yoink_storage::StoreError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FormattedText {
    text: String,
    entities: Vec<Value>,
}
impl FormattedText {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }
    pub(crate) fn entities(&self) -> Value {
        self.entities.clone().into()
    }
    pub(crate) fn with_prefix(mut self, prefix: &str) -> Self {
        let offset = prefix.encode_utf16().count();
        self.text.insert_str(0, prefix);
        for entity in &mut self.entities {
            entity["offset"] = (entity["offset"].as_u64().unwrap() as usize + offset).into();
        }
        self
    }

    pub(crate) fn markdown(header: Option<(&str, &str)>, body: &str) -> Result<Self, StoreError> {
        let mut out = Renderer::default();
        if let Some((label, source)) = header {
            out.push(label);
            out.entity("bold", 0, out.offset, None);
            if let Some(url) = safe_link(source) {
                out.entity("text_link", 0, out.offset, Some(("url", url)));
            }
            out.push("\n\n");
        }
        for event in Parser::new_ext(body, Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES) {
            match event {
                Event::Start(tag) => out.start(tag),
                Event::End(tag) => out.end(tag),
                Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => out.push(&text),
                Event::Code(text) => {
                    let start = out.offset;
                    out.push(&text);
                    out.entity("code", start, out.offset, None);
                }
                Event::SoftBreak | Event::HardBreak => out.push("\n"),
                Event::Rule => out.separate(2),
                Event::TaskListMarker(checked) => out.push(if checked { "[x] " } else { "[ ] " }),
                Event::FootnoteReference(text) => out.push(&format!("[{text}]")),
                Event::InlineMath(text) | Event::DisplayMath(text) => out.push(&text),
            }
            // Reference links can expand a small response into much larger entity metadata.
            if out.text.len().saturating_add(out.entity_bytes) > 4 * 1024 * 1024 {
                return Err(StoreError::InvalidInput);
            }
        }
        let end = out.text.trim_end_matches('\n').len();
        out.text.truncate(end);
        let length = out.text.encode_utf16().count();
        let entities = normalize(out.entities, length);
        Ok(Self {
            text: out.text,
            entities,
        })
    }

    /// Prefer a paragraph, line or word boundary in the last half of each page.
    /// Entity offsets and clipping use UTF-16, while slicing uses UTF-8 boundaries.
    pub(crate) fn pages(&self, limit: usize) -> Vec<Self> {
        assert!((2..=4096).contains(&limit));
        let mut pages = Vec::new();
        let mut byte_start = 0;
        let mut unit_start = 0;
        while byte_start < self.text.len() {
            let rest = &self.text[byte_start..];
            let mut units = 0;
            let mut end = rest.len();
            for (byte, c) in rest.char_indices() {
                if units + c.len_utf16() > limit {
                    end = byte;
                    break;
                }
                units += c.len_utf16();
            }
            if end < rest.len() {
                let window = &rest[..end];
                let candidates = [
                    window.rfind("\n\n").map(|i| i + 2),
                    window.rfind('\n').map(|i| i + 1),
                    window
                        .rfind(char::is_whitespace)
                        .map(|i| i + window[i..].chars().next().unwrap().len_utf8()),
                ];
                if let Some(boundary) = candidates
                    .into_iter()
                    .flatten()
                    .find(|&i| window[..i].encode_utf16().count() >= limit / 2)
                {
                    end = boundary;
                }
            }
            let text = rest[..end].to_owned();
            let count = text.encode_utf16().count();
            let entities = self
                .entities
                .iter()
                .filter_map(|e| {
                    let start = e["offset"].as_u64()? as usize;
                    let finish = start + e["length"].as_u64()? as usize;
                    let left = start.max(unit_start);
                    let right = finish.min(unit_start + count);
                    if left >= right {
                        return None;
                    }
                    let mut clipped = e.clone();
                    clipped["offset"] = (left - unit_start).into();
                    clipped["length"] = (right - left).into();
                    Some(clipped)
                })
                .collect();
            pages.push(Self { text, entities });
            byte_start += end;
            unit_start += count;
        }
        pages
            .into_iter()
            .flat_map(|page| {
                // Leave room for the incoming snapshot and the Bot API receipt envelope.
                let bytes = serde_json::to_vec(&json!({"text":page.text,"entities":page.entities}))
                    .unwrap()
                    .len();
                let half = page.text.encode_utf16().count() / 2;
                if bytes > 32 * 1024 && half >= 2 {
                    page.pages(half)
                } else {
                    vec![page]
                }
            })
            .collect()
    }
}

struct Frame {
    end: TagEnd,
    kind: Option<&'static str>,
    start: usize,
    extra: Option<(&'static str, String)>,
}
#[derive(Default)]
struct Renderer {
    text: String,
    entity_bytes: usize,
    offset: usize,
    frames: Vec<Frame>,
    entities: Vec<Value>,
    lists: Vec<Option<u64>>,
}
impl Renderer {
    fn push(&mut self, text: &str) {
        self.text.push_str(text);
        self.offset += text.encode_utf16().count();
    }
    fn separate(&mut self, count: usize) {
        if self.text.is_empty() {
            return;
        }
        let trailing = self.text.chars().rev().take_while(|&c| c == '\n').count();
        for _ in trailing..count {
            self.push("\n");
        }
    }
    fn entity(&mut self, kind: &str, start: usize, end: usize, extra: Option<(&str, String)>) {
        if end <= start {
            return;
        }
        let mut e = json!({"type":kind,"offset":start,"length":end-start});
        if let Some((key, value)) = extra {
            e[key] = value.into();
        }
        self.entity_bytes = self
            .entity_bytes
            .saturating_add(serde_json::to_vec(&e).unwrap().len());
        self.entities.push(e);
    }
    fn start(&mut self, tag: Tag<'_>) {
        let end = tag.to_end();
        let mut extra = None;
        let kind = match tag {
            Tag::Strong | Tag::Heading { .. } | Tag::TableHead => Some("bold"),
            Tag::Emphasis => Some("italic"),
            Tag::Strikethrough => Some("strikethrough"),
            Tag::Link { dest_url, .. } => {
                extra = safe_link(&dest_url).map(|url| ("url", url));
                extra.as_ref().map(|_| "text_link")
            }
            Tag::BlockQuote(_) => {
                self.separate(2);
                (!self
                    .frames
                    .iter()
                    .any(|frame| matches!(frame.end, TagEnd::BlockQuote(_))))
                .then_some("blockquote")
            }
            Tag::CodeBlock(code) => {
                self.separate(2);
                if let CodeBlockKind::Fenced(info) = code {
                    let language = info.split_whitespace().next().unwrap_or("");
                    if language.len() <= 64
                        && language
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+'))
                        && !language.is_empty()
                    {
                        extra = Some(("language", language.into()));
                    }
                }
                Some("pre")
            }
            Tag::List(number) => {
                self.separate(1);
                self.lists.push(number);
                None
            }
            Tag::Item => {
                self.separate(1);
                self.push(&"  ".repeat(self.lists.len().saturating_sub(1)));
                if let Some(Some(number)) = self.lists.last_mut() {
                    let prefix = format!("{number}. ");
                    *number = number.saturating_add(1);
                    self.push(&prefix);
                } else {
                    self.push("- ");
                }
                None
            }
            _ => None,
        };
        self.frames.push(Frame {
            end,
            kind,
            start: self.offset,
            extra,
        });
    }
    fn end(&mut self, end: TagEnd) {
        if let Some(Frame {
            kind: Some(kind),
            start,
            extra,
            ..
        }) = self.frames.pop()
        {
            self.entity(kind, start, self.offset, extra);
        }
        match end {
            TagEnd::Paragraph => self.separate(if self.lists.is_empty() { 2 } else { 1 }),
            TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::BlockQuote(_) => self.separate(2),
            TagEnd::List(_) => {
                self.lists.pop();
                self.separate(2);
            }
            TagEnd::Item | TagEnd::TableRow | TagEnd::TableHead => self.separate(1),
            TagEnd::TableCell => self.push(" | "),
            _ => {}
        }
    }
}
fn safe_link(raw: &str) -> Option<String> {
    if raw.len() > 4096 {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    (matches!(url.scheme(), "https" | "http" | "mailto")
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.to_string())
}
fn normalize(entities: Vec<Value>, limit: usize) -> Vec<Value> {
    // Telegram forbids formatting across code/pre, and quotations containing links.
    let code = protected_spans(&entities, false);
    let quotes = protected_spans(&entities, true);
    let mut out = Vec::new();
    for entity in &entities {
        let start = entity["offset"].as_u64().unwrap() as usize;
        let end = (start + entity["length"].as_u64().unwrap() as usize).min(limit);
        let protected = if matches!(entity["type"].as_str(), Some("code" | "pre")) {
            &[][..]
        } else if entity["type"] == "blockquote" {
            quotes.as_slice()
        } else {
            code.as_slice()
        };
        let mut cursor = start;
        let mut spans = Vec::new();
        let first = protected.partition_point(|&(_, right)| right <= start);
        for &(left, right) in protected[first..]
            .iter()
            .take_while(|&&(left, _)| left < end)
        {
            if cursor < left {
                spans.push((cursor, left.min(end)));
            }
            cursor = cursor.max(right);
        }
        if cursor < end {
            spans.push((cursor, end));
        }
        for (start, end) in spans.into_iter().filter(|(a, b)| a < b) {
            let mut e = entity.clone();
            e["offset"] = start.into();
            e["length"] = (end - start).into();
            out.push(e);
        }
    }
    out.sort_by_key(|e| {
        (
            e["offset"].as_u64(),
            std::cmp::Reverse(e["length"].as_u64()),
            e["type"].as_str().unwrap().to_owned(),
        )
    });
    out.dedup();
    out
}

fn protected_spans(entities: &[Value], links: bool) -> Vec<(usize, usize)> {
    let mut spans: Vec<_> = entities
        .iter()
        .filter(|e| {
            matches!(e["type"].as_str(), Some("code" | "pre"))
                || (links && e["type"] == "text_link")
        })
        .map(|e| {
            let start = e["offset"].as_u64().unwrap() as usize;
            (start, start + e["length"].as_u64().unwrap() as usize)
        })
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in spans {
        if let Some(last) = merged.last_mut().filter(|last| last.1 >= start) {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commonmark_entities_links_and_code_are_not_raw_markup() {
        let text = FormattedText::markdown(
            Some(("Краткое содержание", "https://example.com/source")),
            "# Заголовок\n\n😀 **важно _внутри_ `token`**\n\n> цитата [link](https://example.org)\n>\n> ```rust\n> let x = 1;\n> ```\n\n~~нет~~ [unsafe](javascript:alert(1))",
        ).unwrap();
        assert!(text.text.starts_with("Краткое содержание\n\nЗаголовок"));
        assert!(!text.text.contains("**"));
        assert!(!text.text.contains("```"));
        for kind in [
            "bold",
            "italic",
            "pre",
            "code",
            "text_link",
            "blockquote",
            "strikethrough",
        ] {
            assert!(text.entities.iter().any(|e| e["type"] == kind), "{kind}");
        }
        assert!(!text.entities.iter().any(|e| {
            e["url"]
                .as_str()
                .is_some_and(|u| u.starts_with("javascript:"))
        }));
        for code in text
            .entities
            .iter()
            .filter(|e| matches!(e["type"].as_str(), Some("code" | "pre")))
        {
            let a = code["offset"].as_u64().unwrap();
            let b = a + code["length"].as_u64().unwrap();
            for e in text
                .entities
                .iter()
                .filter(|e| !matches!(e["type"].as_str(), Some("code" | "pre")))
            {
                let c = e["offset"].as_u64().unwrap();
                let d = c + e["length"].as_u64().unwrap();
                assert!(b <= c || d <= a);
            }
        }
    }
    #[test]
    fn split_preserves_utf16_entities_and_prefers_paragraphs() {
        let body = format!("**{}\n\n{}**", "😀".repeat(1400), "Ж".repeat(6000));
        let text = FormattedText::markdown(None, &body).unwrap();
        let pages = text.pages(4000);
        assert_eq!(
            pages.iter().map(|p| p.text.as_str()).collect::<String>(),
            text.text
        );
        assert!(pages[0].text.ends_with("\n\n"));
        for page in pages {
            let count = page.text.encode_utf16().count() as u64;
            assert!(count <= 4000);
            for e in page.entities {
                assert!(e["offset"].as_u64().unwrap() + e["length"].as_u64().unwrap() <= count);
            }
        }
    }
    #[test]
    fn reference_expansion_is_bounded_and_dense_entities_fit_the_receipt_budget() {
        let link = format!("https://example.com/{}", "a".repeat(3000));
        let small = format!("{}\n\n[label]: {link}", "[😀][label] ".repeat(100));
        let text = FormattedText::markdown(None, &small).unwrap();
        let pages = text.pages(4000);
        assert!(pages.len() > 1);
        assert_eq!(
            pages.iter().map(|p| p.text.as_str()).collect::<String>(),
            text.text
        );
        for page in pages {
            assert!(
                serde_json::to_vec(&json!({"text":page.text(),"entities":page.entities()}))
                    .unwrap()
                    .len()
                    <= 32 * 1024
            );
            for entity in &page.entities {
                assert!(
                    entity["offset"].as_u64().unwrap() + entity["length"].as_u64().unwrap()
                        <= page.text.encode_utf16().count() as u64
                );
            }
        }
        let expanded = format!("{}\n\n[label]: {link}", "[x][label] ".repeat(60000));
        assert!(expanded.len() < 1024 * 1024);
        let rendered = FormattedText::markdown(None, &expanded).unwrap();
        assert!(serde_json::to_vec(&rendered.entities).unwrap().len() <= 2 * expanded.len());
        assert!(matches!(
            FormattedText::markdown(None, &"**x** ".repeat(100000)),
            Err(StoreError::InvalidInput)
        ));
    }
}
