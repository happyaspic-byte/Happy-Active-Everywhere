use everywhere::storage::{Manifest, Receiver, restore};
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;

fn archived_version(root: &Path) -> std::path::PathBuf {
    fs::read_dir(root.join(".everywhere-versions"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
}

#[test]
fn corrupted_archived_version_cannot_replace_current_content() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join("document");
    let source = temp.path().join("source");
    fs::write(&target, b"historical content").unwrap();
    fs::write(&source, b"current content").unwrap();
    restore(&source, &target).unwrap();
    let version = archived_version(temp.path());
    fs::write(&version, b"corrupted history").unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_everywhere"))
        .arg("restore")
        .arg("--version-file")
        .arg(&version)
        .arg("--output")
        .arg(&target)
        .output()
        .unwrap();
    assert!(!result.status.success(), "corrupt history was accepted");
    assert_eq!(fs::read(&target).unwrap(), b"current content");
    assert_eq!(fs::read(&version).unwrap(), b"corrupted history");
}

#[test]
fn quarantined_history_requires_explicit_recovery_outside_the_archive() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join("document");
    let source = temp.path().join("source");
    fs::write(&target, b"historical content").unwrap();
    fs::write(&source, b"current content").unwrap();
    restore(&source, &target).unwrap();
    let version = archived_version(temp.path());
    let quarantine = version.with_extension("corrupt-0");
    fs::rename(&version, &quarantine).unwrap();
    assert!(restore(&quarantine, &target).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"current content");
}

#[test]
fn relative_archive_paths_cannot_bypass_quarantine_rejection() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join("document");
    let source = temp.path().join("source");
    fs::write(&target, b"historical content").unwrap();
    fs::write(&source, b"current content").unwrap();
    restore(&source, &target).unwrap();
    let version = archived_version(temp.path());
    let quarantine = version.with_extension("corrupt-0");
    fs::rename(&version, &quarantine).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_everywhere"))
        .current_dir(quarantine.parent().unwrap())
        .arg("restore")
        .arg("--version-file")
        .arg(quarantine.file_name().unwrap())
        .arg("--output")
        .arg(&target)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(fs::read(&target).unwrap(), b"current content");
}

#[cfg(unix)]
#[test]
fn archive_directory_alias_cannot_bypass_quarantine_rejection() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join("document");
    let source = temp.path().join("source");
    fs::write(&target, b"historical content").unwrap();
    fs::write(&source, b"current content").unwrap();
    restore(&source, &target).unwrap();
    let version = archived_version(temp.path());
    let quarantine = version.with_extension("corrupt-0");
    fs::rename(&version, &quarantine).unwrap();
    let alias = temp.path().join("archive-shortcut");
    std::os::unix::fs::symlink(quarantine.parent().unwrap(), &alias).unwrap();
    assert!(restore(&alias.join(quarantine.file_name().unwrap()), &target).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"current content");
}

#[test]
fn intact_archived_version_restores_and_preserves_the_replaced_content() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join("document");
    let source = temp.path().join("source");
    fs::write(&target, b"historical content").unwrap();
    fs::write(&source, b"current content").unwrap();
    restore(&source, &target).unwrap();
    restore(&archived_version(temp.path()), &target).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"historical content");
    assert!(
        fs::read_dir(temp.path().join(".everywhere-versions"))
            .unwrap()
            .any(|p| fs::read(p.unwrap().path()).unwrap() == b"current content")
    );
}

#[test]
fn case_aliases_cannot_acquire_concurrent_receivers_even_before_creation() {
    for exists in [false, true] {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::write(&source, b"incoming").unwrap();
        let target = temp.path().join("Report.txt");
        fs::write(&target, b"original").unwrap();
        let alias = temp.path().join("report.txt");
        let same_file = alias.exists();
        if !exists {
            fs::remove_file(&target).unwrap();
        }
        let manifest = Manifest::from_path(&source).unwrap();
        let first = Receiver::open(&target, manifest.clone()).unwrap();
        assert!(
            Receiver::open(&alias, manifest.clone()).is_err() == same_file,
            "case alias acquired the same destination while exists={exists}"
        );
        // A separate filename must still be usable while the first is locked.
        let independent = Receiver::open(&temp.path().join("other.txt"), manifest).unwrap();
        drop(independent);
        drop(first);
    }
}

#[test]
fn unicode_normalization_aliases_cannot_acquire_concurrent_receivers() {
    for (left, right) in [
        ("caf\u{e9}.txt", "cafe\u{301}.txt"),
        ("ß.txt", "ẞ.txt"),
        ("ΐ.txt", "Ϊ\u{301}.txt"),
    ] {
        for exists in [false, true] {
            let temp = TempDir::new().unwrap();
            let source = temp.path().join("source");
            fs::write(&source, b"incoming").unwrap();
            let target = temp.path().join(left);
            let alias = temp.path().join(right);
            fs::write(&target, b"original").unwrap();
            let same_file = alias.exists();
            if !exists {
                fs::remove_file(&target).unwrap();
            }
            let manifest = Manifest::from_path(&source).unwrap();
            let first = Receiver::open(&target, manifest.clone()).unwrap();
            assert_eq!(
                Receiver::open(&alias, manifest).is_err(),
                same_file,
                "{left}/{right}, exists={exists}"
            );
            drop(first);
        }
    }
}

#[cfg(unix)]
#[test]
fn archival_copies_are_private_even_with_a_permissive_umask() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    let target = temp.path().join("private-document");
    let source = temp.path().join("source");
    fs::write(&target, b"private history").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&source, b"replacement").unwrap();
    // The shell changes only the child umask, never parallel test processes.
    let result = Command::new("sh")
        .args(["-c", "umask 022; exec \"$@\"", "archive-permissions"])
        .arg(env!("CARGO_BIN_EXE_everywhere"))
        .arg("restore")
        .arg("--version-file")
        .arg(&source)
        .arg("--output")
        .arg(&target)
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result.stderr);
    let version = archived_version(temp.path());
    assert_eq!(fs::read(&version).unwrap(), b"private history");
    assert_eq!(
        fs::metadata(&version).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(version.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}
