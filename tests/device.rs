use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_everywhere"))
        .args(args)
        .output()
        .unwrap()
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
fn encrypted_capture_authenticates_and_refuses_replacement() {
    let base = tempfile::tempdir().unwrap();
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
    let key = base.path().join("offline-key.txt");
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
    let wrong = base.path().join("wrong-key.txt");
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
    let base = tempfile::tempdir().unwrap();
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
    let key = base.path().join("key");
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
