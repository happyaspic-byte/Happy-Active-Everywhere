use everywhere::{identity, service};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_everywhere")
}
fn command(args: &[&str], success: bool) -> String {
    let mut child = Command::new(binary())
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("command timed out: {args:?}");
        }
        thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.success(),
        success,
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
#[track_caller]
fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "condition timed out");
        thread::sleep(Duration::from_millis(50));
    }
}
fn address() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}
fn process_alive(pid: u64) -> bool {
    #[cfg(unix)]
    let result = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .unwrap();
    #[cfg(windows)]
    let result = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &format!("if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit 0 }} else {{ exit 1 }}")]).output().unwrap();
    result.status.success()
}
fn http(addr: SocketAddr, token: &str, path: &str) -> Option<Value> {
    let mut socket = TcpStream::connect_timeout(&addr, Duration::from_millis(150)).ok()?;
    socket.set_read_timeout(Some(Duration::from_secs(1))).ok()?;
    write!(socket,"GET {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n").ok()?;
    let mut data = String::new();
    socket.take(64 * 1024).read_to_string(&mut data).ok()?;
    if !data.starts_with("HTTP/1.1 200") {
        return None;
    }
    serde_json::from_str(data.split_once("\r\n\r\n")?.1).ok()
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
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
        command(
            &[
                "share-init",
                "--state",
                s(&state),
                "--root",
                s(&root),
                "--folder",
                "personal",
            ],
            true,
        );
        Self { state, root, id }
    }
    fn allow(&self, other: &Self) {
        identity::trust(&self.state, &other.state.join("identity.der")).unwrap();
        command(
            &[
                "share-peer",
                "--state",
                s(&self.state),
                "--folder",
                "personal",
                "--peer",
                &other.id,
            ],
            true,
        );
    }
}
fn installed(base: &Path) -> PathBuf {
    let prefix = base.join("installed α $HOME %USERNAME% 'quote'");
    fs::create_dir(&prefix).unwrap();
    fs::create_dir(prefix.join("versions")).unwrap();
    fs::write(
        prefix.join(".everywhere-install"),
        b"everywhere-install-v1\n",
    )
    .unwrap();
    select(&prefix, "first");
    prefix
}
fn select(prefix: &Path, label: &str) -> PathBuf {
    let hash = digest(&fs::read(binary()).unwrap());
    let version = format!("{label}-{hash}");
    let directory = prefix.join("versions").join(&version);
    fs::create_dir(&directory).unwrap();
    let selected = directory.join(if cfg!(windows) {
        "everywhere.exe"
    } else {
        "everywhere"
    });
    fs::copy(binary(), &selected).unwrap();
    fs::write(prefix.join("current"), version).unwrap();
    selected
}
fn evidence(test: impl FnOnce(&Path) + std::panic::UnwindSafe) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("folder-report/service-evidence");
    fs::create_dir_all(&dir).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("bootstrap-")
        .tempdir_in(dir)
        .unwrap();
    if let Err(error) = std::panic::catch_unwind(|| test(temp.path())) {
        eprintln!("Service evidence retained at {}", temp.keep().display());
        std::panic::resume_unwind(error);
    }
}

