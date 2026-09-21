use everywhere::storage::{BLOCK_SIZE, Manifest, Receiver};
use std::fs;
use tempfile::TempDir;

#[test]
fn verified_resume_repairs_corruption_and_preserves_previous_version() {
    let d = TempDir::new().unwrap();
    let source = d.path().join("source");
    let target = d.path().join("target");
    let mut data = vec![7; BLOCK_SIZE * 2 + 13];
    data[BLOCK_SIZE..].fill(9);
    fs::write(&source, &data).unwrap();
    fs::write(&target, b"previous").unwrap();
    let manifest = Manifest::from_path(&source).unwrap();
    let mut r = Receiver::open(&target, manifest.clone()).unwrap();
    assert_eq!(r.missing().unwrap(), vec![0, 1, 2]);
    assert!(r.put(0, b"bad").is_err());
    assert_eq!(fs::read(&target).unwrap(), b"previous");
    r.put(0, &data[..BLOCK_SIZE]).unwrap();
    drop(r);
    let mut r = Receiver::open(&target, manifest.clone()).unwrap();
    assert_eq!(r.missing().unwrap(), vec![1, 2]);
    r.put(1, &data[BLOCK_SIZE..2 * BLOCK_SIZE]).unwrap();
    r.put(2, &data[2 * BLOCK_SIZE..]).unwrap();
    r.finish().unwrap();
    assert_eq!(fs::read(&target).unwrap(), data);
    let versions = fs::read_dir(d.path().join(".everywhere-versions")).unwrap();
    assert!(
        versions
            .map(|e| fs::read(e.unwrap().path()).unwrap())
            .any(|b| b == b"previous")
    );
}
#[test]
fn changed_blocks_reused_local_edits_protected_and_lock_exclusive() {
    let d = TempDir::new().unwrap();
    let source = d.path().join("source");
    let target = d.path().join("target");
    let data = vec![1; BLOCK_SIZE * 2];
    fs::write(&source, &data).unwrap();
    let mut old = data.clone();
    old[BLOCK_SIZE..].fill(2);
    fs::write(&target, &old).unwrap();
    let manifest = Manifest::from_path(&source).unwrap();
    let mut r = Receiver::open(&target, manifest.clone()).unwrap();
    assert!(Receiver::open(&target, manifest).is_err());
    assert_eq!(r.missing().unwrap(), vec![1]);
    r.put(1, &data[BLOCK_SIZE..]).unwrap();
    fs::write(&target, b"local edit").unwrap();
    assert!(r.finish().is_err());
    assert_eq!(fs::read(&target).unwrap(), b"local edit");
}
#[test]
fn malformed_manifest_and_empty_file() {
    let d = TempDir::new().unwrap();
    let source = d.path().join("empty");
    fs::write(&source, []).unwrap();
    let mut manifest = Manifest::from_path(&source).unwrap();
    let target = d.path().join("target");
    let mut r = Receiver::open(&target, manifest.clone()).unwrap();
    assert!(r.missing().unwrap().is_empty());
    r.finish().unwrap();
    assert_eq!(fs::metadata(&target).unwrap().len(), 0);
    manifest.size = 1;
    assert!(manifest.validate().is_err());
    manifest.size = u64::MAX;
    assert!(manifest.validate().is_err());
}
#[cfg(unix)]
#[test]
fn symlink_target_is_rejected() {
    use std::os::unix::fs::symlink;
    let d = TempDir::new().unwrap();
    let source = d.path().join("source");
    let target = d.path().join("target");
    fs::write(&source, b"safe").unwrap();
    symlink(&source, &target).unwrap();
    assert!(Receiver::open(&target, Manifest::from_path(&source).unwrap()).is_err());
    assert_eq!(fs::read(source).unwrap(), b"safe");
}

#[test]
fn local_change_between_processes_survives_resume() {
    let d = TempDir::new().unwrap();
    let source = d.path().join("source");
    let target = d.path().join("target");
    fs::write(&source, b"incoming").unwrap();
    fs::write(&target, b"original").unwrap();
    let manifest = Manifest::from_path(&source).unwrap();
    let mut receiver = Receiver::open(&target, manifest.clone()).unwrap();
    receiver.put(0, b"incoming").unwrap();
    drop(receiver);
    fs::write(&target, b"local edit while offline").unwrap();
    let resumed = Receiver::open(&target, manifest);
    match resumed {
        Err(_) => {}
        Ok(mut receiver) => assert!(receiver.finish().is_err()),
    }
    assert_eq!(fs::read(&target).unwrap(), b"local edit while offline");
}

#[test]
fn corrupt_partial_block_is_requested_again() {
    let d = TempDir::new().unwrap();
    let source = d.path().join("source");
    let target = d.path().join("target");
    let data = vec![7; BLOCK_SIZE + 3];
    fs::write(&source, &data).unwrap();
    let manifest = Manifest::from_path(&source).unwrap();
    let mut r = Receiver::open(&target, manifest.clone()).unwrap();
    r.put(0, &data[..BLOCK_SIZE]).unwrap();
    drop(r);
    let partial = fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "part"))
        .unwrap();
    fs::write(partial, vec![8; BLOCK_SIZE]).unwrap();
    let mut r = Receiver::open(&target, manifest).unwrap();
    assert_eq!(r.missing().unwrap(), vec![0, 1]);
    assert!(r.finish().is_err());
    assert!(!target.exists());
}

#[test]
fn corrupt_journal_and_unwritable_version_location_preserve_original() {
    let d = TempDir::new().unwrap();
    let source = d.path().join("source");
    let target = d.path().join("target");
    fs::write(&source, b"new").unwrap();
    fs::write(&target, b"old").unwrap();
    let manifest = Manifest::from_path(&source).unwrap();
    let mut r = Receiver::open(&target, manifest.clone()).unwrap();
    r.put(0, b"new").unwrap();
    fs::write(d.path().join(".everywhere-versions"), b"blocked").unwrap();
    assert!(r.finish().is_err());
    assert_eq!(fs::read(&target).unwrap(), b"old");
    drop(r);
    let journal = fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    fs::write(journal, b"{broken").unwrap();
    assert!(Receiver::open(&target, manifest).is_err());
    assert_eq!(fs::read(target).unwrap(), b"old");
}

#[test]
fn partial_version_copy_does_not_permanently_block_recovery() {
    let d = TempDir::new().unwrap();
    let source = d.path().join("source");
    let target = d.path().join("target");
    fs::write(&source, b"new").unwrap();
    fs::write(&target, b"old version").unwrap();
    let manifest = Manifest::from_path(&source).unwrap();
    let mut r = Receiver::open(&target, manifest).unwrap();
    r.put(0, b"new").unwrap();
    let versions = d.path().join(".everywhere-versions");
    fs::create_dir(&versions).unwrap();
    let name = blake3::hash(b"target").to_hex();
    let hash = blake3::hash(b"old version").to_hex();
    fs::write(versions.join(format!("{name}-{hash}")), b"partial").unwrap();
    // A corrupt preserved version is quarantined; the intact original remains available to recopy.
    r.finish().unwrap();
    assert_eq!(fs::read(target).unwrap(), b"new");
    assert_eq!(
        fs::read(versions.join(format!("{name}-{hash}"))).unwrap(),
        b"old version"
    );
}
