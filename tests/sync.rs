use everywhere::identity;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}
fn run(args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_everywhere"))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Hosted Windows needs time for hundreds of durable creates/deletions.
    // Keep a bounded test deadline longer than the protocol's progress timeout.
    let deadline = Instant::now() + Duration::from_secs(90);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let result = child.wait_with_output().unwrap();
            panic!(
                "CLI timed out: {args:?}\n{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}
fn ok(args: &[&str]) -> String {
    let output = run(args);
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
struct Node {
    state: PathBuf,
    root: PathBuf,
    id: String,
}
impl Node {
    fn new(base: &Path, name: &str) -> Self {
        let state = base.join(format!("{name}-state"));
        let root = base.join(format!("{name}-files"));
        let id = identity::init(&state).unwrap();
        fs::create_dir(&root).unwrap();
        ok(&[
            "share-init",
            "--state",
            s(&state),
            "--folder",
            "personal",
            "--root",
            s(&root),
        ]);
        Self { state, root, id }
    }
    fn allow(&self, other: &Self) {
        identity::trust(&self.state, &other.state.join("identity.der")).unwrap();
        ok(&[
            "share-peer",
            "--state",
            s(&self.state),
            "--folder",
            "personal",
            "--peer",
            &other.id,
        ]);
    }
    fn command(&self, command: &str, extra: &[&str]) -> String {
        let mut args = vec![command, "--state", s(&self.state), "--folder", "personal"];
        args.extend_from_slice(extra);
        ok(&args)
    }
    fn status(&self) -> Value {
        serde_json::from_str(&self.command("share-status", &[])).unwrap()
    }
}
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn sync(a: &Node, b: &Node) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_everywhere"))
        .args([
            "sync-serve",
            "--state",
            s(&b.state),
            "--folder",
            "personal",
            "--peer",
            &a.id,
            "--listen",
            "127.0.0.1:0",
            "--once",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let mut guard = Server(child);
    let line = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("sync server not ready");
    let address = line
        .trim()
        .strip_prefix("LISTEN ")
        .expect("missing listener address");
    let output = a.command("sync", &["--peer", &b.id, "--addr", address]);
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap()["status"],
        "complete"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(result) = guard.0.try_wait().unwrap() {
            assert!(result.success());
            break;
        }
        assert!(Instant::now() < deadline, "server did not finish");
        thread::sleep(Duration::from_millis(10));
    }
    reader.join().unwrap();
}
fn pair(base: &Path) -> (Node, Node) {
    let a = Node::new(base, "a");
    let b = Node::new(base, "b");
    a.allow(&b);
    b.allow(&a);
    (a, b)
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn head_state(node: &Node, path: &str) -> Value {
    node.status()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == path)
        .unwrap()["versions"]
        .clone()
}

#[test]
fn independent_devices_roundtrip_unicode_empty_files_and_directories() {
    let tmp = TempDir::new().unwrap();
    let (a, b) = pair(tmp.path());
    fs::create_dir(a.root.join("한글 문서")).unwrap();
    fs::create_dir(a.root.join("빈 폴더")).unwrap();
    fs::write(a.root.join("한글 문서/😀.txt"), b"first").unwrap();
    fs::write(a.root.join("empty"), []).unwrap();
    sync(&a, &b);
    assert_eq!(
        sha(&fs::read(b.root.join("한글 문서/😀.txt")).unwrap()),
        sha(b"first")
    );
    assert!(b.root.join("빈 폴더").is_dir());
    assert_eq!(fs::metadata(b.root.join("empty")).unwrap().len(), 0);
    fs::write(b.root.join("한글 문서/😀.txt"), b"edited on b").unwrap();
    sync(&a, &b);
    assert_eq!(
        sha(&fs::read(a.root.join("한글 문서/😀.txt")).unwrap()),
        sha(b"edited on b")
    );
    let before = head_state(&a, "한글 문서/😀.txt");
    sync(&a, &b);
    assert_eq!(head_state(&a, "한글 문서/😀.txt"), before);
    assert_eq!(head_state(&b, "한글 문서/😀.txt"), before);
}

#[test]
fn offline_concurrent_edits_preserve_both_payloads_without_conflict_growth() {
    let tmp = TempDir::new().unwrap();
    let (a, b) = pair(tmp.path());
    fs::write(a.root.join("note"), b"initial").unwrap();
    sync(&a, &b);
    fs::write(a.root.join("note"), b"edited on a").unwrap();
    fs::write(b.root.join("note"), b"edited on b").unwrap();
    sync(&a, &b);
    for node in [&a, &b] {
        let conflicts: Value = serde_json::from_str(&node.command("share-conflicts", &[])).unwrap();
        let versions = conflicts[0]["revisions"].as_array().unwrap();
        let mut contents: Vec<_> = versions
            .iter()
            .map(|v| sha(&fs::read(v["object_path"].as_str().unwrap()).unwrap()))
            .collect();
        contents.sort();
        let mut expected = vec![sha(b"edited on a"), sha(b"edited on b")];
        expected.sort();
        assert_eq!(contents, expected);
    }
    let versions = head_state(&a, "note");
    sync(&b, &a);
    sync(&a, &b);
    assert_eq!(head_state(&a, "note"), versions);
    assert_eq!(head_state(&b, "note"), versions);
}

#[test]
fn deletion_requires_persistent_approval_before_network_propagation() {
    let tmp = TempDir::new().unwrap();
    let (a, b) = pair(tmp.path());
    fs::write(a.root.join("note"), b"keep until approved").unwrap();
    sync(&a, &b);
    fs::remove_file(a.root.join("note")).unwrap();
    a.command("share-scan", &[]);
    sync(&a, &b);
    assert_eq!(
        fs::read(b.root.join("note")).unwrap(),
        b"keep until approved"
    );
    assert_eq!(a.status()["pending_deletions"][0], "note");
    a.command("share-approve-deletes", &["--all"]);
    sync(&a, &b);
    assert!(!a.root.join("note").exists());
    assert!(!b.root.join("note").exists());
    let deleted = head_state(&a, "note");
    sync(&a, &b);
    assert_eq!(head_state(&b, "note"), deleted);
}

#[test]
fn three_devices_converge_after_different_orders_of_offline_edits() {
    let tmp = TempDir::new().unwrap();
    let a = Node::new(tmp.path(), "a");
    let b = Node::new(tmp.path(), "b");
    let c = Node::new(tmp.path(), "c");
    for (left, right) in [(&a, &b), (&a, &c), (&b, &a), (&b, &c), (&c, &a), (&c, &b)] {
        left.allow(right);
    }
    fs::write(a.root.join("note"), b"initial").unwrap();
    sync(&a, &b);
    sync(&a, &c);
    for (node, bytes) in [(&a, b"edit a"), (&b, b"edit b"), (&c, b"edit c")] {
        fs::write(node.root.join("note"), bytes).unwrap();
    }
    sync(&a, &b);
    sync(&b, &c);
    sync(&c, &a);
    sync(&a, &b);
    let expected = head_state(&a, "note");
    assert_eq!(expected["heads"].as_array().unwrap().len(), 3);
    assert_eq!(head_state(&b, "note"), expected);
    assert_eq!(head_state(&c, "note"), expected);
    let visible = sha(&fs::read(a.root.join("note")).unwrap());
    assert_eq!(sha(&fs::read(b.root.join("note")).unwrap()), visible);
    assert_eq!(sha(&fs::read(c.root.join("note")).unwrap()), visible);
    for node in [&a, &b, &c] {
        let conflicts: Value = serde_json::from_str(&node.command("share-conflicts", &[])).unwrap();
        let mut got: Vec<_> = conflicts[0]["revisions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| sha(&fs::read(r["object_path"].as_str().unwrap()).unwrap()))
            .collect();
        let mut expected = vec![sha(b"edit a"), sha(b"edit b"), sha(b"edit c")];
        got.sort();
        expected.sort();
        assert_eq!(got, expected);
    }
}

#[test]
fn simultaneous_delete_and_edit_preserve_the_edit_and_the_tombstone() {
    let tmp = TempDir::new().unwrap();
    let (a, b) = pair(tmp.path());
    fs::write(a.root.join("note"), b"initial").unwrap();
    sync(&a, &b);
    fs::remove_file(a.root.join("note")).unwrap();
    a.command("share-approve-deletes", &["--all"]);
    fs::write(b.root.join("note"), b"offline edit").unwrap();
    sync(&a, &b);
    for node in [&a, &b] {
        assert_eq!(fs::read(node.root.join("note")).unwrap(), b"offline edit");
        let heads = head_state(node, "note")["heads"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(heads.len(), 2);
        assert!(heads.iter().any(|h| h["content"]["kind"] == "deleted"));
    }
}

#[test]
fn long_offline_peer_does_not_resurrect_a_deleted_file() {
    let tmp = TempDir::new().unwrap();
    let (a, b) = pair(tmp.path());
    let c = Node::new(tmp.path(), "c");
    a.allow(&c);
    c.allow(&a);
    fs::write(a.root.join("note"), b"initial").unwrap();
    sync(&a, &b);
    sync(&a, &c);
    fs::remove_file(a.root.join("note")).unwrap();
    a.command("share-approve-deletes", &["--all"]);
    sync(&a, &b);
    sync(&c, &a);
    for node in [&a, &b, &c] {
        assert!(!node.root.join("note").exists());
    }
    assert_eq!(head_state(&a, "note"), head_state(&c, "note"));
}

#[test]
fn explicit_conflict_resolution_and_historical_restore_converge() {
    let tmp = TempDir::new().unwrap();
    let (a, b) = pair(tmp.path());
    fs::write(a.root.join("note"), b"original").unwrap();
    sync(&a, &b);
    fs::write(a.root.join("note"), b"edit a").unwrap();
    fs::write(b.root.join("note"), b"edit b").unwrap();
    sync(&a, &b);
    let conflicts: Value = serde_json::from_str(&a.command("share-conflicts", &[])).unwrap();
    let choice = conflicts[0]["revisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| fs::read(r["object_path"].as_str().unwrap()).unwrap() == b"edit b")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    a.command("share-resolve", &["--path", "note", "--revision", &choice]);
    sync(&a, &b);
    for node in [&a, &b] {
        assert_eq!(fs::read(node.root.join("note")).unwrap(), b"edit b");
        assert_eq!(
            head_state(node, "note")["heads"].as_array().unwrap().len(),
            1
        );
    }
    let history: Value =
        serde_json::from_str(&a.command("share-history", &["--path", "note"])).unwrap();
    let original = history
        .as_array()
        .unwrap()
        .iter()
        .find(|r| {
            r["object_path"]
                .as_str()
                .is_some_and(|p| fs::read(p).unwrap() == b"original")
        })
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    a.command(
        "share-restore",
        &["--path", "note", "--revision", &original],
    );
    sync(&a, &b);
    assert_eq!(fs::read(a.root.join("note")).unwrap(), b"original");
    assert_eq!(fs::read(b.root.join("note")).unwrap(), b"original");
    assert_eq!(head_state(&a, "note"), head_state(&b, "note"));
}

#[test]
fn missing_mount_marker_never_creates_deletion_revisions() {
    let tmp = TempDir::new().unwrap();
    let node = Node::new(tmp.path(), "a");
    fs::write(node.root.join("note"), b"keep").unwrap();
    node.command("share-scan", &[]);
    let before = head_state(&node, "note");
    let marker = fs::read(node.root.join(".everywhere-folder")).unwrap();
    fs::remove_file(node.root.join(".everywhere-folder")).unwrap();
    fs::remove_file(node.root.join("note")).unwrap();
    let output = run(&[
        "share-approve-deletes",
        "--state",
        s(&node.state),
        "--folder",
        "personal",
        "--all",
    ]);
    assert!(!output.status.success());
    fs::write(node.root.join(".everywhere-folder"), marker).unwrap();
    assert_eq!(head_state(&node, "note"), before);
}

#[cfg(unix)]
#[test]
fn replaced_root_with_copied_marker_cannot_scan_a_stale_directory_handle() {
    use everywhere::share::Share;
    let tmp = TempDir::new().unwrap();
    let node = Node::new(tmp.path(), "a");
    fs::write(node.root.join("note"), b"keep current root").unwrap();
    let share = Share::open(&node.state, "personal").unwrap();
    share.scan(false).unwrap();
    let before = share.status().unwrap();
    let moved = tmp.path().join("old-root");
    fs::rename(&node.root, &moved).unwrap();
    fs::create_dir(&node.root).unwrap();
    fs::copy(
        moved.join(".everywhere-folder"),
        node.root.join(".everywhere-folder"),
    )
    .unwrap();
    fs::copy(moved.join("note"), node.root.join("note")).unwrap();
    fs::remove_file(moved.join("note")).unwrap();
    assert!(
        share.scan(true).is_err(),
        "stale handle generated a deletion for the current root"
    );
    assert_eq!(share.status().unwrap(), before);
    assert_eq!(
        sha(&fs::read(node.root.join("note")).unwrap()),
        sha(b"keep current root")
    );
    drop(share);
    // A fresh session may bind to the restored root and continue normally.
    node.command("share-scan", &[]);
    assert_eq!(node.status(), before);
}

#[test]
fn malformed_epoch_reports_error_instead_of_panicking() {
    let tmp = TempDir::new().unwrap();
    let node = Node::new(tmp.path(), "a");
    let config_path = node.state.join("shares/personal/config.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    config["epoch"] = Value::String("x".into());
    fs::write(config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    // A matching corrupt DB value must still fail configuration validation.
    let db = rusqlite::Connection::open(node.state.join("shares/personal/index.sqlite")).unwrap();
    db.execute("UPDATE meta SET value='x' WHERE key='epoch'", [])
        .unwrap();
    drop(db);
    fs::write(node.root.join("note"), b"keep").unwrap();
    let result = run(&[
        "share-scan",
        "--state",
        s(&node.state),
        "--folder",
        "personal",
    ]);
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("panicked"));
}

#[test]
fn revocation_after_metadata_staging_prevents_filesystem_application() {
    use everywhere::{
        model::{Content, Versions},
        share::{Record, Share},
    };
    let tmp = TempDir::new().unwrap();
    let (a, b) = pair(tmp.path());
    let share = Share::open(&a.state, "personal").unwrap();
    share.begin_incoming().unwrap();
    share
        .stage(
            &b.id,
            &[Record {
                path: "remote-directory".into(),
                versions: Versions::default().edit(&b.id, Content::Directory).unwrap(),
                seq: 1,
            }],
        )
        .unwrap();
    identity::revoke(&a.state, &b.id).unwrap();
    assert!(share.commit_incoming(&b.id).is_err());
    assert!(!a.root.join("remote-directory").exists());
    drop(share);
    // Removing the folder grant must remain possible after global revocation.
    a.command("share-peer", &["--peer", &b.id, "--remove"]);
}

#[test]
fn paged_metadata_and_nested_deletions_converge_without_missing_paths() {
    let tmp = TempDir::new().unwrap();
    let (a, b) = pair(tmp.path());
    fs::create_dir_all(a.root.join("nested/inside")).unwrap();
    for index in 0..270 {
        fs::write(a.root.join(format!("nested/inside/file-{index:04}")), []).unwrap();
    }
    sync(&a, &b);
    for index in 0..270 {
        assert!(
            b.root
                .join(format!("nested/inside/file-{index:04}"))
                .is_file()
        );
    }
    fs::remove_dir_all(a.root.join("nested")).unwrap();
    a.command("share-approve-deletes", &["--all"]);
    sync(&a, &b);
    assert!(!b.root.join("nested").exists());
}

#[test]
fn folder_scans_remain_usable_after_legacy_file_restoration() {
    let tmp = TempDir::new().unwrap();
    let a = Node::new(tmp.path(), "a");
    fs::write(a.root.join("note"), b"first").unwrap();
    a.command("share-scan", &[]);
    let previous = tmp.path().join("restore-source");
    fs::write(&previous, b"restored through old CLI").unwrap();
    everywhere::storage::restore(&previous, &a.root.join("note")).unwrap();
    a.command("share-scan", &[]);
    assert_eq!(
        fs::read(a.root.join("note")).unwrap(),
        b"restored through old CLI"
    );
}

#[cfg(unix)]
#[test]
fn remote_replacement_keeps_private_destination_and_recovery_private() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let (a, b) = pair(tmp.path());
    fs::write(a.root.join("private"), b"initial").unwrap();
    sync(&a, &b);
    fs::set_permissions(b.root.join("private"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(a.root.join("private"), b"remote edit").unwrap();
    sync(&a, &b);
    assert_eq!(fs::read(b.root.join("private")).unwrap(), b"remote edit");
    assert_eq!(
        fs::metadata(b.root.join("private"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(b.root.join(".everywhere-recovery"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

fn restart_after_publication_database_failure(local_edit: bool) {
    // Inject an actual SQLite write failure, not a fake filesystem or network.
    // The persistent server is then killed with the new bytes on disk but the
    // old materialization in SQLite, exactly the recovery gap under test.
    let tmp = TempDir::new().unwrap();
    let result = std::panic::catch_unwind(|| {
        let (a, b) = pair(tmp.path());
        fs::write(a.root.join("note"), b"initial").unwrap();
        sync(&a, &b);
        let db_path = b.state.join("shares/personal/index.sqlite");
        let db = rusqlite::Connection::open(&db_path).unwrap();
        let before: String = db
            .query_row(
                "SELECT materialized FROM entries WHERE path='note'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        db.execute_batch(
            "CREATE TRIGGER test_fail_materialization BEFORE UPDATE OF materialized ON entries
             WHEN NEW.path='note' AND NEW.materialized!=OLD.materialized
             BEGIN SELECT RAISE(ABORT,'injected publication database failure'); END;",
        )
        .unwrap();
        drop(db);
        fs::write(a.root.join("note"), b"remote revision").unwrap();

        let mut child = Command::new(env!("CARGO_BIN_EXE_everywhere"))
            .args([
                "sync-serve",
                "--state",
                s(&b.state),
                "--folder",
                "personal",
                "--peer",
                &a.id,
                "--listen",
                "127.0.0.1:0",
            ])
            .stdout(Stdio::piped())
            .stderr(fs::File::create(tmp.path().join("receiver.log")).unwrap())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut server = Server(child);
        let (tx, rx) = mpsc::channel();
        let startup = thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            let mut line = String::new();
            stdout.read_line(&mut line).unwrap();
            let _ = tx.send(line);
            stdout
        });
        let line = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let _stdout = startup.join().unwrap();
        let address = line.trim().strip_prefix("LISTEN ").unwrap();
        let failed = run(&[
            "sync",
            "--state",
            s(&a.state),
            "--folder",
            "personal",
            "--peer",
            &b.id,
            "--addr",
            address,
        ]);
        fs::write(tmp.path().join("sender.log"), &failed.stderr).unwrap();
        assert!(!failed.status.success());
        let sent_revision = head_state(&a, "note");
        assert_eq!(
            sha(&fs::read(b.root.join("note")).unwrap()),
            sha(b"remote revision")
        );
        let db = rusqlite::Connection::open(&db_path).unwrap();
        let materialized: String = db
            .query_row(
                "SELECT materialized FROM entries WHERE path='note'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            materialized, before,
            "fault did not hit the publication/DB gap"
        );
        assert!(server.0.try_wait().unwrap().is_none());
        server.0.kill().unwrap();
        server.0.wait().unwrap();
        db.execute_batch("DROP TRIGGER test_fail_materialization")
            .unwrap();
        drop(db);

        if local_edit {
            fs::write(b.root.join("note"), b"local edit after crash").unwrap();
        }
        sync(&a, &b);
        let settled = head_state(&a, "note");
        assert_eq!(head_state(&b, "note"), settled);
        if local_edit {
            for node in [&a, &b] {
                let conflicts: Value =
                    serde_json::from_str(&node.command("share-conflicts", &[])).unwrap();
                let mut actual: Vec<_> = conflicts[0]["revisions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| sha(&fs::read(r["object_path"].as_str().unwrap()).unwrap()))
                    .collect();
                let mut expected = vec![sha(b"remote revision"), sha(b"local edit after crash")];
                actual.sort();
                expected.sort();
                assert_eq!(actual, expected);
            }
        } else {
            assert_eq!(
                settled, sent_revision,
                "restart invented another causal revision"
            );
            assert_eq!(settled["heads"].as_array().unwrap().len(), 1);
            assert_eq!(
                sha(&fs::read(b.root.join("note")).unwrap()),
                sha(b"remote revision")
            );
        }
        sync(&b, &a);
        assert_eq!(head_state(&a, "note"), settled);
        assert_eq!(head_state(&b, "note"), settled);
    });
    if let Err(error) = result {
        let evidence = tmp.keep();
        eprintln!(
            "Recovery failure evidence (device DBs, objects, journals, logs): {}",
            evidence.display()
        );
        std::panic::resume_unwind(error);
    }
}

#[test]
fn killed_receiver_adopts_published_file_without_inventing_a_local_edit() {
    restart_after_publication_database_failure(false);
}

#[test]
fn killed_receiver_preserves_a_local_edit_made_before_restarting() {
    restart_after_publication_database_failure(true);
}
