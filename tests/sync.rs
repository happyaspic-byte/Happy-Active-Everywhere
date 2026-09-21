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
    let deadline = Instant::now() + Duration::from_secs(20);
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
