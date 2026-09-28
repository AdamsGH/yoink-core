mod common;

use common::{fixture, insert, request};
use serde_json::Value;
use std::fs;
use yoink_stats_search::{RefreshMode, SearchService};

#[tokio::test]
async fn absent_commit_payload_causes_public_search_to_rebuild() {
    let f = fixture().await;
    insert(&f.store, 1, -61, 1, Some("payload recovery"), "text").await;
    assert_eq!(
        f.search
            .search(request(-61, "payload"))
            .await
            .unwrap()
            .total,
        1
    );
    let index = f.directory.path().join("index");
    drop(f.search);
    let meta_path = index.join("meta.json");
    let mut metadata: Value = serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    *payload_mut(&mut metadata) = Value::Null;
    fs::write(&meta_path, serde_json::to_vec(&metadata).unwrap()).unwrap();

    let reopened = SearchService::open(f.store.database().clone(), &index)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .search(request(-61, "payload"))
            .await
            .unwrap()
            .total,
        1
    );
    assert_eq!(
        reopened.refresh().await.unwrap().mode,
        RefreshMode::Unchanged
    );
    assert_binding(&meta_path, 1);
}

#[tokio::test]
async fn unsupported_commit_payload_version_is_replaced_without_pending_changes() {
    let f = fixture().await;
    insert(&f.store, 1, -62, 1, Some("obsolete searchable"), "text").await;
    assert_eq!(
        f.search
            .search(request(-62, "obsolete"))
            .await
            .unwrap()
            .total,
        1
    );
    let index = f.directory.path().join("index");
    drop(f.search);

    let meta_path = index.join("meta.json");
    let mut metadata: Value = serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    let encoded = payload_mut(&mut metadata)
        .as_str()
        .expect("serialized commit binding")
        .to_owned();
    let mut binding: Value = serde_json::from_str(&encoded).unwrap();
    binding["version"] = Value::from(99);
    *payload_mut(&mut metadata) = Value::String(serde_json::to_string(&binding).unwrap());
    fs::write(&meta_path, serde_json::to_vec(&metadata).unwrap()).unwrap();

    let reopened = SearchService::open(f.store.database().clone(), &index)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .search(request(-62, "obsolete"))
            .await
            .unwrap()
            .total,
        1
    );
    assert_binding(&meta_path, 1);
}

fn assert_binding(meta_path: &std::path::Path, revision: i64) {
    let metadata: Value = serde_json::from_slice(&fs::read(meta_path).unwrap()).unwrap();
    let binding: Value = serde_json::from_str(metadata["payload"].as_str().unwrap()).unwrap();
    assert_eq!(binding["version"], 1);
    assert_eq!(binding["revision"], revision);
}

fn payload_mut(metadata: &mut Value) -> &mut Value {
    metadata
        .as_object_mut()
        .and_then(|object| object.get_mut("payload"))
        .expect("Tantivy metadata must expose its commit payload")
}
