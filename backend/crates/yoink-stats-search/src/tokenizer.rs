use tantivy::tokenizer::{LowerCaser, RemoveLongFilter, SimpleTokenizer, TextAnalyzer};

pub(crate) const TOKENIZER_NAME: &str = "yoink_stats_simple_v1";

pub(crate) fn analyzer() -> TextAnalyzer {
    TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(LowerCaser)
        .filter(RemoveLongFilter::limit(2048))
        .build()
}

pub fn tokenize(text: &str) -> Vec<String> {
    let mut analyzer = analyzer();
    let mut stream = analyzer.token_stream(text);
    let mut terms = Vec::new();
    while stream.advance() {
        terms.push(stream.token().text.clone());
    }
    terms
}
