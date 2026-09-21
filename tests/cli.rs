use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, Stdio},
};
use tempfile::TempDir;

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_everywhere"))
        .args(args)
        .output()
        .unwrap()
}
fn ok(args: &[&str]) -> String {
    let output = cli(args);
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}
fn init(path: &Path) {
    ok(&["init", "--state", s(path)]);
}
fn trust(state: &Path, peer: &Path) -> String {
    ok(&[
        "trust",
        "--state",
        s(state),
        "--cert",
        s(&peer.join("identity.der")),
    ])
    .trim()
    .to_owned()
}
struct Server {
    child: Child,
    addr: String,
    _stdout: BufReader<std::process::ChildStdout>,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn server(state: &Path, peer: &str, target: &Path) -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_everywhere"))
        .args([
            "receive",
            "--state",
            s(state),
            "--peer",
            peer,
            "--listen",
            "127.0.0.1:0",
            "--output",
            s(target),
            "--once",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    stdout.read_line(&mut line).unwrap();
    assert!(line.starts_with("LISTEN "), "server not ready: {line}");
    Server {
        child,
        addr: line.trim().strip_prefix("LISTEN ").unwrap().into(),
        _stdout: stdout,
    }
}
#[test]
fn init_preserves_identity_and_private_key_permissions() {
    let tmp = TempDir::new().unwrap();
    let state = tmp.path().join("device");
    init(&state);
    let cert = fs::read(state.join("identity.der")).unwrap();
    let key = fs::read(state.join("identity.key.der")).unwrap();
    assert!(!cli(&["init", "--state", s(&state)]).status.success());
    assert_eq!(cert, fs::read(state.join("identity.der")).unwrap());
    assert_eq!(key, fs::read(state.join("identity.key.der")).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(state.join("identity.key.der"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[test]
fn two_independent_devices_transfer_twice_and_reject_unapproved_peer() {
    for run in 0..2 {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        let c = tmp.path().join("c");
        init(&a);
        init(&b);
        init(&c);
        let aid = trust(&b, &a);
        let bid = trust(&a, &b);
        let _ = trust(&c, &b);
        let source = tmp.path().join("source");
        let target = tmp.path().join("target");
        let data: Vec<u8> = (0..(3 * 1024 * 1024 + 71))
            .map(|i| ((i * 31 + run) % 251) as u8)
            .collect();
        fs::write(&source, &data).unwrap();
        let mut srv = server(&b, &aid, &target);
        let result = ok(&[
            "send",
            "--state",
            s(&a),
            "--peer",
            &bid,
            "--addr",
            &srv.addr,
            "--source",
            s(&source),
        ]);
        assert!(result.contains("\"tls\":\"TLSv1_3\""), "{result}");
        assert!(result.contains("\"sent_blocks\":4"), "{result}");
        assert!(srv.child.wait().unwrap().success());
        assert_eq!(
            Sha256::digest(&data),
            Sha256::digest(fs::read(&target).unwrap())
        );
        let mut srv = server(&b, &aid, &target);
        let rejected = cli(&[
            "send",
            "--state",
            s(&c),
            "--peer",
            &bid,
            "--addr",
            &srv.addr,
            "--source",
            s(&source),
        ]);
        assert!(!rejected.status.success());
        assert!(!srv.child.wait().unwrap().success());
        assert_eq!(
            Sha256::digest(&data),
            Sha256::digest(fs::read(&target).unwrap())
        );
        ok(&["revoke", "--state", s(&a), "--peer", &bid]);
        assert!(
            !cli(&[
                "send",
                "--state",
                s(&a),
                "--peer",
                &bid,
                "--addr",
                "127.0.0.1:1",
                "--source",
                s(&source)
            ])
            .status
            .success()
        );
    }
}

#[test]
fn killed_sender_and_receiver_resume_twice_with_sha256() {
    for run in 0..2 {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        init(&a);
        init(&b);
        let aid = trust(&b, &a);
        let bid = trust(&a, &b);
        let source = tmp.path().join("source");
        let target = tmp.path().join("target");
        let data: Vec<u8> = (0..(5 * 1024 * 1024 + 71))
            .map(|i| ((i * 17 + run) % 251) as u8)
            .collect();
        fs::write(&source, &data).unwrap();
        fs::write(&target, b"old content").unwrap();
        let mut srv = server(&b, &aid, &target);
        let mut sender = Command::new(env!("CARGO_BIN_EXE_everywhere"))
            .args([
                "send",
                "--state",
                s(&a),
                "--peer",
                &bid,
                "--addr",
                &srv.addr,
                "--source",
                s(&source),
                "--block-delay-ms",
                "500",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut progress = BufReader::new(sender.stderr.take().unwrap());
        let mut line = String::new();
        progress.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "BLOCK 0");
        sender.kill().unwrap();
        sender.wait().unwrap();
        srv.child.kill().unwrap();
        srv.child.wait().unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"old content");
        let mut srv = server(&b, &aid, &target);
        let result = ok(&[
            "send",
            "--state",
            s(&a),
            "--peer",
            &bid,
            "--addr",
            &srv.addr,
            "--source",
            s(&source),
        ]);
        let report: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(report["reused_blocks"], 1);
        assert_eq!(report["sent_blocks"], 5);
        assert!(srv.child.wait().unwrap().success());
        assert_eq!(
            Sha256::digest(&data),
            Sha256::digest(fs::read(&target).unwrap())
        );
        println!(
            "resume run {run}: {result} sha256={:x}",
            Sha256::digest(&data)
        );
    }
}

#[test]
fn source_mutation_during_transfer_never_replaces_destination() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    init(&a);
    init(&b);
    let aid = trust(&b, &a);
    let bid = trust(&a, &b);
    let source = tmp.path().join("source");
    let target = tmp.path().join("target");
    fs::write(&source, vec![1; 3 * 1024 * 1024]).unwrap();
    fs::write(&target, b"old").unwrap();
    let mut srv = server(&b, &aid, &target);
    let mut sender = Command::new(env!("CARGO_BIN_EXE_everywhere"))
        .args([
            "send",
            "--state",
            s(&a),
            "--peer",
            &bid,
            "--addr",
            &srv.addr,
            "--source",
            s(&source),
            "--block-delay-ms",
            "200",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut progress = BufReader::new(sender.stderr.take().unwrap());
    let mut line = String::new();
    progress.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "BLOCK 0");
    fs::write(&source, vec![2; 3 * 1024 * 1024]).unwrap();
    assert!(!sender.wait().unwrap().success());
    assert!(!srv.child.wait().unwrap().success());
    assert_eq!(fs::read(&target).unwrap(), b"old");
}

#[test]
fn receiver_revocation_mid_transfer_preserves_old_file() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    init(&a);
    init(&b);
    let aid = trust(&b, &a);
    let bid = trust(&a, &b);
    let source = tmp.path().join("source");
    let target = tmp.path().join("target");
    fs::write(&source, vec![4; 3 * 1024 * 1024]).unwrap();
    fs::write(&target, b"old").unwrap();
    let mut srv = server(&b, &aid, &target);
    let mut sender = Command::new(env!("CARGO_BIN_EXE_everywhere"))
        .args([
            "send",
            "--state",
            s(&a),
            "--peer",
            &bid,
            "--addr",
            &srv.addr,
            "--source",
            s(&source),
            "--block-delay-ms",
            "200",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut progress = BufReader::new(sender.stderr.take().unwrap());
    let mut line = String::new();
    progress.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "BLOCK 0");
    ok(&["revoke", "--state", s(&b), "--peer", &aid]);
    assert!(!sender.wait().unwrap().success());
    assert!(!srv.child.wait().unwrap().success());
    assert_eq!(fs::read(&target).unwrap(), b"old");
}

#[cfg(unix)]
#[test]
fn readonly_partial_file_preserves_original() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    init(&a);
    init(&b);
    let aid = trust(&b, &a);
    let bid = trust(&a, &b);
    let source = tmp.path().join("source");
    let target = tmp.path().join("target");
    fs::write(&source, b"incoming").unwrap();
    fs::write(&target, b"old").unwrap();
    let manifest = everywhere::storage::Manifest::from_path(&source).unwrap();
    drop(everywhere::storage::Receiver::open(&target, manifest).unwrap());
    let partial = fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "part"))
        .unwrap();
    fs::set_permissions(&partial, fs::Permissions::from_mode(0o400)).unwrap();
    let mut srv = server(&b, &aid, &target);
    assert!(
        !cli(&[
            "send",
            "--state",
            s(&a),
            "--peer",
            &bid,
            "--addr",
            &srv.addr,
            "--source",
            s(&source)
        ])
        .status
        .success()
    );
    assert!(!srv.child.wait().unwrap().success());
    assert_eq!(fs::read(&target).unwrap(), b"old");
    fs::set_permissions(&partial, fs::Permissions::from_mode(0o600)).unwrap();
}
