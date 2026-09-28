mod common;

use common::{fixture, insert, request};
use std::fs;
use yoink_stats_search::SearchService;

#[cfg(unix)]
#[tokio::test]
async fn rejects_unsafe_index_entries_and_preserves_unrelated_files() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let f = fixture().await;
    let base = f.directory.path();
    let marker = base.join("keep.txt");
    fs::write(&marker, "untouched").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(base.join("index"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    let symlink_dir = base.join("symlink-index");
    fs::create_dir(&symlink_dir).unwrap();
    fs::set_permissions(&symlink_dir, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&marker, symlink_dir.join("linked")).unwrap();
    let error = SearchService::open(f.store.database().clone(), &symlink_dir)
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("contains a symlink"), "{error}");
    assert_eq!(fs::read_to_string(&marker).unwrap(), "untouched");

    let hardlink_dir = base.join("hardlink-index");
    fs::create_dir(&hardlink_dir).unwrap();
    fs::set_permissions(&hardlink_dir, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(hardlink_dir.join("meta.json"), "{}").unwrap();
    let external = base.join("external");
    fs::write(&external, "keep").unwrap();
    fs::hard_link(&external, hardlink_dir.join("linked-file")).unwrap();
    let error = SearchService::open(f.store.database().clone(), &hardlink_dir)
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("hard-linked"), "{error}");
    assert_eq!(fs::read_to_string(&external).unwrap(), "keep");

    let unrelated = base.join("unrelated-index");
    fs::create_dir(&unrelated).unwrap();
    fs::set_permissions(&unrelated, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(unrelated.join("do-not-touch"), "sentinel").unwrap();
    let error = SearchService::open(f.store.database().clone(), &unrelated)
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("not empty"), "{error}");
    assert_eq!(
        fs::read_to_string(unrelated.join("do-not-touch")).unwrap(),
        "sentinel"
    );

    let public_parent = base.join("public-parent");
    fs::create_dir(&public_parent).unwrap();
    fs::set_permissions(&public_parent, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        SearchService::open(f.store.database().clone(), public_parent.join("index"))
            .await
            .is_err()
    );
    assert!(!public_parent.join("index").exists());

    let public_index = base.join("public-index");
    fs::create_dir(&public_index).unwrap();
    fs::set_permissions(&public_index, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        SearchService::open(f.store.database().clone(), &public_index)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn corrupt_metadata_and_a_second_writer_fail_without_touching_other_files() {
    let f = fixture().await;
    insert(&f.store, 1, -55, 0, Some("retained"), "text").await;
    let index = f.directory.path().join("index");
    let marker = f.directory.path().join("marker");
    fs::write(&marker, "untouched").unwrap();
    assert!(
        SearchService::open(f.store.database().clone(), &index)
            .await
            .is_err()
    );
    assert_eq!(fs::read_to_string(&marker).unwrap(), "untouched");

    drop(f.search);
    let metadata = index.join("meta.json");
    let original = fs::read(&metadata).unwrap();
    fs::write(&metadata, b"corrupt metadata").unwrap();
    assert!(
        SearchService::open(f.store.database().clone(), &index)
            .await
            .is_err()
    );
    assert_eq!(fs::read_to_string(&marker).unwrap(), "untouched");
    fs::write(&metadata, original).unwrap();

    let first = SearchService::open(f.store.database().clone(), &index)
        .await
        .unwrap();
    assert_eq!(
        first
            .search(request(-55, "not-present"))
            .await
            .unwrap()
            .total,
        0
    );
    let second = SearchService::open(f.store.database().clone(), &index).await;
    assert!(
        second.is_err(),
        "a concurrent writer must not acquire the same index"
    );
}
