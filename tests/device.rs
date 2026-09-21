use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

struct Evidence(Option<tempfile::TempDir>);
impl Evidence {
    fn new() -> Self {
        let reports = Path::new(env!("CARGO_MANIFEST_DIR")).join("folder-report/recovery-evidence");
        fs::create_dir_all(&reports).unwrap();
        Self(Some(
            tempfile::Builder::new()
                .prefix("device-")
                .tempdir_in(reports)
                .unwrap(),
        ))
    }
    fn path(&self) -> &Path {
        self.0.as_ref().unwrap().path()
    }
}
impl Drop for Evidence {
    fn drop(&mut self) {
        if std::thread::panicking() {
            if let Some(temp) = self.0.take() {
                eprintln!("Device evidence retained at {}", temp.keep().display());
            }
        }
    }
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}
fn cli() -> Command {
    #[cfg(unix)]
    {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "umask 000; exec \"$@\"",
            "everywhere-test",
            env!("CARGO_BIN_EXE_everywhere"),
        ]);
        command
    }
    #[cfg(not(unix))]
    {
        Command::new(env!("CARGO_BIN_EXE_everywhere"))
    }
}
fn run(args: &[&str]) -> Output {
    use std::{
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };
    let mut child = cli()
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("CLI timeout {args:?}: {:?}", child.wait_with_output());
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

fn enroll(state: &Path, other: &Path, peer: &str) {
    ok(&[
        "trust",
        "--state",
        s(state),
        "--cert",
        s(&other.join("identity.der")),
    ]);
    ok(&[
        "share-peer",
        "--state",
        s(state),
        "--folder",
        "personal",
        "--peer",
        peer,
    ]);
}
fn exchange(a: &Path, aid: &str, b: &Path, bid: &str) {
    use std::{
        io::{BufRead, BufReader},
        process::{Child, Stdio},
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };
    struct Guard(Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let log = b.join("last-server.log");
    let mut server = Guard(
        Command::new(env!("CARGO_BIN_EXE_everywhere"))
            .args([
                "sync-serve",
                "--state",
                s(b),
                "--folder",
                "personal",
                "--peer",
                aid,
                "--listen",
                "127.0.0.1:0",
                "--once",
            ])
            .stdout(Stdio::piped())
            .stderr(fs::File::create(&log).unwrap())
            .spawn()
            .unwrap(),
    );
    let stdout = server.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).unwrap();
        let _ = tx.send(line);
    });
    let ready = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let address = ready.trim().strip_prefix("LISTEN ").unwrap();
    ok(&[
        "sync",
        "--state",
        s(a),
        "--folder",
        "personal",
        "--peer",
        bid,
        "--addr",
        address,
    ]);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            assert!(status.success(), "{}", fs::read_to_string(log).unwrap());
            break;
        }
        assert!(Instant::now() < deadline, "server exit timeout");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn recovery_reconciles_newer_remote_deletion_then_syncs_both_directions() {
    let base = Evidence::new();
    let a = base.path().join("a");
    let b = base.path().join("b");
    let ar = base.path().join("a-files");
    let br = base.path().join("b-files");
    let aid = ok(&["init", "--state", s(&a)]).trim().to_owned();
    let bid = ok(&["init", "--state", s(&b)]).trim().to_owned();
    for (state, root) in [(&a, &ar), (&b, &br)] {
        fs::create_dir(root).unwrap();
        ok(&[
            "share-init",
            "--state",
            s(state),
            "--folder",
            "personal",
            "--root",
            s(root),
        ]);
    }
    enroll(&a, &b, &bid);
    enroll(&b, &a, &aid);
    fs::write(ar.join("later-deleted"), b"must not resurrect").unwrap();
    fs::write(ar.join("note"), b"baseline").unwrap();
    exchange(&a, &aid, &b, &bid);
    let key = base.path().join("offline-key.agekey");
    let recipient = json(&["device-keygen", "--output", s(&key)])["recipient"]
        .as_str()
        .unwrap()
        .to_owned();
    let backup = base.path().join("backup.age");
    ok(&[
        "device-backup",
        "--state",
        s(&a),
        "--recipient",
        &recipient,
        "--output",
        s(&backup),
    ]);
    fs::remove_file(br.join("later-deleted")).unwrap();
    ok(&[
        "share-approve-deletes",
        "--state",
        s(&b),
        "--folder",
        "personal",
        "--all",
    ]);
    let workspace = base.path().join("c");
    let result = json(&[
        "device-recover",
        "--backup",
        s(&backup),
        "--key",
        s(&key),
        "--output",
        s(&workspace),
    ]);
    let cid = result["identity"].as_str().unwrap();
    let c = workspace.join("state");
    let cr = workspace.join("folders/personal");
    assert!(cr.join("later-deleted").exists());
    enroll(&b, &c, cid);
    enroll(&c, &b, &bid);
    exchange(&c, cid, &b, &bid);
    assert!(!cr.join("later-deleted").exists());
    assert!(!br.join("later-deleted").exists());
    assert_eq!(sha(&cr.join("note")), sha(&br.join("note")));
    ok(&["device-activate", "--state", s(&c), "--folder", "personal"]);
    fs::write(cr.join("note"), b"C edit after recovery").unwrap();
    exchange(&c, cid, &b, &bid);
    assert_eq!(
        sha(&br.join("note")),
        format!("{:x}", Sha256::digest(b"C edit after recovery"))
    );
    fs::write(br.join("note"), b"B edit returns to recovered C").unwrap();
    exchange(&c, cid, &b, &bid);
    assert_eq!(
        sha(&cr.join("note")),
        format!("{:x}", Sha256::digest(b"B edit returns to recovered C"))
    );
    assert!(!cr.join("later-deleted").exists());
    assert_eq!(
        json(&["share-conflicts", "--state", s(&c), "--folder", "personal"]),
        serde_json::json!([])
    );
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
fn reject(args: &[&str]) {
    let output = run(args);
    assert!(!output.status.success(), "unexpected success: {args:?}");
}
fn json(args: &[&str]) -> Value {
    serde_json::from_str(&ok(args)).unwrap()
}
fn sha(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

#[test]
fn interrupted_device_operations_publish_nothing_and_retry_safely() {
    use std::{
        io::Write,
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };
    let base = Evidence::new();
    let state = base.path().join("state");
    let root = base.path().join("files");
    fs::create_dir(&root).unwrap();
    ok(&["init", "--state", s(&state)]);
    ok(&[
        "share-init",
        "--state",
        s(&state),
        "--folder",
        "personal",
        "--root",
        s(&root),
    ]);
    let payload = root.join("large.bin");
    let mut file = fs::File::create(&payload).unwrap();
    for _ in 0..32 {
        file.write_all(&vec![173u8; 1024 * 1024]).unwrap();
    }
    file.sync_all().unwrap();
    drop(file);
    let original = sha(&payload);
    let key = base.path().join("key.agekey");
    let recipient = json(&["device-keygen", "--output", s(&key)])["recipient"]
        .as_str()
        .unwrap()
        .to_owned();
    let backup = base.path().join("backup.age");
    let workspace = base.path().join("recovered");
    let backup_args = [
        "device-backup",
        "--state",
        s(&state),
        "--recipient",
        &recipient,
        "--output",
        s(&backup),
    ];
    let restore_args = [
        "device-recover",
        "--backup",
        s(&backup),
        "--key",
        s(&key),
        "--output",
        s(&workspace),
    ];
    for (args, prefix, output) in [
        (&backup_args[..], ".everywhere-backup-", &backup),
        (&restore_args[..], ".everywhere-restore-", &workspace),
    ] {
        let log = base.path().join("interrupted.stderr");
        let mut child = cli()
            .args(args)
            .stdout(Stdio::null())
            .stderr(fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if fs::read_dir(base.path()).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(prefix)
            }) {
                break;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "operation exited before interruption: {}",
                fs::read_to_string(&log).unwrap()
            );
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("staging did not appear");
            }
            thread::sleep(Duration::from_millis(1));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for entry in fs::read_dir(base.path()).unwrap() {
                let entry = entry.unwrap();
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".everywhere-")
                    && entry.file_type().unwrap().is_dir()
                {
                    assert_eq!(
                        entry.metadata().unwrap().permissions().mode() & 0o777,
                        0o700
                    );
                }
            }
        }
        assert!(!output.exists(), "interrupted operation published output");
        assert_eq!(sha(&payload), original);
        ok(args);
    }
    assert_eq!(sha(&workspace.join("folders/personal/large.bin")), original);
}

