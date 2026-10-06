//! Bounded extractor prose retained for reclassification, never for authorization.
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentMetadata {
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub categories: Vec<String>,
}
impl ContentMetadata {
    pub fn valid(&self) -> bool {
        self.title.chars().count() <= 2000
            && self.description.chars().count() <= 2000
            && [&self.tags, &self.categories]
                .into_iter()
                .all(|list| list.len() <= 100 && list.iter().all(|s| s.chars().count() <= 128))
    }
    pub fn prose(&self) -> String {
        format!(
            "{} {} {} {}",
            self.title,
            self.description,
            self.tags.join(" "),
            self.categories.join(" ")
        )
    }
}
impl std::fmt::Debug for ContentMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ContentMetadata([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nsfw_metadata_is_bounded_in_unicode_characters_and_debug_redacted() {
        let mut metadata = ContentMetadata {
            title: "я".repeat(2000),
            ..Default::default()
        };
        assert!(metadata.valid());
        metadata.title.push('я');
        assert!(!metadata.valid());
        metadata.title = "private-source-title".into();
        metadata.tags = vec!["tag".into(); 101];
        assert!(!metadata.valid());
        metadata.tags = vec!["tag".into()];
        assert!(metadata.valid());
        assert!(!format!("{metadata:?}").contains("private-source-title"));
    }
}
