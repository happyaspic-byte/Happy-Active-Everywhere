//! Central fleet control. File content still travels over the existing peer protocol.
mod agent;
mod store;
mod tls;
mod web;

pub use agent::enroll;
pub(crate) use agent::run as run_agent;
pub(crate) use web::serve;

use crate::{identity, management::Operation, root::random_id, wire::Timing};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::SocketAddr,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const PROTOCOL: &[u8] = b"everywhere-central/1";
fn timing() -> Timing {
    Timing {
        io: Duration::from_secs(15),
        operation: Duration::from_secs(30),
        frame: 2 * 1024 * 1024,
        ..Timing::default()
    }
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn secret_hash(value: &str) -> String {
    blake3::hash(value.as_bytes()).to_hex().to_string()
}
fn read_private(path: &Path) -> Result<Vec<u8>> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "expected a regular configuration file"
    );
    let mut bytes = Vec::new();
    File::open(path)?
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 256 * 1024, "configuration too large");
    Ok(bytes)
}
fn write_private(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_file_name(format!("central-{}.tmp", random_id()?));
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    drop(file);
    // Publish a complete credential/configuration without replacing an existing
    // enrollment. A crash may leave only our harmless temporary file.
    fs::hard_link(&temporary, path)?;
    fs::remove_file(temporary)?;
    #[cfg(unix)]
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Invitation {
    format: u32,
    address: SocketAddr,
    certificate: Vec<u8>,
    token: String,
    expires: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServerConfig {
    control_listen: SocketAddr,
    public_origin: Option<String>,
}
pub fn configure(
    state: &Path,
    control_listen: SocketAddr,
    public_origin: Option<String>,
) -> Result<()> {
    ensure!(control_listen.port() != 0, "use a fixed control port");
    if let Some(origin) = &public_origin {
        let host = origin.strip_prefix("https://").unwrap_or_default();
        ensure!(
            !host.is_empty()
                && !host.contains(['/', '?', '#', '@'])
                && !host.chars().any(char::is_whitespace),
            "public origin must be https://HOST[:PORT] without a path"
        );
    }
    if !state.try_exists()? {
        identity::init(state)?;
    }
    ensure!(
        !state.join("central-agent.json").try_exists()? && !state.join("shares").try_exists()?,
        "central server requires a separate state directory"
    );
    let path = state.join("central-server.json");
    let config = ServerConfig {
        control_listen,
        public_origin,
    };
    if path.try_exists()? {
        let existing: ServerConfig = serde_json::from_slice(&read_private(&path)?)?;
        ensure!(
            serde_json::to_value(existing)? == serde_json::to_value(&config)?,
            "central server already configured differently"
        );
    } else {
        write_private(&path, &config)?;
    }
    crate::management::token(state)?;
    Ok(())
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    id: String,
    operation: Operation,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ResultState {
    Completed,
    Failed,
    Uncertain,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandResult {
    id: String,
    status: ResultState,
    value: Value,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Request {
    Enroll {
        invitation: String,
        credential: String,
        name: String,
        certificate: Vec<u8>,
    },
    Poll {
        device: String,
        credential: String,
        report: Value,
        results: Vec<CommandResult>,
        active: Option<String>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    state: String,
    command: Option<Envelope>,
    acknowledged: Vec<String>,
    error: Option<String>,
}
impl Reply {
    fn state(state: &str) -> Self {
        Self {
            state: state.into(),
            command: None,
            acknowledged: vec![],
            error: None,
        }
    }
}
