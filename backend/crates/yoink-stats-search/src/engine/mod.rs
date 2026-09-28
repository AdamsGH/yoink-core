mod filesystem;
mod query;

use std::path::Path;

use tantivy::{
    Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term,
    schema::{
        FAST, Field, INDEXED, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing,
        TextOptions,
    },
    tokenizer::TokenizerManager,
};

use crate::{
    SearchRequest, StatsMessage,
    tokenizer::{TOKENIZER_NAME, analyzer},
};

pub(crate) use binding::CommitBinding;
mod binding;

pub(crate) struct NativeIndex {
    index: Index,
    writer: IndexWriter,
    reader: IndexReader,
    fields: Fields,
}

#[derive(Clone, Copy)]
struct Fields {
    id: Field,
    chat: Field,
    user: Field,
    date: Field,
    kind: Field,
    body: Field,
}

fn schema() -> (Schema, Fields) {
    let mut builder = Schema::builder();
    let id = builder.add_i64_field("id", INDEXED | FAST | STORED);
    let chat = builder.add_i64_field("chat_id", INDEXED | FAST | STORED);
    let user = builder.add_i64_field("from_user", INDEXED | FAST | STORED);
    let date = builder.add_i64_field("date", INDEXED | FAST | STORED);
    let kind = builder.add_text_field("msg_type", STRING);
    let body_options = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(TOKENIZER_NAME)
            .set_index_option(IndexRecordOption::Basic),
    );
    let body = builder.add_text_field("body", body_options);
    (
        builder.build(),
        Fields {
            id,
            chat,
            user,
            date,
            kind,
            body,
        },
    )
}

impl NativeIndex {
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        filesystem::secure_directory(path)?;
        let meta = path.join("meta.json");
        let exists = meta.exists();
        if !exists
            && std::fs::read_dir(path)
                .map_err(index_error)?
                .next()
                .is_some()
        {
            return Err("index directory is not empty and has no Tantivy index; use a fresh private directory".into());
        }
        let (expected, fields) = schema();
        let mut index = if exists {
            let index = Index::open_in_dir(path).map_err(index_error)?;
            if index.schema() != expected {
                return Err(
                    "Tantivy index schema mismatch; use a fresh private directory and rebuild"
                        .into(),
                );
            }
            index
        } else {
            Index::create_in_dir(path, expected).map_err(index_error)?
        };
        let tokenizers = TokenizerManager::default();
        tokenizers.register(TOKENIZER_NAME, analyzer());
        index.set_tokenizers(tokenizers);
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(index_error)?;
        let writer = index.writer(30_000_000).map_err(index_error)?;
        Ok(Self {
            index,
            writer,
            reader,
            fields,
        })
    }

    pub(crate) fn reset(&mut self) -> Result<(), String> {
        self.writer.rollback().map_err(index_error)?;
        self.writer.delete_all_documents().map_err(index_error)?;
        Ok(())
    }

    pub(crate) fn update(&mut self, id: i64, message: Option<&StatsMessage>) -> Result<(), String> {
        self.writer
            .delete_term(Term::from_field_i64(self.fields.id, id));
        if let Some(message) = message {
            let mut doc = TantivyDocument::default();
            doc.add_i64(self.fields.id, id);
            doc.add_i64(self.fields.chat, message.chat_id);
            if let Some(user) = message.from_user {
                doc.add_i64(self.fields.user, user);
            }
            doc.add_i64(self.fields.date, message.date);
            doc.add_text(self.fields.kind, &message.msg_type);
            doc.add_text(
                self.fields.body,
                format!(
                    "{} {}",
                    message.text.as_deref().unwrap_or_default(),
                    message.caption.as_deref().unwrap_or_default()
                ),
            );
            self.writer.add_document(doc).map_err(index_error)?;
        }
        Ok(())
    }

    pub(crate) fn commit(&mut self, binding: &CommitBinding) -> Result<(), String> {
        let payload = serde_json::to_string(binding).map_err(index_error)?;
        let mut prepared = self.writer.prepare_commit().map_err(index_error)?;
        prepared.set_payload(&payload);
        prepared.commit().map_err(index_error)?;
        self.reader.reload().map_err(index_error)
    }

    pub(crate) fn binding(&self) -> Result<Option<CommitBinding>, String> {
        let meta = self.index.load_metas().map_err(index_error)?;
        match meta.payload {
            Some(payload) => Ok(serde_json::from_str(&payload).ok()),
            None => Ok(None),
        }
    }

    pub(crate) fn search(&self, request: &SearchRequest) -> Result<(u64, Vec<i64>), String> {
        query::search(self, request)
    }
}

fn index_error(error: impl std::fmt::Display) -> String {
    format!(
        "Tantivy index error: {error}. Use a fresh private index directory and rebuild if corruption persists."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_and_body_tokenizer_are_stable() {
        let (s, f) = schema();
        assert!(s.get_field("id").is_ok());
        assert!(s.get_field("body").is_ok());
        assert_eq!(
            crate::tokenizer::tokenize("Hello WORLD"),
            vec!["hello".to_owned(), "world".to_owned()]
        );
        assert_eq!(f.id, s.get_field("id").unwrap());
    }
}
