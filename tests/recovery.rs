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
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("CLI timeout: {args:?}: {:?}", child.wait_with_output());
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}
fn ok(args: &[&str]) -> String {
    let result = run(args);
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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
        fs::create_dir(&root).unwrap();
        let id = ok(&["init", "--state", s(&state)]).trim().to_owned();
        let node = Self { state, root, id };
        node.command("share-init", &["--root", s(&node.root)]);
        node
    }
    fn command(&self, command: &str, extra: &[&str]) -> String {
        let mut args = vec![command, "--state", s(&self.state), "--folder", "personal"];
        args.extend_from_slice(extra);
        ok(&args)
    }
    fn rejected(&self, command: &str, extra: &[&str]) {
        let mut args = vec![command, "--state", s(&self.state), "--folder", "personal"];
        args.extend_from_slice(extra);
        let result = run(&args);
        assert!(!result.status.success(), "unexpected success: {args:?}");
    }
    fn allow(&self, other: &Self) {
        ok(&[
            "trust",
            "--state",
            s(&self.state),
            "--cert",
            s(&other.state.join("identity.der")),
        ]);
        self.command("share-peer", &["--peer", &other.id]);
    }
    fn status(&self) -> Value {
        serde_json::from_str(&self.command("share-status", &[])).unwrap()
    }
    fn heads(&self, path: &str) -> Value {
        self.status()["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["path"] == path)
            .unwrap()["versions"]
            .clone()
    }
    fn db(&self) -> PathBuf {
        self.state.join("shares/personal/index.sqlite")
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
    exchange(a, b, true);
}
fn exchange(a: &Node, b: &Node, succeeds: bool) {
    let log = fs::File::create(b.state.join("last-server.log")).unwrap();
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
        .stderr(log)
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).unwrap();
        let _ = tx.send(line);
    });
    let mut guard = Server(child);
    let ready = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let address = ready.trim().strip_prefix("LISTEN ").unwrap();
    if succeeds {
        a.command("sync", &["--peer", &b.id, "--addr", address]);
    } else {
        a.rejected("sync", &["--peer", &b.id, "--addr", address]);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(code) = guard.0.try_wait().unwrap() {
            assert!(
                code.success() == succeeds,
                "{}",
                fs::read_to_string(b.state.join("last-server.log")).unwrap()
            );
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn receive_only_peer_cannot_release_recovery_without_remote_metadata() {
    evidence(|base| {
        let (a, b) = pair(base);
        fs::write(a.root.join("note"), b"original").unwrap();
        sync(&a, &b);
        let checkpoint = base.join("checkpoint");
        a.command("share-backup", &["--output", s(&checkpoint)]);
        let path = b.state.join("shares/personal/config.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        config["mode"] = Value::String("receive-only".into());
        fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
        fs::write(a.root.join("note"), b"post-checkpoint bytes").unwrap();
        a.command("share-recover", &["--backup", s(&checkpoint)]);
        let before = a.status();
        exchange(&a, &b, false);
        assert_eq!(a.status(), before);
        assert_eq!(
            sha(&fs::read(a.root.join("note")).unwrap()),
            sha(b"post-checkpoint bytes")
        );
    });
}

#[test]
fn a_known_nonselected_remote_head_does_not_become_a_third_revision() {
    evidence(|base| {
        let (a, b) = pair(base);
        let c = Node::new(base, "c");
        b.allow(&c);
        c.allow(&b);
        fs::write(a.root.join("note"), b"original").unwrap();
        sync(&a, &b);
        sync(&b, &c);
        let checkpoint = base.join("checkpoint");
        a.command("share-backup", &["--output", s(&checkpoint)]);
        fs::write(b.root.join("note"), b"offline B").unwrap();
        fs::write(c.root.join("note"), b"offline C").unwrap();
        sync(&b, &c);
        let expected = b.heads("note");
        let selected_bytes = fs::read(b.root.join("note")).unwrap();
        let nonselected: &[u8] = if selected_bytes == b"offline B" {
            b"offline C"
        } else {
            b"offline B"
        };
        fs::write(a.root.join("note"), nonselected).unwrap();
        a.command("share-recover", &["--backup", s(&checkpoint)]);
        sync(&a, &b);
        sync(&a, &b);
        assert_eq!(a.heads("note"), expected);
        assert_eq!(b.heads("note"), expected);
        assert_eq!(
            sha(&fs::read(a.root.join("note")).unwrap()),
            sha(&selected_bytes)
        );
    });
}

#[test]
fn checkpoint_without_primary_key_is_rejected_before_publication() {
    evidence(|base| {
        let a = Node::new(base, "a");
        fs::write(a.root.join("note"), b"keep").unwrap();
        a.command("share-scan", &[]);
        let checkpoint = base.join("checkpoint");
        a.command("share-backup", &["--output", s(&checkpoint)]);
        let database = checkpoint.join("index.sqlite");
        {
            let db = rusqlite::Connection::open(&database).unwrap();
            db.execute_batch("ALTER TABLE entries RENAME TO old_entries;
                CREATE TABLE entries(path TEXT,alias TEXT,versions TEXT,materialized TEXT,observed TEXT,seq INTEGER);
                INSERT INTO entries SELECT * FROM old_entries; DROP TABLE old_entries;").unwrap();
        }
        let manifest_path = checkpoint.join("manifest.json");
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["database_hash"] = Value::String(
            blake3::hash(&fs::read(database).unwrap())
                .to_hex()
                .to_string(),
        );
        fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let before = a.status();
        a.rejected("share-recover", &["--backup", s(&checkpoint)]);
        assert_eq!(a.status(), before);
        assert_eq!(sha(&fs::read(a.root.join("note")).unwrap()), sha(b"keep"));
    });
}
fn pair(base: &Path) -> (Node, Node) {
    let a = Node::new(base, "a");
    let b = Node::new(base, "b");
    a.allow(&b);
    b.allow(&a);
    (a, b)
}
fn evidence(test: impl FnOnce(&Path) + std::panic::UnwindSafe) {
    let reports = Path::new(env!("CARGO_MANIFEST_DIR")).join("folder-report/recovery-evidence");
    fs::create_dir_all(&reports).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("cli-")
        .tempdir_in(reports)
        .unwrap();
    let result = std::panic::catch_unwind(|| test(temp.path()));
    if let Err(error) = result {
        eprintln!("Recovery evidence retained at {}", temp.keep().display());
        std::panic::resume_unwind(error);
    }
}

#[test]
fn restored_index_reconciles_remote_bytes_tombstones_and_pending_reviews() {
    // Losing the recovery gate would create local revisions from already received
    // bytes; reusing old cursors would skip the post-checkpoint remote tombstone.
    evidence(|base| {
        let (a, b) = pair(base);
        let c = Node::new(base, "c");
        b.allow(&c);
        c.allow(&b);
        for name in ["note", "gone", "pending", "recent"] {
            fs::write(a.root.join(name), b"original").unwrap();
        }
        sync(&a, &b);
        sync(&b, &c);
        fs::remove_file(a.root.join("gone")).unwrap();
        a.command("share-approve-deletes", &["--all"]);
        fs::remove_file(a.root.join("pending")).unwrap();
        a.command("share-scan", &[]);
        sync(&a, &b);
        let backup = base.join("checkpoint");
        a.command("share-backup", &["--output", s(&backup)]);
        let old_epoch = a.status()["epoch"].clone();
        fs::write(b.root.join("note"), b"remote after checkpoint").unwrap();
        fs::write(b.root.join("later"), b"new remote path").unwrap();
        fs::remove_file(b.root.join("recent")).unwrap();
        b.command("share-approve-deletes", &["--all"]);
        sync(&a, &b);
        let expected_note = b.heads("note");
        let expected_later = b.heads("later");
        fs::write(a.db(), b"corrupt live index").unwrap();
        a.command("share-recover", &["--backup", s(&backup)]);
        assert_eq!(
            sha(&fs::read(a.root.join("note")).unwrap()),
            sha(b"remote after checkpoint")
        );
        assert_ne!(a.status()["epoch"], old_epoch);
        assert_eq!(a.status()["recovery_pending"], true);
        a.rejected("share-scan", &[]);
        a.rejected("share-approve-deletes", &["--all"]);
        sync(&a, &b);
        sync(&a, &b);
        for node in [&a, &b] {
            assert_eq!(node.heads("note"), expected_note);
            assert_eq!(node.heads("later"), expected_later);
            assert!(!node.root.join("gone").exists());
            assert!(!node.root.join("recent").exists());
        }
        assert_eq!(a.status()["recovery_pending"], false);
        assert_eq!(
            a.status()["pending_deletions"],
            serde_json::json!(["pending"])
        );
        assert_eq!(
            sha(&fs::read(b.root.join("pending")).unwrap()),
            sha(b"original")
        );
        sync(&b, &c);
        assert!(!c.root.join("gone").exists());
        assert!(!c.root.join("recent").exists());
        assert_eq!(c.heads("note"), expected_note);
        a.command("share-approve-deletes", &["--all"]);
        let deletion = a.heads("pending");
        a.command("share-approve-deletes", &["--all"]);
        assert_eq!(a.heads("pending"), deletion);
        sync(&a, &b);
        assert!(!b.root.join("pending").exists());
        let history: Value =
            serde_json::from_str(&a.command("share-history", &["--path", "note"])).unwrap();
        assert!(history.as_array().unwrap().iter().any(|r| {
            r["object_path"]
                .as_str()
                .is_some_and(|p| sha(&fs::read(p).unwrap()) == sha(b"original"))
        }));
    });
}

#[test]
fn recovery_preserves_a_distinct_local_edit_and_current_peer_revocation() {
    evidence(|base| {
        let (a, b) = pair(base);
        let denied = Node::new(base, "denied");
        a.allow(&denied);
        fs::write(a.root.join("note"), b"original").unwrap();
        sync(&a, &b);
        let backup = base.join("checkpoint");
        a.command("share-backup", &["--output", s(&backup)]);
        a.command("share-peer", &["--peer", &denied.id, "--remove"]);
        fs::write(b.root.join("note"), b"remote edit").unwrap();
        b.command("share-scan", &[]);
        fs::write(a.root.join("note"), b"local edit after checkpoint").unwrap();
        fs::remove_file(a.db()).unwrap();
        a.command("share-recover", &["--backup", s(&backup)]);
        let config: Value =
            serde_json::from_slice(&fs::read(a.state.join("shares/personal/config.json")).unwrap())
                .unwrap();
        assert!(
            !config["peers"]
                .as_array()
                .unwrap()
                .contains(&Value::String(denied.id))
        );
        assert_eq!(
            sha(&fs::read(a.root.join("note")).unwrap()),
            sha(b"local edit after checkpoint")
        );
        sync(&a, &b);
        sync(&a, &b);
        for node in [&a, &b] {
            let conflicts: Value =
                serde_json::from_str(&node.command("share-conflicts", &[])).unwrap();
            let mut got: Vec<_> = conflicts[0]["revisions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| sha(&fs::read(r["object_path"].as_str().unwrap()).unwrap()))
                .collect();
            let mut expected = vec![sha(b"remote edit"), sha(b"local edit after checkpoint")];
            got.sort();
            expected.sort();
            assert_eq!(got, expected);
        }
        let heads = a.heads("note");
        sync(&a, &b);
        assert_eq!(a.heads("note"), heads);
        assert_eq!(b.heads("note"), heads);
    });
}

#[test]
fn invalid_checkpoint_is_rejected_before_changing_live_index_or_files() {
    evidence(|base| {
        let a = Node::new(base, "a");
        fs::write(a.root.join("note"), b"keep").unwrap();
        a.command("share-scan", &[]);
        let backup = base.join("checkpoint");
        a.command("share-backup", &["--output", s(&backup)]);
        let before = a.status();
        let objects = backup.join("objects");
        let object = fs::read_dir(objects)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        fs::write(&object, b"damaged archived bytes").unwrap();
        a.rejected("share-recover", &["--backup", s(&backup)]);
        assert_eq!(a.status(), before);
        assert_eq!(sha(&fs::read(a.root.join("note")).unwrap()), sha(b"keep"));
        assert!(!a.state.join("shares/personal/state-recovery.json").exists());
        fs::write(&object, b"keep").unwrap();
        let missing = base.join("saved-object");
        fs::rename(&object, &missing).unwrap();
        a.rejected("share-recover", &["--backup", s(&backup)]);
        fs::rename(&missing, &object).unwrap();
        let manifest_path = backup.join("manifest.json");
        let manifest = fs::read(&manifest_path).unwrap();
        fs::remove_file(&manifest_path).unwrap();
        a.rejected("share-recover", &["--backup", s(&backup)]);
        fs::write(&manifest_path, b"{").unwrap();
        a.rejected("share-recover", &["--backup", s(&backup)]);
        let mut wrong_root: Value = serde_json::from_slice(&manifest).unwrap();
        wrong_root["config"]["root"] = serde_json::json!(base.join("other-root"));
        fs::write(&manifest_path, serde_json::to_vec(&wrong_root).unwrap()).unwrap();
        a.rejected("share-recover", &["--backup", s(&backup)]);
        fs::write(&manifest_path, manifest).unwrap();
        let database = backup.join("index.sqlite");
        let original_db = fs::read(&database).unwrap();
        fs::write(&database, b"broken checkpoint index").unwrap();
        a.rejected("share-recover", &["--backup", s(&backup)]);
        fs::write(database, original_db).unwrap();
        assert_eq!(a.status(), before);
        a.rejected("share-backup", &["--output", s(&backup)]);
        a.rejected(
            "share-backup",
            &["--output", s(&a.root.join("unsafe-backup"))],
        );
        assert!(!a.root.join("unsafe-backup").exists());
        a.rejected(
            "share-backup",
            &["--output", s(&a.state.join("unsafe-backup"))],
        );
        assert!(!a.state.join("unsafe-backup").exists());
    });
}

#[test]
fn checkpoint_refuses_active_share_and_a_different_device() {
    evidence(|base| {
        let (a, b) = pair(base);
        let backup = base.join("checkpoint");
        let held = everywhere::share::Share::open(&a.state, "personal").unwrap();
        a.rejected("share-backup", &["--output", s(&backup)]);
        assert!(!backup.exists());
        drop(held);
        a.command("share-backup", &["--output", s(&backup)]);
        let held = everywhere::share::Share::open(&a.state, "personal").unwrap();
        a.rejected("share-recover", &["--backup", s(&backup)]);
        drop(held);
        b.rejected("share-recover", &["--backup", s(&backup)]);
        assert_eq!(b.status()["entries"], serde_json::json!([]));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&backup).unwrap().permissions().mode() & 0o077,
                0
            );
            for name in ["manifest.json", "index.sqlite"] {
                assert_eq!(
                    fs::metadata(backup.join(name))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o077,
                    0
                );
            }
        }
    });
}

#[test]
fn recovery_repairs_corrupt_cache_without_discarding_the_damaged_bytes() {
    evidence(|base| {
        let (a, b) = pair(base);
        fs::write(a.root.join("note"), b"keep").unwrap();
        sync(&a, &b);
        let backup = base.join("checkpoint");
        a.command("share-backup", &["--output", s(&backup)]);
        let history: Value =
            serde_json::from_str(&a.command("share-history", &["--path", "note"])).unwrap();
        let object = PathBuf::from(history[0]["object_path"].as_str().unwrap());
        fs::write(&object, b"damaged cached bytes").unwrap();
        a.command("share-recover", &["--backup", s(&backup)]);
        assert_eq!(sha(&fs::read(&object).unwrap()), sha(b"keep"));
        let share = a.state.join("shares/personal");
        let archived = fs::read_dir(share)
            .unwrap()
            .filter_map(|e| {
                let e = e.unwrap();
                if e.file_name().to_string_lossy().starts_with("index-before-") {
                    Some(e.path().join(format!(
                        "corrupt-object-{}",
                        object.file_name().unwrap().to_str().unwrap()
                    )))
                } else {
                    None
                }
            })
            .find(|p| p.is_file())
            .unwrap();
        assert_eq!(
            sha(&fs::read(archived).unwrap()),
            sha(b"damaged cached bytes")
        );
        sync(&a, &b);
        assert_eq!(sha(&fs::read(a.root.join("note")).unwrap()), sha(b"keep"));
    });
}

#[test]
fn recovery_keeps_send_only_and_receive_only_direction_policies() {
    for mode in ["send-only", "receive-only"] {
        evidence(|base| {
            let (a, b) = pair(base);
            let config_path = a.state.join("shares/personal/config.json");
            let mut config: Value =
                serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
            config["mode"] = Value::String(mode.into());
            fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
            let source = if mode == "send-only" { &a } else { &b };
            fs::write(source.root.join("note"), b"original").unwrap();
            sync(&a, &b);
            let backup = base.join("checkpoint");
            a.command("share-backup", &["--output", s(&backup)]);
            fs::write(source.root.join("note"), b"after checkpoint").unwrap();
            a.command("share-recover", &["--backup", s(&backup)]);
            assert_eq!(a.status()["recovery_pending"], mode == "receive-only");
            sync(&a, &b);
            for node in [&a, &b] {
                assert_eq!(
                    sha(&fs::read(node.root.join("note")).unwrap()),
                    sha(b"after checkpoint")
                );
            }
            let config: Value = serde_json::from_slice(&fs::read(config_path).unwrap()).unwrap();
            assert_eq!(config["mode"], mode);
        });
    }
}

#[cfg(unix)]
#[test]
fn checkpoint_object_symlink_is_rejected_without_touching_its_target() {
    use std::os::unix::fs::symlink;
    evidence(|base| {
        let a = Node::new(base, "a");
        fs::write(a.root.join("note"), b"keep").unwrap();
        a.command("share-scan", &[]);
        let backup = base.join("checkpoint");
        a.command("share-backup", &["--output", s(&backup)]);
        let object = fs::read_dir(backup.join("objects"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let external = base.join("external");
        fs::write(&external, b"keep").unwrap();
        fs::remove_file(&object).unwrap();
        symlink(&external, &object).unwrap();
        let before = a.status();
        a.rejected("share-recover", &["--backup", s(&backup)]);
        assert_eq!(a.status(), before);
        assert_eq!(sha(&fs::read(external).unwrap()), sha(b"keep"));
    });
}
