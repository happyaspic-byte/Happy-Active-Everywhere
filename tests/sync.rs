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
