use yoink_domain::DeliveryTarget;
use yoink_downloads::{DeliveryArtifact, DeliveryPresentation};
use yoink_i18n::{Key, Language};

use crate::translate;

pub(super) struct MediaCaption {
    pub html: String,
    pub above: bool,
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn clock(seconds: f64) -> String {
    if seconds >= 1_000_000_000.0 || (seconds > 0.0 && seconds < 0.001) {
        return format!("{seconds:.3e}s");
    }
    let millis = (seconds * 1000.0).round() as u64;
    let seconds = millis / 1000;
    let base = if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    };
    if millis.is_multiple_of(1000) {
        base
    } else {
        format!("{base}.{:03}", millis % 1000)
            .trim_end_matches('0')
            .to_owned()
    }
}

impl MediaCaption {
    pub fn new(
        target: DeliveryTarget,
        presentation: &DeliveryPresentation,
        artifacts: &[DeliveryArtifact],
    ) -> Option<Self> {
        if !presentation.first_unit || artifacts.is_empty() {
            return None;
        }
        let language = Language::from_code(Some(&presentation.language));
        let source = translate(language, Key::MediaSource, &[]);
        let url = escape(presentation.source_url.as_str());
        let private = target.chat_id() > 0 && target.thread_id().is_none();
        if !private {
            return Some(Self {
                html: format!(
                    "<a href=\"tg://user?id={}\">{}</a>\n\n<a href=\"{url}\">{}</a>",
                    presentation.actor_id,
                    presentation.actor_id,
                    escape(&source),
                ),
                above: false,
            });
        }
        let metadata = match &artifacts[0] {
            DeliveryArtifact::Upload(artifact) => artifact.metadata.as_ref(),
            DeliveryArtifact::Cached(artifact) => artifact.metadata(),
        };
        let title: String = metadata
            .map(|metadata| metadata.title.as_str())
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        let title = if title.trim().is_empty() {
            source.as_str()
        } else {
            title.trim()
        };
        let ranges = presentation
            .clips
            .iter()
            .map(|clip| {
                format!(
                    "✂️ {} → {}",
                    clock(clip.start_seconds()),
                    clock(clip.end_seconds())
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let tail = if ranges.is_empty() {
            String::new()
        } else {
            format!("\n\n{ranges}")
        };
        let available = 1024usize.saturating_sub(tail.encode_utf16().count());
        let truncated = title.encode_utf16().count() > available;
        let budget = available.saturating_sub(if truncated { 3 } else { 0 });
        let mut label = String::new();
        let mut length = 0;
        for c in title.chars() {
            length += c.len_utf16();
            if length > budget {
                break;
            }
            label.push(c);
        }
        if truncated {
            label.push_str("...");
        }
        Some(Self {
            html: format!("<a href=\"{url}\"><b>{}</b></a>{tail}", escape(&label)),
            above: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoink_domain::{CachedMedia, CachedMediaKind, ClipRange, ContentMetadata};

    fn artifact(title: &str) -> DeliveryArtifact {
        DeliveryArtifact::Cached(
            CachedMedia::new(CachedMediaKind::Video, "opaque_id".into(), 1024)
                .unwrap()
                .with_metadata(ContentMetadata {
                    title: title.into(),
                    ..Default::default()
                })
                .unwrap(),
        )
    }

    fn presentation() -> DeliveryPresentation {
        DeliveryPresentation {
            actor_id: 42,
            source_url: "https://example.com/watch?v=1&lang=en".parse().unwrap(),
            clips: vec![],
            language: "en".into(),
            first_unit: true,
        }
    }

    #[test]
    fn private_caption_preserves_literal_title_and_source_hyperlink() {
        let target = DeliveryTarget::new(1, 42, None, None).unwrap();
        let caption = MediaCaption::new(
            target,
            &presentation(),
            &[artifact("A <title> & \"quote\"")],
        )
        .unwrap();
        assert_eq!(
            caption.html,
            "<a href=\"https://example.com/watch?v=1&amp;lang=en\"><b>A &lt;title&gt; &amp; &quot;quote&quot;</b></a>"
        );
        assert!(caption.above);
    }

    #[test]
    fn long_unicode_title_keeps_its_link_and_room_for_all_clip_ranges() {
        let mut presentation = presentation();
        presentation.clips = vec![
            ClipRange::new(1.25, 65.5).unwrap(),
            ClipRange::new(3600.0, 3601.0).unwrap(),
        ];
        let target = DeliveryTarget::new(1, 42, None, None).unwrap();
        let caption =
            MediaCaption::new(target, &presentation, &[artifact(&"😀".repeat(1000))]).unwrap();
        assert!(caption.html.starts_with("<a href="));
        assert!(caption.html.contains("...</b></a>"));
        assert!(
            caption
                .html
                .ends_with("✂️ 00:01.25 → 01:05.5\n✂️ 1:00:00 → 1:00:01")
        );
        let text = caption
            .html
            .split("<b>")
            .nth(1)
            .unwrap()
            .replace("</b></a>", "");
        assert!(text.encode_utf16().count() <= 1024);
    }

    #[test]
    fn missing_titles_are_localized_and_only_the_first_unit_gets_a_caption() {
        let target = DeliveryTarget::new(1, 42, None, None).unwrap();
        let mut presentation = presentation();
        presentation.language = "ru".into();
        assert!(
            MediaCaption::new(target, &presentation, &[artifact("")])
                .unwrap()
                .html
                .contains("<b>источник</b>")
        );
        presentation.first_unit = false;
        assert!(MediaCaption::new(target, &presentation, &[artifact("title")]).is_none());
    }

    #[test]
    fn group_caption_mentions_the_requester_without_leaking_the_private_title() {
        let target = DeliveryTarget::new(1, -100, Some(7), None).unwrap();
        let caption =
            MediaCaption::new(target, &presentation(), &[artifact("private title")]).unwrap();
        assert!(
            caption
                .html
                .starts_with("<a href=\"tg://user?id=42\">42</a>")
        );
        assert!(!caption.html.contains("private title"));
        assert!(!caption.above);
    }
}
