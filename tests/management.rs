use everywhere::identity;
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};
use tempfile::TempDir;

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn request(
    address: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    origin: Option<&str>,
    body: &str,
) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let auth = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let origin = origin
        .map(|o| format!("Origin: {o}\r\n"))
        .unwrap_or_default();
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: {address}\r\n{auth}{origin}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}
#[test]
fn management_requires_token_and_same_origin_and_registers_a_real_folder() {
    let temporary = TempDir::new().unwrap();
    let state = temporary.path().join("state");
    let root = temporary.path().join("files");
    identity::init(&state).unwrap();
    fs::create_dir(&root).unwrap();
    let token = Command::new(env!("CARGO_BIN_EXE_everywhere"))
        .args(["management-token", "--state", state.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        token.status.success(),
        "{}",
        String::from_utf8_lossy(&token.stderr)
    );
    let token = String::from_utf8(token.stdout).unwrap();
    let token = token.trim();
    assert_eq!(token.len(), 64);
    let mut child = Command::new(env!("CARGO_BIN_EXE_everywhere"))
        .args([
            "manage",
            "--state",
            state.to_str().unwrap(),
            "--listen",
            "127.0.0.1:0",
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
    let server = Server(child);
    let line = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("management server did not listen");
    assert!(!line.contains(token));
    let address = line.trim().strip_prefix("LISTEN ").unwrap();
    let page = request(address, "GET", "/", None, None, "");
    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(page.contains("Content-Security-Policy") || page.contains("content-security-policy"));
    assert!(!page.contains(token));
    assert!(request(address, "GET", "/api/status", None, None, "").starts_with("HTTP/1.1 401"));
    assert!(
        request(address, "GET", "/api/status", Some("wrong"), None, "").starts_with("HTTP/1.1 401")
    );
    assert!(
        request(
            address,
            "GET",
            "/api/status",
            Some(token),
            Some("https://untrusted.example"),
            ""
        )
        .starts_with("HTTP/1.1 403")
    );
    let body = serde_json::json!({"action":"create-folder","folder":"photos","root":root,"mode":"bidirectional"}).to_string();
    let result = request(
        address,
        "POST",
        "/api/command",
        Some(token),
        Some(&format!("http://{address}")),
        &body,
    );
    assert!(result.starts_with("HTTP/1.1 200"), "{result}");
    assert!(root.join(".everywhere-folder").is_file());
    let response = request(address, "GET", "/api/status", Some(token), None, "");
    assert!(response.starts_with("HTTP/1.1 200"));
    let json: Value = serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(json["folders"][0]["folder"], "photos");
    assert_eq!(json["folders"][0]["files"], 0);
    drop(server);
    reader.join().unwrap();
}