#[test]
fn encrypted_capture_authenticates_and_refuses_replacement() {
    let base = Evidence::new();
    let state = base.path().join("state");
    let root = base.path().join("files");
    fs::create_dir(&root).unwrap();
    let identity = ok(&["init", "--state", s(&state)]).trim().to_owned();
    ok(&[
        "share-init",
        "--state",
        s(&state),
        "--folder",
        "personal",
        "--root",
        s(&root),
    ]);
    fs::write(root.join("비밀.txt"), b"private payload after registration").unwrap();
    let before = sha(&root.join("비밀.txt"));
    let key = base.path().join("offline-key.agekey");
    let recipient = json(&["device-keygen", "--output", s(&key)])["recipient"]
        .as_str()
        .unwrap()
        .to_owned();
    let key_before = sha(&key);
    reject(&["device-keygen", "--output", s(&key)]);
    assert_eq!(sha(&key), key_before);
    let backup = base.path().join("backup.age");
    let args = [
        "device-backup",
        "--state",
        s(&state),
        "--recipient",
        &recipient,
        "--output",
        s(&backup),
    ];
    let result = json(&args);
    assert_eq!(result["status"], "device-backup-complete");
    let ciphertext = fs::read(&backup).unwrap();
    for secret in [
        fs::read(state.join("identity.key.der")).unwrap(),
        fs::read(root.join("비밀.txt")).unwrap(),
    ] {
        assert!(!ciphertext.windows(secret.len()).any(|w| w == secret));
    }
    let inspected = json(&["device-inspect", "--backup", s(&backup), "--key", s(&key)]);
    assert_eq!(inspected["identity"], identity);
    assert_eq!(inspected["folders"][0]["id"], "personal");
    assert_eq!(sha(&root.join("비밀.txt")), before);
    let digest = sha(&backup);
    reject(&args);
    assert_eq!(sha(&backup), digest);
    let wrong = base.path().join("wrong-key.agekey");
    ok(&["device-keygen", "--output", s(&wrong)]);
    reject(&["device-inspect", "--backup", s(&backup), "--key", s(&wrong)]);
    let broken = base.path().join("broken.age");
    fs::write(&broken, &ciphertext[..ciphertext.len() - 1]).unwrap();
    reject(&["device-inspect", "--backup", s(&broken), "--key", s(&key)]);
    let mut tampered = ciphertext.clone();
    let end = tampered.len() - 24;
    tampered[end] ^= 1;
    fs::write(&broken, tampered).unwrap();
    reject(&["device-inspect", "--backup", s(&broken), "--key", s(&key)]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&key).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn encrypted_capture_rejects_busy_or_missing_roots() {
    use fs2::FileExt;
    let base = Evidence::new();
    let state = base.path().join("state");
    let root = base.path().join("files");
    fs::create_dir(&root).unwrap();
    ok(&["init", "--state", s(&state)]);
    ok(&[
        "share-init",
        "--state",
        s(&state),
        "--folder",
        "personal",
        "--root",
        s(&root),
    ]);
    let key = base.path().join("key.agekey");
    let recipient = json(&["device-keygen", "--output", s(&key)])["recipient"]
        .as_str()
        .unwrap()
        .to_owned();
    let backup = base.path().join("backup.age");
    let args = [
        "device-backup",
        "--state",
        s(&state),
        "--recipient",
        &recipient,
        "--output",
        s(&backup),
    ];
    for name in [
        "service.lock",
        "management.lock",
        "shares/personal/index.lock",
    ] {
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(state.join(name))
            .unwrap();
        lock.try_lock_exclusive().unwrap();
        reject(&args);
        assert!(!backup.exists());
        drop(lock);
    }
    fs::rename(&root, base.path().join("detached")).unwrap();
    reject(&args);
    assert!(!backup.exists());
}

#[test]
fn recovery_preserves_content_history_pending_deletes_and_quarantines_authority() {
    let base = Evidence::new();
    let state = base.path().join("state");
    let root = base.path().join("files");
    let other = base.path().join("other");
    fs::create_dir(&root).unwrap();
    let id = ok(&["init", "--state", s(&state)]).trim().to_owned();
    let peer = ok(&["init", "--state", s(&other)]).trim().to_owned();
    ok(&[
        "share-init",
        "--state",
        s(&state),
        "--folder",
        "personal",
        "--root",
        s(&root),
        "--mode",
        "send-only",
    ]);
    ok(&[
        "trust",
        "--state",
        s(&state),
        "--cert",
        s(&other.join("identity.der")),
    ]);
    ok(&[
        "share-peer",
        "--state",
        s(&state),
        "--folder",
        "personal",
        "--peer",
        &peer,
    ]);
    fs::write(state.join("jobs.json"), serde_json::to_vec(&serde_json::json!({"daily":{"id":"daily","folder":"personal","peer":peer,"address":"127.0.0.1:7999","direction":"connect","enabled":true}})).unwrap()).unwrap();
    fs::create_dir(root.join("empty")).unwrap();
    fs::write(root.join("비밀.txt"), b"old version").unwrap();
    fs::write(root.join("pending"), b"pending-delete contents").unwrap();
    fs::write(root.join("approved-gone"), b"deleted before backup").unwrap();
    ok(&["share-scan", "--state", s(&state), "--folder", "personal"]);
    fs::remove_file(root.join("approved-gone")).unwrap();
    ok(&[
        "share-approve-deletes",
        "--state",
        s(&state),
        "--folder",
        "personal",
        "--all",
    ]);
    fs::write(root.join("비밀.txt"), b"new version").unwrap();
    fs::remove_file(root.join("pending")).unwrap();
    let key = base.path().join("key.agekey");
    let recipient = json(&["device-keygen", "--output", s(&key)])["recipient"]
        .as_str()
        .unwrap()
        .to_owned();
    let backup = base.path().join("backup.age");
    ok(&[
        "device-backup",
        "--state",
        s(&state),
        "--recipient",
        &recipient,
        "--output",
        s(&backup),
    ]);
    let old_status = json(&["share-status", "--state", s(&state), "--folder", "personal"]);
    let old_history = json(&[
        "share-history",
        "--state",
        s(&state),
        "--folder",
        "personal",
        "--path",
        "비밀.txt",
    ]);
    let payload_hash = sha(&root.join("비밀.txt"));
    let nested_output = root.join("nested-restoration");
    reject(&[
        "device-recover",
        "--backup",
        s(&backup),
        "--key",
        s(&key),
        "--output",
        s(&nested_output),
    ]);
    assert!(!nested_output.exists());
    let output = base.path().join("recovered");
    let restored = json(&[
        "device-recover",
        "--backup",
        s(&backup),
        "--key",
        s(&key),
        "--output",
        s(&output),
    ]);
    assert_ne!(restored["identity"], id);
    let restored_state = output.join("state");
    let restored_root = output.join("folders/personal");
    assert_eq!(sha(&restored_root.join("비밀.txt")), payload_hash);
    assert!(restored_root.join("empty").is_dir());
    assert!(!restored_root.join("pending").exists());
    assert!(!restored_root.join("approved-gone").exists());
    assert_eq!(
        fs::read_dir(restored_state.join("peers")).unwrap().count(),
        0
    );
    let config: Value = serde_json::from_slice(
        &fs::read(restored_state.join("shares/personal/config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(config["mode"], "receive-only");
    assert_eq!(config["peers"], serde_json::json!([]));
    assert_eq!(
        Path::new(config["root"].as_str().unwrap()),
        restored_root.canonicalize().unwrap()
    );
    let jobs: Value =
        serde_json::from_slice(&fs::read(restored_state.join("jobs.json")).unwrap()).unwrap();
    assert_eq!(jobs["daily"]["enabled"], false);
    let status = json(&[
        "share-status",
        "--state",
        s(&restored_state),
        "--folder",
        "personal",
    ]);
    assert_eq!(status["recovery_pending"], true);
    assert_eq!(status["pending_deletions"], old_status["pending_deletions"]);
    assert_ne!(status["epoch"], old_status["epoch"]);
    let history = json(&[
        "share-history",
        "--state",
        s(&restored_state),
        "--folder",
        "personal",
        "--path",
        "비밀.txt",
    ]);
    assert_eq!(
        history.as_array().unwrap().len(),
        old_history.as_array().unwrap().len()
    );
    for (actual, expected) in history
        .as_array()
        .unwrap()
        .iter()
        .zip(old_history.as_array().unwrap())
    {
        for field in ["id", "content", "clock"] {
            assert_eq!(actual[field], expected[field]);
        }
        assert_eq!(
            sha(Path::new(actual["object_path"].as_str().unwrap())),
            sha(Path::new(expected["object_path"].as_str().unwrap()))
        );
    }
    reject(&[
        "device-activate",
        "--state",
        s(&restored_state),
        "--folder",
        "personal",
    ]);
    ok(&[
        "device-activate",
        "--state",
        s(&restored_state),
        "--folder",
        "personal",
        "--offline-authority",
    ]);
    ok(&[
        "share-scan",
        "--state",
        s(&restored_state),
        "--folder",
        "personal",
    ]);
    reject(&[
        "device-recover",
        "--backup",
        s(&backup),
        "--key",
        s(&key),
        "--output",
        s(&output),
    ]);
    assert_eq!(sha(&restored_root.join("비밀.txt")), payload_hash);
    let replacement = base.path().join("replacement");
    reject(&[
        "device-recover",
        "--backup",
        s(&backup),
        "--key",
        s(&key),
        "--output",
        s(&replacement),
        "--retired-device",
        &peer,
    ]);
    assert!(!replacement.exists());
    // The original test node is now retired. Its credential file is never overwritten.
    fs::rename(&state, base.path().join("retired-state")).unwrap();
    let result = json(&[
        "device-recover",
        "--backup",
        s(&backup),
        "--key",
        s(&key),
        "--output",
        s(&replacement),
        "--retired-device",
        &id,
    ]);
    assert_eq!(result["identity"], id);
    assert_eq!(
        sha(&replacement.join("state/identity.key.der")),
        sha(&base.path().join("retired-state/identity.key.der"))
    );
}

#[cfg(unix)]
#[test]
fn recovery_workspace_is_private_under_permissive_umask() {
    use std::os::unix::fs::PermissionsExt;
    let base = Evidence::new();
    let state = base.path().join("state");
    ok(&["init", "--state", s(&state)]);
    let key = base.path().join("key.agekey");
    let recipient = json(&["device-keygen", "--output", s(&key)])["recipient"]
        .as_str()
        .unwrap()
        .to_owned();
    let backup = base.path().join("backup.age");
    ok(&[
        "device-backup",
        "--state",
        s(&state),
        "--recipient",
        &recipient,
        "--output",
        s(&backup),
    ]);
    let output = base.path().join("recovered");
    ok(&[
        "device-recover",
        "--backup",
        s(&backup),
        "--key",
        s(&key),
        "--output",
        s(&output),
    ]);
    assert_eq!(
        fs::metadata(output).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[cfg(unix)]
#[test]
fn recovery_does_not_require_access_to_historical_mount_ancestry() {
    use std::os::unix::fs::PermissionsExt;
    let base = Evidence::new();
    let state = base.path().join("state");
    let mount = base.path().join("old-mount");
    let root = mount.join("files");
    fs::create_dir_all(&root).unwrap();
    ok(&["init", "--state", s(&state)]);
    ok(&[
        "share-init",
        "--state",
        s(&state),
        "--folder",
        "personal",
        "--root",
        s(&root),
    ]);
    fs::write(root.join("file"), b"independent backup").unwrap();
    let key = base.path().join("key.agekey");
    let recipient = json(&["device-keygen", "--output", s(&key)])["recipient"]
        .as_str()
        .unwrap()
        .to_owned();
    let backup = base.path().join("backup.age");
    ok(&[
        "device-backup",
        "--state",
        s(&state),
        "--recipient",
        &recipient,
        "--output",
        s(&backup),
    ]);
    fs::set_permissions(&mount, fs::Permissions::from_mode(0o0)).unwrap();
    let output = base.path().join("recovered");
    let result = run(&[
        "device-recover",
        "--backup",
        s(&backup),
        "--key",
        s(&key),
        "--output",
        s(&output),
    ]);
    fs::set_permissions(&mount, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        sha(&output.join("folders/personal/file")),
        format!("{:x}", Sha256::digest(b"independent backup"))
    );
}
