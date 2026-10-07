//! Confined local probe results, not extractor prose or remote stream URLs.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MediaInfoReport {
    pub size_bytes: u64,
    pub streams: Vec<MediaInfoStream>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MediaInfoStream {
    pub kind: String,
    pub codec: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}
impl MediaInfoReport {
    pub fn valid(&self) -> bool {
        self.size_bytes > 0
            && (1..=16).contains(&self.streams.len())
            && self.streams.iter().all(|s| {
                ["video", "audio", "subtitle", "other"].contains(&s.kind.as_str())
                    && [
                        "h264", "hevc", "av1", "vp9", "vp8", "aac", "mp3", "opus", "vorbis",
                        "flac", "unknown",
                    ]
                    .contains(&s.codec.as_str())
                    && s.width.is_none_or(|v| v <= 16384)
                    && s.height.is_none_or(|v| v <= 16384)
            })
    }
    pub fn render(&self) -> String {
        let mut lines = vec![format!("Media info\nSize: {} bytes", self.size_bytes)];
        for s in &self.streams {
            lines.push(format!(
                "{}: {}{}",
                s.kind,
                s.codec,
                match (s.width, s.height) {
                    (Some(w), Some(h)) => format!(" {w}×{h}"),
                    _ => String::new(),
                }
            ));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report_is_closed_bounded_technical_data() {
        let base = r#"{"size_bytes":123,"streams":[{"kind":"video","codec":"h264","width":1280,"height":720}]}"#;
        let value: MediaInfoReport = serde_json::from_str(base).unwrap();
        assert!(value.valid());
        assert!(value.render().contains("1280×720"));
        assert!(value.render().len() < 4096);
        for bad in [
            base.replace("h264", "<script>"),
            base.replace("1280", "999999"),
        ] {
            assert!(
                !serde_json::from_str::<MediaInfoReport>(&bad)
                    .unwrap()
                    .valid()
            );
        }
        assert!(
            serde_json::from_str::<MediaInfoReport>(
                &base.replace("\"kind\":", "\"filename\":\"/private\",\"kind\":")
            )
            .is_err()
        );
    }
}
