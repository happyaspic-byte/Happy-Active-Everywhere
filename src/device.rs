//! Offline encrypted device snapshots. Restored authority is never implicit.
use crate::{
    identity, jobs,
    share::{self, Share},
};
use age::secrecy::ExposeSecret;
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zeroize::{Zeroize, Zeroizing};

mod archive;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    format: u32,
    certificate: Vec<u8>,
    private_key: Vec<u8>,
    peers: BTreeMap<String, Vec<u8>>,
    jobs: BTreeMap<String, jobs::Config>,
    folders: BTreeSet<String>,
}
impl Drop for Header {
    fn drop(&mut self) {
        self.private_key.zeroize();
    }
}
impl Header {
    fn validate(&self) -> Result<()> {
        ensure!(self.format == 1, "unsupported device archive format");
        ensure!(
            self.folders.len() <= 128 && self.peers.len() <= 128 && self.jobs.len() <= 32,
            "device archive exceeds configuration limits"
        );
        ensure!(
            self.certificate.len() <= 65536 && self.private_key.len() <= 65536,
            "identity too large"
        );
        // Validate that the saved certificate and signing key match.
        rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(
            vec![self.certificate.clone().into()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(self.private_key.clone()).into(),
        )?;
        for id in &self.folders {
            share::valid_id(id)?;
        }
        for (id, cert) in &self.peers {
            identity::valid_peer(id)?;
            ensure!(
                cert.len() <= 65536 && identity::fingerprint(cert) == *id,
                "invalid historical peer certificate"
            );
            rustls::RootCertStore::empty().add(cert.clone().into())?;
        }
        for (id, config) in &self.jobs {
            config.validate()?;
            ensure!(
                id == &config.id && self.folders.contains(&config.folder),
                "invalid historical job binding"
            );
        }
        Ok(())
    }
    fn summary(&self, staging: &Path) -> Result<Value> {
        self.validate()?;
        let mut folders = Vec::new();
        for id in &self.folders {
            let config = share::backup::verify(&staging.join("folders").join(id))?;
            ensure!(
                config.id == *id && config.identity == identity::fingerprint(&self.certificate),
                "folder identity mismatch"
            );
            identity::valid_peer(&config.marker)?;
            for peer in &config.peers {
                identity::valid_peer(peer)?;
            }
            ensure!(config.peers.len() <= 128, "too many historical grants");
            folders.push(config);
        }
        Ok(
            json!({"format":self.format,"identity":identity::fingerprint(&self.certificate),
            "folders":folders,"historical_peers":self.peers.keys().collect::<Vec<_>>(),
            "historical_jobs":self.jobs}),
        )
    }
}

fn regular(path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "expected a regular file"
    );
    Ok(())
}
fn bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    regular(path)?;
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "device metadata too large");
    Ok(bytes)
}
fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = private_file(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    sync_dir(path.parent().context("missing parent")?)
}
fn private_dir(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    sync_dir(path.parent().context("missing parent")?)
}
fn new_output(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()?;
    let result = parent.join(path.file_name().context("output requires a new name")?);
    match fs::symlink_metadata(&result) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(result),
        Err(e) => Err(e.into()),
        Ok(_) => anyhow::bail!("output already exists; it will never be replaced"),
    }
}
fn lock(state: &Path, name: &str, exclusive: bool) -> Result<File> {
    ensure!(
        fs::symlink_metadata(state)?.file_type().is_dir(),
        "state must be a real directory"
    );
    let path = state.join(name);
    match fs::symlink_metadata(&path) {
        Ok(m) => ensure!(m.file_type().is_file(), "unsafe device lock"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    if exclusive {
        FileExt::try_lock_exclusive(&file)
    } else {
        FileExt::try_lock_shared(&file)
    }
    .context("device is busy; stop its service/operations and retry")?;
    Ok(file)
}
pub(crate) fn config_guard(state: &Path) -> Result<File> {
    lock(state, "device.lock", false)
}

pub fn keygen(output: &Path) -> Result<Value> {
    let output = new_output(output)?;
    let key = age::x25519::Identity::generate();
    write_private(&output, key.to_string().expose_secret().as_bytes())?;
    Ok(json!({"recipient":key.to_public().to_string(),"key_file":output}))
}
pub fn backup(state: &Path, recipient: &str, output: &Path) -> Result<Value> {
    let recipient: age::x25519::Recipient = recipient
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid age X25519 recipient"))?;
    let state = state.canonicalize()?;
    let output = new_output(output)?;
    ensure!(
        !output.starts_with(&state),
        "backup must be outside device state"
    );
    let _service = lock(&state, "service.lock", true)?;
    let _manager = lock(&state, "management.lock", true)?;
    let _config = lock(&state, "device.lock", true)?;
    let mut header = Header {
        format: 1,
        certificate: bounded(&state.join("identity.der"), 65536)?,
        private_key: bounded(&state.join("identity.key.der"), 65536)?,
        peers: BTreeMap::new(),
        jobs: BTreeMap::new(),
        folders: BTreeSet::new(),
    };
    for entry in fs::read_dir(state.join("peers"))? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("invalid peer filename"))?;
        let id = name.strip_suffix(".der").context("unexpected peer file")?;
        identity::valid_peer(id)?;
        ensure!(header.peers.len() < 128, "too many peers");
        header
            .peers
            .insert(id.into(), bounded(&entry.path(), 65536)?);
    }
    if state.join("jobs.json").try_exists()? {
        header.jobs = serde_json::from_slice(&bounded(&state.join("jobs.json"), 256 * 1024)?)?;
    }
    if state.join("shares").try_exists()? {
        for entry in fs::read_dir(state.join("shares"))? {
            let entry = entry?;
            ensure!(entry.file_type()?.is_dir(), "unexpected share entry");
            let id = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("invalid share filename"))?;
            share::valid_id(&id)?;
            ensure!(header.folders.len() < 128, "too many folders");
            header.folders.insert(id);
        }
    }
    header.validate()?;
    let mut shares = Vec::new();
    for id in &header.folders {
        let share = Share::open(&state, id)?;
        ensure!(
            !output.starts_with(&share.config.root),
            "backup must be outside synchronized roots"
        );
        shares.push(share);
    }
    let staging = tempfile::Builder::new()
        .prefix(".everywhere-backup-")
        .tempdir_in(output.parent().unwrap())?;
    private_dir(&staging.path().join("folders"))?;
    for share in &shares {
        share.scan(false)?;
        share::backup::capture(
            share,
            &staging.path().join("folders").join(&share.config.id),
        )?;
    }
    header.summary(staging.path())?;
    archive::encode(&header, staging.path(), &recipient, &output)?;
    Ok(
        json!({"status":"device-backup-complete","output":output,"identity":identity::fingerprint(&header.certificate),"folders":header.folders.len()}),
    )
}
pub fn inspect(backup: &Path, key: &Path) -> Result<Value> {
    let (staging, header) = archive::decode(backup, key, None)?;
    header.summary(staging.path())
}

