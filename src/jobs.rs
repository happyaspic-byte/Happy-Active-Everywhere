//! Persisted jobs run the same CLI engine in supervised child processes.
use crate::{identity, root::random_id, share};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Connect,
    Listen,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub id: String,
    pub folder: String,
    pub peer: String,
    pub address: SocketAddr,
    pub direction: Direction,
    pub enabled: bool,
}
impl Config {
    fn validate(&self) -> Result<()> {
        for id in [&self.id, &self.folder] {
            ensure!(
                !id.is_empty()
                    && id.len() <= 64
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                "invalid job or folder identifier"
            );
        }
        identity::valid_peer(&self.peer)?;
        ensure!(
            self.address.port() != 0,
            "managed jobs require an explicit nonzero port"
        );
        Ok(())
    }
}
#[derive(Default)]
struct Progress {
    last_message: String,
    last_success: Option<u64>,
}
struct Running {
    child: Child,
    readers: Vec<thread::JoinHandle<()>>,
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}
struct Inner {
    configs: BTreeMap<String, Config>,
    running: BTreeMap<String, Running>,
    progress: BTreeMap<String, Arc<Mutex<Progress>>>,
    retry: BTreeMap<String, Instant>,
}
pub struct Jobs {
    state: PathBuf,
    inner: Mutex<Inner>,
}
fn reader(
    mut input: impl Read + Send + 'static,
    progress: Arc<Mutex<Progress>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0; 4096];
        let mut line = Vec::new();
        loop {
            let count = match input.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => count,
            };
            for byte in &buffer[..count] {
                if *byte == b'\n' {
                    if let Ok(mut status) = progress.lock() {
                        status.last_message = String::from_utf8_lossy(&line).into_owned();
                        if serde_json::from_slice::<Value>(&line)
                            .is_ok_and(|v| v["status"] == "complete")
                        {
                            status.last_success = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .ok()
                                .map(|v| v.as_secs());
                        }
                    }
                    line.clear();
                } else if line.len() < 4096 {
                    line.push(*byte);
                }
            }
        }
    })
}
impl Jobs {
    pub fn open(state: &Path) -> Result<Self> {
        let path = state.join("jobs.json");
        let configs: BTreeMap<String, Config> = if path.try_exists()? {
            ensure!(
                fs::symlink_metadata(&path)?.file_type().is_file(),
                "unsafe job configuration"
            );
            let mut bytes = Vec::new();
            File::open(path)?
                .take(256 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= 256 * 1024, "job configuration too large");
            serde_json::from_slice(&bytes)?
        } else {
            BTreeMap::new()
        };
        ensure!(configs.len() <= 32, "too many managed jobs");
        for (id, config) in &configs {
            config.validate()?;
            ensure!(id == &config.id, "job identifier mismatch");
        }
        Ok(Self {
            state: state.into(),
            inner: Mutex::new(Inner {
                configs,
                running: BTreeMap::new(),
                progress: BTreeMap::new(),
                retry: BTreeMap::new(),
            }),
        })
    }
    fn persist(&self, configs: &BTreeMap<String, Config>) -> Result<()> {
        let temporary = self.state.join(format!("jobs-{}.tmp", random_id()?));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(configs)?)?;
        file.sync_all()?;
        fs::rename(temporary, self.state.join("jobs.json"))?;
        #[cfg(unix)]
        File::open(&self.state)?.sync_all()?;
        Ok(())
    }
    pub fn save(&self, config: Config) -> Result<()> {
        config.validate()?;
        identity::Identity::new(&self.state, &config.peer)?;
        share::check_access(&self.state, &config.folder, &config.peer, false)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("job supervisor unavailable"))?;
        let mut configs = inner.configs.clone();
        configs.insert(config.id.clone(), config.clone());
        ensure!(configs.len() <= 32, "too many managed jobs");
        self.persist(&configs)?;
        inner.running.remove(&config.id);
        inner.retry.remove(&config.id);
        inner.configs = configs;
        Ok(())
    }
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("job supervisor unavailable"))?;
        let mut configs = inner.configs.clone();
        let config = configs.get_mut(id).context("unknown job")?;
        if enabled {
            identity::Identity::new(&self.state, &config.peer)?;
            share::check_access(&self.state, &config.folder, &config.peer, false)?;
        }
        config.enabled = enabled;
        self.persist(&configs)?;
        inner.configs = configs;
        if !enabled {
            inner.running.remove(id);
        }
        inner.retry.remove(id);
        Ok(())
    }
    pub fn tick(&self) -> Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("job supervisor unavailable"))?;
        let configs: Vec<_> = inner.configs.values().cloned().collect();
        for config in configs {
            let ended = inner
                .running
                .get_mut(&config.id)
                .map(|r| r.child.try_wait())
                .transpose()?
                .flatten()
                .is_some();
            if ended {
                inner.running.remove(&config.id);
                inner
                    .retry
                    .insert(config.id.clone(), Instant::now() + Duration::from_secs(5));
            }
            if !config.enabled
                || inner.running.contains_key(&config.id)
                || inner
                    .retry
                    .get(&config.id)
                    .is_some_and(|t| *t > Instant::now())
            {
                continue;
            }
            let progress = inner
                .progress
                .entry(config.id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(Progress::default())))
                .clone();
            let result = (|| -> Result<Running> {
                identity::Identity::new(&self.state, &config.peer)?;
                share::check_access(&self.state, &config.folder, &config.peer, false)?;
                let mut command = Command::new(std::env::current_exe()?);
                match config.direction {
                    Direction::Connect => {
                        command
                            .arg("sync")
                            .arg("--addr")
                            .arg(config.address.to_string())
                            .arg("--continuous");
                    }
                    Direction::Listen => {
                        command
                            .arg("sync-serve")
                            .arg("--listen")
                            .arg(config.address.to_string());
                    }
                }
                let mut child = command
                    .arg("--state")
                    .arg(&self.state)
                    .arg("--folder")
                    .arg(&config.folder)
                    .arg("--peer")
                    .arg(&config.peer)
                    .arg("--parent-watch")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()?;
                let readers = vec![
                    reader(child.stdout.take().unwrap(), progress.clone()),
                    reader(child.stderr.take().unwrap(), progress.clone()),
                ];
                Ok(Running { child, readers })
            })();
            match result {
                Ok(running) => {
                    inner.running.insert(config.id, running);
                }
                Err(error) => {
                    if let Ok(mut status) = progress.lock() {
                        status.last_message = format!("{error:#}");
                    }
                    inner
                        .retry
                        .insert(config.id, Instant::now() + Duration::from_secs(5));
                }
            }
        }
        Ok(())
    }
    pub fn status(&self) -> Result<Value> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("job supervisor unavailable"))?;
        let mut result = Vec::new();
        for config in inner.configs.values() {
            let mut value = serde_json::to_value(config)?;
            value["running"] = json!(inner.running.contains_key(&config.id));
            value["pid"] = json!(inner.running.get(&config.id).map(|r| r.child.id()));
            if let Some(status) = inner.progress.get(&config.id) {
                let status = status
                    .lock()
                    .map_err(|_| anyhow::anyhow!("job progress unavailable"))?;
                value["last_message"] = json!(status.last_message);
                value["last_success"] = json!(status.last_success);
            }
            result.push(value);
        }
        Ok(Value::Array(result))
    }
    pub fn shutdown(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.running.clear();
        }
    }
}
/// EOF on the supervisor-owned pipe also stops children after a manager crash.
pub fn watch_parent() {
    thread::spawn(|| {
        let mut byte = [0];
        loop {
            match std::io::stdin().read(&mut byte) {
                Ok(0) | Err(_) => std::process::exit(0),
                Ok(_) => {}
            }
        }
    });
}
