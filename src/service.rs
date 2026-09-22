//! Per-user deployment state and a verified, parent-owned manager bootstrap.
use crate::{identity, root::random_id};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};
mod native;

#[derive(Clone, Copy)]
pub enum Action {
    Start,
    Stop,
    Restart,
    Status,
    Uninstall,
}

fn wait_ready(state: &Path, expected: bool) -> Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    loop {
        let status = local_status(state)?;
        if (expected && status["healthy"] == true)
            || (!expected && status["runner_live"] == false && status["manager_lock_held"] == false)
        {
            return Ok(());
        }
        ensure!(
            std::time::Instant::now() < deadline,
            "service did not reach the requested state; inspect {}",
            state.join("service/output.log").display()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn verify_bootstrap(config: &Config) -> Result<()> {
    let version = config
        .bootstrap
        .parent()
        .and_then(Path::file_name)
        .and_then(|v| v.to_str())
        .context("invalid bootstrap version")?;
    installed_binary(&config.prefix, version)?;
    selected(&config.prefix)?;
    Ok(())
}
fn report(config: &Config) -> Result<Value> {
    let mut result = local_status(&config.state)?;
    result["installed"] = json!(true);
    result["native"] = native::status(config)?;
    Ok(result)
}
pub fn install(state: &Path, prefix: &Path, listen: SocketAddr) -> Result<Value> {
    let state = state.canonicalize()?;
    let _lifecycle = lock(&state.join("service.lock"))?;
    let config = configure_locked(&state, prefix, listen)?;
    verify_bootstrap(&config)?;
    native::install(&config)?;
    native::start(&config)?;
    wait_ready(&config.state, true)?;
    report(&config)
}
pub fn control(state: &Path, action: Action) -> Result<Value> {
    let state = state.canonicalize()?;
    let _lifecycle = lock(&state.join("service.lock"))?;
    if !exists(&state.join("service/config.json"))? {
        ensure!(
            matches!(action, Action::Status | Action::Uninstall),
            "service is not installed"
        );
        return Ok(json!({"installed":false,"state":state}));
    }
    let config = load(&state)?;
    match action {
        Action::Start => {
            verify_bootstrap(&config)?;
            native::start(&config)?;
            wait_ready(&state, true)?;
        }
        Action::Stop => {
            native::stop(&config)?;
            wait_ready(&state, false)?;
        }
        Action::Restart => {
            verify_bootstrap(&config)?;
            native::stop(&config)?;
            wait_ready(&state, false)?;
            native::start(&config)?;
            wait_ready(&state, true)?;
        }
        Action::Uninstall => {
            native::uninstall(&config)?;
            wait_ready(&state, false)?;
            fs::remove_file(state.join("service/config.json"))?;
            sync_dir(&state.join("service"))?;
            return Ok(json!({"installed":false,"state":state}));
        }
        Action::Status => {}
    }
    report(&config)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub format: u32,
    pub id: String,
    pub state: PathBuf,
    pub device: String,
    pub prefix: PathBuf,
    pub bootstrap: PathBuf,
    pub listen: SocketAddr,
}
fn regular(path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "expected a non-link regular file: {}",
        path.display()
    );
    Ok(())
}
fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
fn directory(path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_dir(),
        "expected a non-link directory: {}",
        path.display()
    );
    Ok(())
}
fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn private_dir(path: &Path) -> Result<()> {
    if exists(path)? {
        return directory(path);
    }
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = builder;
        builder.mode(0o700);
        builder
    };
    builder.create(path)?;
    sync_dir(path.parent().context("directory needs a parent")?)
}
fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}
fn lock(path: &Path) -> Result<File> {
    if exists(path)? {
        regular(path)?;
    }
    let file = private_options().create(true).truncate(false).open(path)?;
    file.try_lock_exclusive()
        .with_context(|| format!("already running or busy: {}", path.display()))?;
    Ok(file)
}
fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    regular(path)?;
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "oversized service file: {}",
        path.display()
    );
    Ok(bytes)
}
fn text(path: &Path, limit: u64) -> Result<String> {
    Ok(String::from_utf8(read(path, limit)?)?)
}
fn publish(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    if exists(path)? {
        regular(path)?;
    }
    let temporary = path.with_file_name(format!("service-{}.tmp", random_id()?));
    let mut file = private_options().create_new(true).open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    if replace {
        fs::rename(&temporary, path)?;
    } else {
        fs::hard_link(&temporary, path)?;
        fs::remove_file(&temporary)?;
    }
    sync_dir(path.parent().unwrap())
}
fn valid_path(path: &Path) -> Result<()> {
    let value = path.to_str().context("deployment paths must be Unicode")?;
    ensure!(
        path.is_absolute() && !value.chars().any(char::is_control),
        "invalid deployment path"
    );
    Ok(())
}
fn binary_name() -> &'static str {
    if cfg!(windows) {
        "everywhere.exe"
    } else {
        "everywhere"
    }
}
fn hash_file(path: &Path) -> Result<String> {
    regular(path)?;
    let mut source = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn installed_binary(prefix: &Path, version: &str) -> Result<PathBuf> {
    ensure!(
        !version.is_empty()
            && version.len() <= 160
            && !version.starts_with('.')
            && version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "invalid installation pointer"
    );
    let (label, expected) = version
        .rsplit_once('-')
        .context("installation pointer has no checksum")?;
    ensure!(
        !label.is_empty()
            && expected.len() == 64
            && expected
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid installation checksum"
    );
    directory(prefix)?;
    directory(&prefix.join("versions"))?;
    let version_dir = prefix.join("versions").join(version);
    directory(&version_dir)?;
    let binary = version_dir.join(binary_name());
    ensure!(
        hash_file(&binary)? == expected,
        "installed binary checksum mismatch"
    );
    Ok(binary)
}
fn selected(prefix: &Path) -> Result<PathBuf> {
    ensure!(
        text(&prefix.join(".everywhere-install"), 128)?.trim() == "everywhere-install-v1",
        "unknown installation format"
    );
    installed_binary(prefix, text(&prefix.join("current"), 256)?.trim())
}
fn binding(config: &Config, state: &Path) -> Result<()> {
    ensure!(
        config.format == 1 && config.state == state,
        "service state binding mismatch"
    );
    ensure!(
        config.listen.ip().is_loopback() && config.listen.port() != 0,
        "service requires a fixed loopback address"
    );
    valid_path(&config.state)?;
    valid_path(&config.prefix)?;
    valid_path(&config.bootstrap)?;
    ensure!(
        !config.state.starts_with(&config.prefix) && !config.prefix.starts_with(&config.state),
        "state and installation must not overlap"
    );
    ensure!(
        identity::fingerprint(&read(&state.join("identity.der"), 64 * 1024)?) == config.device,
        "service device identity changed"
    );
    let expected = service_id(state, &config.device)?;
    ensure!(config.id == expected, "invalid service identifier");
    let version = config
        .bootstrap
        .parent()
        .and_then(Path::file_name)
        .and_then(|v| v.to_str())
        .context("invalid bootstrap path")?;
    ensure!(
        config.bootstrap
            == config
                .prefix
                .join("versions")
                .join(version)
                .join(binary_name()),
        "bootstrap is outside its installation"
    );
    Ok(())
}
fn service_id(state: &Path, device: &str) -> Result<String> {
    let mut hash = blake3::Hasher::new();
    hash.update(state.to_str().context("non-Unicode state path")?.as_bytes());
    hash.update(b"\0");
    hash.update(device.as_bytes());
    Ok(format!(
        "happy-everywhere-{}",
        &hash.finalize().to_hex()[..32]
    ))
}
fn load(state: &Path) -> Result<Config> {
    let state = state.canonicalize()?;
    directory(&state.join("service"))?;
    let config: Config =
        serde_json::from_slice(&read(&state.join("service/config.json"), 64 * 1024)?)?;
    binding(&config, &state)?;
    Ok(config)
}
/// Prepare immutable deployment state; the native installer calls this before registration.
pub fn configure(state: &Path, prefix: &Path, listen: SocketAddr) -> Result<Config> {
    let state = state.canonicalize()?;
    let _lifecycle = lock(&state.join("service.lock"))?;
    configure_locked(&state, prefix, listen)
}
// Caller retains service.lock across configuration AND native registration.
fn configure_locked(state: &Path, prefix: &Path, listen: SocketAddr) -> Result<Config> {
    ensure!(
        listen.ip().is_loopback() && listen.port() != 0,
        "service requires a fixed loopback address"
    );
    let state = state.canonicalize()?;
    let prefix = prefix.canonicalize()?;
    valid_path(&state)?;
    valid_path(&prefix)?;
    ensure!(
        !state.starts_with(&prefix) && !prefix.starts_with(&state),
        "state and installation must not overlap"
    );
    let device = identity::fingerprint(&read(&state.join("identity.der"), 64 * 1024)?);
    regular(&state.join("identity.key.der"))?;
    let bootstrap = selected(&prefix)?;
    let config = Config {
        format: 1,
        id: service_id(&state, &device)?,
        state: state.clone(),
        device,
        prefix,
        bootstrap,
        listen,
    };
    private_dir(&state.join("service"))?;
    let path = state.join("service/config.json");
    if exists(&path)? {
        let existing = load(&state)?;
        // Upgrading the selected binary does not replace the retained bootstrap.
        ensure!(
            existing.state == config.state
                && existing.device == config.device
                && existing.prefix == config.prefix
                && existing.listen == config.listen,
            "service already configured differently; uninstall it before changing deployment settings"
        );
        return Ok(existing);
    }
    let _manager = lock(&state.join("management.lock"))?;
    crate::management::token(&state)?;
    publish(&path, &serde_json::to_vec_pretty(&config)?, false)?;
    Ok(config)
}
struct ManagedChild(Child);
impl Drop for ManagedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn logfile(state: &Path) -> Result<File> {
    let path = state.join("service/output.log");
    if exists(&path)? {
        regular(&path)?;
        if fs::metadata(&path)?.len() > 1024 * 1024 {
            let previous = state.join("service/output.previous.log");
            if exists(&previous)? {
                regular(&previous)?;
            }
            fs::rename(&path, previous)?;
            sync_dir(&state.join("service"))?;
        }
    }
    Ok(private_options().create(true).append(true).open(path)?)
}
/// Native service managers execute a retained bootstrap; manager updates follow `current`.
pub fn run(state: &Path) -> Result<()> {
    let config = load(state)?;
    let _runner = lock(&config.state.join("service/run.lock"))?;
    let available = lock(&config.state.join("management.lock"))?;
    let executable = selected(&config.prefix)?;
    let log = logfile(&config.state)?;
    drop(available); // The manager takes its own lifetime lock before binding.
    let mut child = ManagedChild(
        Command::new(&executable)
            .arg("manage")
            .arg("--state")
            .arg(&config.state)
            .arg("--listen")
            .arg(config.listen.to_string())
            .arg("--parent-watch")
            .stdin(Stdio::piped())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?,
    );
    publish(
        &config.state.join("service/run.json"),
        &serde_json::to_vec_pretty(
            &json!({"runner_pid":std::process::id(),"manager_pid":child.0.id(),"selected_binary":executable}),
        )?,
        true,
    )?;
    // Child::wait closes its stored stdin. Keep the liveness pipe separately
    // until the manager exits so waiting does not look like bootstrap death.
    let _parent_pipe = child
        .0
        .stdin
        .take()
        .context("missing manager liveness pipe")?;
    let result = child.0.wait()?;
    anyhow::bail!("managed process exited ({result}); native service supervision may restart it")
}
fn runner_live(state: &Path) -> Result<bool> {
    lock_held(&state.join("service/run.lock"))
}
fn lock_held(path: &Path) -> Result<bool> {
    if !exists(path)? {
        return Ok(false);
    }
    regular(path)?;
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(false),
        Err(e) if e.raw_os_error() == fs2::lock_contended_error().raw_os_error() => Ok(true),
        Err(e) => Err(e.into()),
    }
}
fn health(config: &Config) -> Result<Value> {
    let token = text(&config.state.join("management.token"), 64)?;
    identity::valid_peer(&token)?;
    let mut socket = TcpStream::connect_timeout(&config.listen, Duration::from_millis(300))?;
    socket.set_read_timeout(Some(Duration::from_secs(1)))?;
    socket.set_write_timeout(Some(Duration::from_secs(1)))?;
    write!(
        socket,
        "GET /api/health HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n",
        config.listen
    )?;
    let mut response = String::new();
    socket.take(16 * 1024).read_to_string(&mut response)?;
    ensure!(
        response.starts_with("HTTP/1.1 200 "),
        "manager health request rejected"
    );
    let (_, body) = response
        .split_once("\r\n\r\n")
        .context("incomplete health response")?;
    let value: Value = serde_json::from_str(body)?;
    ensure!(
        value["device"] == config.device && value["status"] == "ok",
        "manager identity mismatch"
    );
    Ok(value)
}
/// Local ownership/health is distinct from native registration and enabled state.
pub fn local_status(state: &Path) -> Result<Value> {
    let config = load(state)?;
    let live = runner_live(&config.state)?;
    let record = read(&config.state.join("service/run.json"), 8192)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let response = health(&config).ok();
    let manager_pid = response.as_ref().and_then(|v| v["pid"].as_u64());
    let recorded = record.as_ref().and_then(|v| v["manager_pid"].as_u64());
    Ok(
        json!({"id":config.id,"state":config.state,"listen":config.listen,"runner_live":live,"manager_lock_held":lock_held(&config.state.join("management.lock"))?,"manager_pid":manager_pid,"healthy":live && manager_pid.is_some() && manager_pid==recorded,"log":config.state.join("service/output.log")}),
    )
}
