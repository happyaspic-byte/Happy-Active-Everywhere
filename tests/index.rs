use everywhere::index::{Change, Index};
use std::fs;
use tempfile::TempDir;
#[test]
fn scan_persists_edits_and_tombstones_without_wall_clock() {
    let d = TempDir::new().unwrap();
    let root = d.path().join("folder");
    fs::create_dir(&root).unwrap();
    let db = d.path().join("index.sqlite");
    let mut index = Index::create(&db, &root, "device-a").unwrap();
    fs::write(root.join("one"), b"first").unwrap();
    let first = index.scan(false).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].change, Change::Created);
    let clock = index.entries().unwrap()[0].clock.clone();
    drop(index);
    let mut index = Index::open(&db, &root, "device-a").unwrap();
    assert!(index.scan(false).unwrap().is_empty());
    fs::write(root.join("one"), b"second").unwrap();
    assert_eq!(index.scan(false).unwrap()[0].change, Change::Modified);
    assert_eq!(
        index.entries().unwrap()[0].clock.relation(&clock),
        everywhere::versions::Relation::After
    );
    fs::remove_file(root.join("one")).unwrap();
    assert!(index.scan(false).is_err());
    assert!(index.entries().unwrap()[0].hash.is_some());
    assert_eq!(index.scan(true).unwrap()[0].change, Change::Deleted);
    assert!(index.entries().unwrap()[0].hash.is_none());
}
#[test]
fn missing_mount_marker_or_wrong_root_never_generates_deletions() {
    let d = TempDir::new().unwrap();
    let root = d.path().join("folder");
    fs::create_dir(&root).unwrap();
    let db = d.path().join("index.sqlite");
    let mut index = Index::create(&db, &root, "device-a").unwrap();
    fs::write(root.join("one"), b"safe").unwrap();
    index.scan(false).unwrap();
    let moved = d.path().join("detached");
    fs::rename(&root, &moved).unwrap();
    fs::create_dir(&root).unwrap();
    assert!(index.scan(true).is_err());
    assert!(index.entries().unwrap()[0].hash.is_some());
    assert!(Index::open(&db, &moved, "device-a").is_err());
}
#[test]
fn corrupt_database_fails_closed_and_preserves_files() {
    let d = TempDir::new().unwrap();
    let root = d.path().join("folder");
    fs::create_dir(&root).unwrap();
    let db = d.path().join("index.sqlite");
    drop(Index::create(&db, &root, "device-a").unwrap());
    fs::write(root.join("one"), b"safe").unwrap();
    fs::write(&db, b"broken database").unwrap();
    assert!(Index::open(&db, &root, "device-a").is_err());
    assert_eq!(fs::read(root.join("one")).unwrap(), b"safe");
}

#[cfg(unix)]
#[test]
fn ambiguous_backslash_filename_does_not_alias_nested_path() {
    let d = TempDir::new().unwrap();
    let root = d.path().join("folder");
    fs::create_dir(&root).unwrap();
    let mut index = Index::create(&d.path().join("index.sqlite"), &root, "a").unwrap();
    fs::create_dir(root.join("a")).unwrap();
    fs::write(root.join("a/b"), b"nested").unwrap();
    fs::write(root.join("a\\b"), b"literal").unwrap();
    assert!(index.scan(false).is_err());
    assert!(index.entries().unwrap().is_empty());
}

#[test]
fn pending_deletion_does_not_block_unrelated_creates_or_edits() {
    let d = TempDir::new().unwrap();
    let root = d.path().join("folder");
    fs::create_dir(&root).unwrap();
    let db = d.path().join("index.sqlite");
    let mut index = Index::create(&db, &root, "a").unwrap();
    fs::write(root.join("deleted"), b"keep deletion pending").unwrap();
    fs::write(root.join("edited"), b"before").unwrap();
    index.scan(false).unwrap();
    fs::remove_file(root.join("deleted")).unwrap();
    fs::write(root.join("edited"), b"after").unwrap();
    fs::write(root.join("created"), b"new").unwrap();
    let events = index.scan(false).unwrap();
    assert!(events.iter().any(|e| e.path == "created" && e.change == Change::Created));
    assert!(events.iter().any(|e| e.path == "edited" && e.change == Change::Modified));
    drop(index);
    let index = Index::open(&db, &root, "a").unwrap();
    let entries = index.entries().unwrap();
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().find(|e| e.path == "deleted").unwrap().hash.is_some());
}
