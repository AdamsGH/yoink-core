use std::ops::Bound;

use chrono::NaiveDate;
use tantivy::{
    Order, TantivyDocument, Term,
    collector::{Count, TopDocs, sort_key::SortByStaticFastValue},
    query::{BooleanQuery, Occur, Query, RangeQuery, TermQuery},
    schema::{IndexRecordOption, Value},
};

use crate::{SearchRequest, tokenizer::tokenize};

use super::{NativeIndex, index_error};

pub(super) fn search(
    index: &NativeIndex,
    request: &SearchRequest,
) -> Result<(u64, Vec<i64>), String> {
    let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![(
        Occur::Must,
        Box::new(TermQuery::new(
            Term::from_field_i64(index.fields.chat, request.chat_id),
            IndexRecordOption::Basic,
        )),
    )];
    if let Some(user) = request.from_user {
        clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_field_i64(index.fields.user, user),
                IndexRecordOption::Basic,
            )),
        ));
    }
    if let Some(kind) = request.msg_type.as_deref() {
        clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_field_text(index.fields.kind, kind),
                IndexRecordOption::Basic,
            )),
        ));
    }
    if let Some(from) = request.from_date {
        clauses.push((
            Occur::Must,
            Box::new(RangeQuery::new(
                Bound::Included(Term::from_field_i64(index.fields.date, day_start(from))),
                Bound::Unbounded,
            )),
        ));
    }
    if let Some(to) = request.to_date {
        clauses.push((
            Occur::Must,
            Box::new(RangeQuery::new(
                Bound::Unbounded,
                Bound::Included(Term::from_field_i64(index.fields.date, day_end(to))),
            )),
        ));
    }
    if let Some(text) = request.query.as_deref().filter(|q| !q.is_empty()) {
        let mut terms = tokenize(text);
        terms.sort_unstable();
        terms.dedup();
        if terms.is_empty() {
            return Ok((0, Vec::new()));
        }
        for term in terms {
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(index.fields.body, &term),
                    IndexRecordOption::Basic,
                )),
            ));
        }
    }
    let query = BooleanQuery::new(clauses);
    let searcher = index.reader.searcher();
    if request.limit == 0 {
        return Ok((
            searcher.search(&query, &Count).map_err(index_error)? as u64,
            Vec::new(),
        ));
    }
    let page = TopDocs::with_limit(request.limit as usize)
        .and_offset(request.offset as usize)
        .order_by((
            (SortByStaticFastValue::<i64>::for_field("date"), Order::Desc),
            (SortByStaticFastValue::<i64>::for_field("id"), Order::Desc),
        ));
    let (total, docs) = searcher
        .search(&query, &(Count, page))
        .map_err(index_error)?;
    let ids = docs
        .into_iter()
        .map(|(_, address)| {
            let doc: TantivyDocument = searcher.doc(address).map_err(index_error)?;
            doc.get_first(index.fields.id)
                .and_then(|v| v.as_i64())
                .ok_or_else(|| "corrupt search document: missing indexed ID".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((total as u64, ids))
}

fn day_start(date: NaiveDate) -> i64 {
    date.and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_micros()
}

fn day_end(date: NaiveDate) -> i64 {
    date.and_hms_micro_opt(23, 59, 59, 999_999)
        .unwrap()
        .and_utc()
        .timestamp_micros()
}