#[test]
fn runner_owns_manager_and_resumes_real_tls_jobs_after_version_switch() {
    evidence(|base| {
        let a = Node::new(base, "a");
        let b = Node::new(base, "b");
        a.allow(&b);
        b.allow(&a);
        let prefix = installed(base);
        let listen = address();
        let config = service::configure(&a.state, &prefix, listen).unwrap();
        let token = command(&["management-token", "--state", s(&a.state)], true)
            .trim()
            .to_string();
        let mut server = Process(
            Command::new(binary())
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
                .stderr(fs::File::create(base.join("peer.stderr.log")).unwrap())
                .spawn()
                .unwrap(),
        );
        let (tx, rx) = mpsc::channel();
        let output = server.0.stdout.take().unwrap();
        thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if tx.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        let ready = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let remote = ready.trim().strip_prefix("LISTEN ").unwrap();
        let job = json!({"id":"active","folder":"personal","peer":b.id,"address":remote,"direction":"connect","enabled":true});
        let mut paused = job.clone();
        paused["id"] = json!("paused");
        paused["enabled"] = json!(false);
        fs::write(
            a.state.join("jobs.json"),
            json!({"active":job,"paused":paused}).to_string(),
        )
        .unwrap();
        fs::write(b.root.join("note"), b"from real background peer").unwrap();
        let start = |parent_watch: bool| {
            let mut command = Command::new(&config.bootstrap);
            command.args(["service", "run", "--state", s(&a.state)]);
            if parent_watch {
                command.arg("--parent-watch").stdin(Stdio::piped());
            }
            Process(
                command
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            )
        };
        let runner = start(false);
        wait(|| http(listen, &token, "/api/health").is_some());
        assert!(http(listen, "wrong", "/api/health").is_none());
        let health = http(listen, &token, "/api/health").unwrap();
        assert_eq!(health["device"], a.id);
        let status = service::local_status(&a.state).unwrap();
        assert_eq!(status["healthy"], true);
        assert_eq!(status["manager_pid"], health["pid"]);
        command(&["service", "run", "--state", s(&a.state)], false);
        wait(|| {
            fs::read(a.root.join("note"))
                .is_ok_and(|bytes| digest(&bytes) == digest(b"from real background peer"))
        });
        // This restart case begins after a committed exchange. Merely seeing
        // published bytes can precede the DB acknowledgement; killing there
        // intentionally leaves edits concurrent (covered by the recovery tests).
        wait(|| {
            http(listen, &token, "/api/status").is_some_and(|status| {
                status["jobs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|job| job["id"] == "active" && job["last_success"].is_u64())
            })
        });
        let jobs = http(listen, &token, "/api/status").unwrap()["jobs"].clone();
        let worker_pid = jobs
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"] == "active")
            .unwrap()["pid"]
            .as_u64()
            .expect("running job has no process identity");
        assert!(process_alive(worker_pid));
        assert_eq!(
            jobs.as_array()
                .unwrap()
                .iter()
                .find(|j| j["id"] == "paused")
                .unwrap()["running"],
            false
        );
        drop(runner);
        wait(|| http(listen, &token, "/api/health").is_none());
        wait(|| !process_alive(worker_pid));
        wait(|| service::local_status(&a.state).is_ok_and(|s| s["runner_live"] == false));
        let selected = select(&prefix, "second");
        fs::write(a.root.join("note"), b"local edit while stopped").unwrap();
        let mut runner = start(true);
        wait(|| http(listen, &token, "/api/health").is_some());
        wait(|| {
            if let Some(status) = http(listen, &token, "/api/status") {
                fs::write(base.join("last-status.json"), status.to_string()).unwrap();
            }
            fs::read(b.root.join("note"))
                .is_ok_and(|bytes| digest(&bytes) == digest(b"local edit while stopped"))
        });
        let run: Value =
            serde_json::from_slice(&fs::read(a.state.join("service/run.json")).unwrap()).unwrap();
        assert_eq!(run["selected_binary"], s(&selected.canonicalize().unwrap()));
        assert_ne!(run["manager_pid"], health["pid"]);
        assert_eq!(
            http(listen, &token, "/api/status").unwrap()["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|j| j["id"] == "paused")
                .unwrap()["enabled"],
            false
        );
        // The Windows task wrapper owns this pipe. Closing it must terminate
        // the bootstrap as well as the actual manager and worker subtree.
        let worker_pid = http(listen, &token, "/api/status").unwrap()["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"] == "active")
            .unwrap()["pid"]
            .as_u64()
            .unwrap();
        drop(runner.0.stdin.take());
        wait(|| http(listen, &token, "/api/health").is_none());
        wait(|| runner.0.try_wait().unwrap().is_some());
        wait(|| !process_alive(worker_pid));
        assert!(a.state.join("identity.key.der").is_file());
        assert_eq!(
            digest(&fs::read(a.root.join("note")).unwrap()),
            digest(b"local edit while stopped")
        );
    });
}

#[test]
fn configuration_and_bootstrap_refuse_unsafe_deployments_without_replacement() {
    evidence(|base| {
        let a = Node::new(base, "a");
        let prefix = installed(base);
        for addr in ["0.0.0.0:7445", "127.0.0.1:0"] {
            assert!(service::configure(&a.state, &prefix, addr.parse().unwrap()).is_err());
        }
        assert!(!a.state.join("service/config.json").exists());
        let listen = address();
        let config = service::configure(&a.state, &prefix, listen).unwrap();
        let saved = fs::read(a.state.join("service/config.json")).unwrap();
        assert!(service::configure(&a.state, &prefix, address()).is_err());
        assert_eq!(
            fs::read(a.state.join("service/config.json")).unwrap(),
            saved
        );
        let current = fs::read(prefix.join("current")).unwrap();
        fs::write(prefix.join("current"), "../../outside").unwrap();
        command(&["service", "run", "--state", s(&a.state)], false);
        fs::write(prefix.join("current"), current).unwrap();
        fs::write(&config.bootstrap, b"corrupt executable").unwrap();
        command(&["service", "run", "--state", s(&a.state)], false);
        assert_eq!(
            fs::read(a.state.join("service/config.json")).unwrap(),
            saved
        );
        assert!(!a.state.join("service/run.json").exists());
        let b = Node::new(base, "b");
        let nested = installed(&b.state);
        assert!(service::configure(&b.state, &nested, address()).is_err());
    });
}

#[test]
fn a_foreground_manager_is_not_reported_as_an_owned_service() {
    evidence(|base| {
        let a = Node::new(base, "a");
        let prefix = installed(base);
        let listen = address();
        service::configure(&a.state, &prefix, listen).unwrap();
        let token = command(&["management-token", "--state", s(&a.state)], true)
            .trim()
            .to_string();
        let manager = Process(
            Command::new(binary())
                .args([
                    "manage",
                    "--state",
                    s(&a.state),
                    "--listen",
                    &listen.to_string(),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        wait(|| http(listen, &token, "/api/health").is_some());
        command(&["service", "run", "--state", s(&a.state)], false);
        assert_eq!(service::local_status(&a.state).unwrap()["healthy"], false);
        assert!(http(listen, &token, "/api/health").is_some());
        drop(manager);
    });
}

#[cfg(unix)]
#[test]
fn symlinked_service_manifest_never_changes_its_target() {
    evidence(|base| {
        let a = Node::new(base, "a");
        let prefix = installed(base);
        fs::create_dir(a.state.join("service")).unwrap();
        let outside = base.join("unrelated");
        fs::write(&outside, b"do not touch").unwrap();
        std::os::unix::fs::symlink(&outside, a.state.join("service/config.json")).unwrap();
        assert!(service::configure(&a.state, &prefix, address()).is_err());
        command(&["service", "run", "--state", s(&a.state)], false);
        assert_eq!(fs::read(outside).unwrap(), b"do not touch");
    });
}

#[cfg(unix)]
#[test]
fn unreadable_service_state_is_an_error_not_an_uninstalled_service() {
    use std::os::unix::fs::PermissionsExt;
    evidence(|base| {
        let a = Node::new(base, "a");
        let prefix = installed(base);
        service::configure(&a.state, &prefix, address()).unwrap();
        let directory = a.state.join("service");
        let original = fs::metadata(&directory).unwrap().permissions();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o0)).unwrap();
        let result = Command::new(binary())
            .args(["service", "status", "--state", s(&a.state)])
            .output();
        fs::set_permissions(&directory, original).unwrap();
        let result = result.unwrap();
        assert!(
            !result.status.success(),
            "unreadable state was reported uninstalled: {}",
            String::from_utf8_lossy(&result.stdout)
        );
        assert!(a.state.join("service/config.json").is_file());
    });
}

#[cfg(windows)]
#[test]
fn scheduler_query_errors_are_not_missing_registration() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(repo.join("scripts/verify-task-query.ps1"))
        .arg("-Helper")
        .arg(repo.join("src/service/task.ps1"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