pub fn recover(
    backup: &Path,
    key: &Path,
    output: &Path,
    retired_device: Option<&str>,
) -> Result<Value> {
    let output = new_output(output)?;
    let parent = output.parent().unwrap();
    let (decoded, mut header) = archive::decode(backup, key, Some(parent))?;
    let summary = header.summary(decoded.path())?;
    let old_identity = identity::fingerprint(&header.certificate);
    if let Some(expected) = retired_device {
        ensure!(
            expected == old_identity,
            "retired device fingerprint does not match backup"
        );
    }
    let workspace = tempfile::Builder::new()
        .prefix(".everywhere-restore-")
        .tempdir_in(parent)?;
    let state = workspace.path().join("state");
    let identity = if retired_device.is_some() {
        private_dir(&state)?;
        private_dir(&state.join("peers"))?;
        write_private(&state.join("identity.der"), &header.certificate)?;
        write_private(&state.join("identity.key.der"), &header.private_key)?;
        old_identity
    } else {
        identity::init(&state)?
    };
    private_dir(&state.join("recovery-peers"))?;
    for (peer, cert) in &header.peers {
        write_private(
            &state.join("recovery-peers").join(format!("{peer}.der")),
            cert,
        )?;
    }
    for config in header.jobs.values_mut() {
        config.enabled = false;
    }
    write_private(
        &state.join("jobs.json"),
        &serde_json::to_vec_pretty(&header.jobs)?,
    )?;
    write_private(
        &state.join("device-recovery.json"),
        &serde_json::to_vec_pretty(&summary)?,
    )?;
    private_dir(&state.join("shares"))?;
    private_dir(&workspace.path().join("folders"))?;
    for id in &header.folders {
        share::backup::import(
            &decoded.path().join("folders").join(id),
            &state,
            &workspace.path().join("folders").join(id),
            &output.join("folders").join(id),
        )?;
    }
    sync_dir(&workspace.path().join("folders"))?;
    sync_dir(&state)?;
    sync_dir(workspace.path())?;
    archive::publish_directory(workspace.path(), &output)?;
    // The staging path no longer exists; TempDir never owns the published tree.
    sync_dir(parent)?;
    Ok(
        json!({"status":"device-recovered","identity":identity,"state":output.join("state"),
        "output":output,"folders":header.folders.len(),"quarantined":true}),
    )
}

pub fn activate(state: &Path, folder: &str, offline_authority: bool) -> Result<Value> {
    share::valid_id(folder)?;
    let report: Value = serde_json::from_slice(&bounded(
        &state.join("device-recovery.json"),
        4 * 1024 * 1024,
    )?)?;
    let folders = report["folders"]
        .as_array()
        .context("invalid recovery report")?;
    let config = folders
        .iter()
        .find(|c| c["id"] == folder)
        .context("folder was not recovered by this device operation")?;
    let config: share::Config = serde_json::from_value(config.clone())?;
    share::backup::activate_device(state, folder, config.mode, offline_authority)
}
