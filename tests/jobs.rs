use everywhere::identity;
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;
fn run(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_everywhere"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
struct Process {
    child: Child,
    address: String,
}
impl Process {
    fn start(args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_everywhere"))
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        let mut process = Self {
            child,
            address: String::new(),
        };
        let first = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("listener did not start");
        process.address = first.trim().strip_prefix("LISTEN ").unwrap().into();
        process
    }
    fn api(&self, token: &str, body: Option<Value>) -> Value {
        let mut stream = TcpStream::connect(&self.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        let (method, path) = if body.is_some() {
            ("POST", "/api/command")
        } else {
            ("GET", "/api/status")
        };
        let body = body.map(|v| v.to_string()).unwrap_or_default();
        write!(stream,"{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",self.address,body.len()).unwrap();
        let mut result = String::new();
        stream.read_to_string(&mut result).unwrap();
        assert!(result.starts_with("HTTP/1.1 200"), "{result}");
        serde_json::from_str(result.split("\r\n\r\n").nth(1).unwrap()).unwrap()
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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
        run(&[
            "share-init",
            "--state",
            state.to_str().unwrap(),
            "--folder",
            "personal",
            "--root",
            root.to_str().unwrap(),
        ]);
        Self { state, root, id }
    }
    fn allow(&self, other: &Self) {
        identity::trust(&self.state, &other.state.join("identity.der")).unwrap();
        run(&[
            "share-peer",
            "--state",
            self.state.to_str().unwrap(),
            "--folder",
            "personal",
            "--peer",
            &other.id,
        ]);
    }
}
fn wait_content(path: &Path, expected: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if fs::read(path).is_ok_and(|v| v == expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "managed job did not deliver expected bytes"
        );
        thread::sleep(Duration::from_millis(50));
    }
}
#[test]
fn managed_job_transfers_pauses_and_preserves_pause_across_manager_restart() {
    let tmp = TempDir::new().unwrap();
    let a = Node::new(tmp.path(), "a");
    let b = Node::new(tmp.path(), "b");
    a.allow(&b);
    b.allow(&a);
    fs::write(b.root.join("note"), b"from background job").unwrap();
    let server = Process::start(&[
        "sync-serve",
        "--state",
        b.state.to_str().unwrap(),
        "--folder",
        "personal",
        "--peer",
        &a.id,
        "--listen",
        "127.0.0.1:0",
    ]);
    let token = run(&["management-token", "--state", a.state.to_str().unwrap()]);
    let manager_args = [
        "manage",
        "--state",
        a.state.to_str().unwrap(),
        "--listen",
        "127.0.0.1:0",
    ];
    let manager = Process::start(&manager_args);
    let config = json!({"id":"personal-pull","folder":"personal","peer":b.id,"address":server.address,"direction":"connect","enabled":true});
    manager.api(&token, Some(json!({"action":"save-job","job":config})));
    wait_content(&a.root.join("note"), b"from background job");
    manager.api(
        &token,
        Some(json!({"action":"set-job-enabled","id":"personal-pull","enabled":false})),
    );
    assert_eq!(manager.api(&token, None)["jobs"][0]["running"], false);
    drop(manager);
    let manager = Process::start(&manager_args);
    let status = manager.api(&token, None);
    assert_eq!(status["jobs"][0]["enabled"], false);
    assert_eq!(status["jobs"][0]["running"], false);
    fs::write(b.root.join("note"), b"after resume").unwrap();
    manager.api(
        &token,
        Some(json!({"action":"set-job-enabled","id":"personal-pull","enabled":true})),
    );
    wait_content(&a.root.join("note"), b"after resume");
    manager.api(
        &token,
        Some(json!({"action":"set-job-enabled","id":"personal-pull","enabled":false})),
    );
}
