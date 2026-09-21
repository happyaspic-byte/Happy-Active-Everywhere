use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub const BLOCK_SIZE: usize = 1024 * 1024;
const MAX_SIZE: u64 = 1 << 40;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub size: u64,
    pub hash: String,
    pub blocks: Vec<String>,
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn regular(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(m) => {
            ensure!(
                m.file_type().is_file(),
                "not a regular file: {}",
                path.display()
            );
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
impl Manifest {
    pub fn from_path(path: &Path) -> Result<Self> {
        ensure!(regular(path)?, "source missing");
        let mut file = File::open(path)?;
        let before = file.metadata()?;
        let size = before.len();
        ensure!(size <= MAX_SIZE, "file exceeds 1 TiB alpha limit");
        let mut full = blake3::Hasher::new();
        let mut blocks = Vec::new();
        let mut buffer = vec![0; BLOCK_SIZE];
        let mut remaining = size;
        while remaining > 0 {
            let len = remaining.min(BLOCK_SIZE as u64) as usize;
            file.read_exact(&mut buffer[..len])?;
            full.update(&buffer[..len]);
            blocks.push(blake3::hash(&buffer[..len]).to_hex().to_string());
            remaining -= len as u64;
        }
        let after = file.metadata()?;
        ensure!(
            before.len() == after.len() && before.modified()? == after.modified()?,
            "source changed during hashing"
        );
        Ok(Self {
            size,
            hash: full.finalize().to_hex().to_string(),
            blocks,
        })
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.size <= MAX_SIZE, "file exceeds 1 TiB alpha limit");
        ensure!(
            self.blocks.len() as u64 == self.size.div_ceil(BLOCK_SIZE as u64),
            "block count mismatch"
        );
        ensure!(
            valid_hash(&self.hash) && self.blocks.iter().all(|h| valid_hash(h)),
            "invalid hash"
        );
        Ok(())
    }
}
pub fn block_len(manifest: &Manifest, index: u64) -> Result<usize> {
    ensure!(index < (manifest.blocks.len() as u64), "block out of range");
    Ok((manifest.size - index * BLOCK_SIZE as u64).min(BLOCK_SIZE as u64) as usize)
}
fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn open_rw(path: &Path) -> Result<File> {
    regular(path)?;
    let mut opts = OpenOptions::new();
    opts.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    Ok(opts.open(path)?)
}
fn snapshot(path: &Path) -> Result<Option<Manifest>> {
    if regular(path)? {
        Ok(Some(Manifest::from_path(path)?))
    } else {
        Ok(None)
    }
}

#[derive(Serialize, Deserialize)]
struct Journal {
    manifest: Manifest,
    original: Option<Manifest>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallIntent {
    recovery_id: String,
    original: Manifest,
    incoming: Manifest,
}

fn recovery_directory(parent: &Path) -> Result<PathBuf> {
    let path = parent.join(".everywhere-recovery");
    match fs::create_dir(&path) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
            }
            sync_dir(parent)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    ensure!(fs::symlink_metadata(&path)?.file_type().is_dir(), "unsafe recovery directory");
    Ok(path)
}

// Record intent before moving the old directory entry. Preserve the actual file,
// not only a byte snapshot: editors may still hold and write its open handle.
fn prepare_install(
    target: &Path,
    part: &Path,
    intent_path: &Path,
    original: &Manifest,
    incoming: &Manifest,
) -> Result<PathBuf> {
    let parent = target.parent().context("target has no parent")?;
    let recovery = recovery_directory(parent)?;
    let recovery_id = blake3::hash(&rcgen::KeyPair::generate()?.serialize_der()).to_hex().to_string();
    let directory = recovery.join(&recovery_id);
    fs::create_dir(&directory)?;
    // Also preflight hard-link support before touching the user's target.
    fs::hard_link(part, directory.join("incoming"))?;
    let intent = InstallIntent {
        recovery_id,
        original: original.clone(),
        incoming: incoming.clone(),
    };
    let mut record = OpenOptions::new().create_new(true).write(true).open(directory.join("record.json"))?;
    serde_json::to_writer(&mut record, &serde_json::json!({
        "target": target.file_name(), "intent": intent,
    }))?;
    record.sync_all()?;
    sync_dir(&directory)?;
    sync_dir(&recovery)?;
    let mut file = OpenOptions::new().create_new(true).write(true).open(intent_path)?;
    serde_json::to_writer(&mut file, &intent)?;
    file.sync_all()?;
    sync_dir(parent)?;
    let displaced = directory.join("previous");
    fs::rename(target, &displaced)?;
    sync_dir(&directory)?;
    sync_dir(parent)?;
    Ok(displaced)
}

fn recover_install(target: &Path, intent_path: &Path, incoming: &Manifest) -> Result<()> {
    if !regular(intent_path)? {
        return Ok(());
    }
    let intent: InstallIntent = serde_json::from_reader(File::open(intent_path)?.take(160 * 1024 * 1024))?;
    ensure!(valid_hash(&intent.recovery_id), "invalid recovery identifier");
    ensure!(intent.incoming == *incoming, "install intent manifest mismatch");
    intent.original.validate()?;
    let parent = target.parent().context("target has no parent")?;
    let recovery = recovery_directory(parent)?;
    let directory = recovery.join(&intent.recovery_id);
    ensure!(fs::symlink_metadata(&directory)?.file_type().is_dir(), "unsafe recovery entry");
    let displaced = directory.join("previous");
    if !regular(target)? {
        ensure!(regular(&displaced)?, "interrupted install has no recoverable original");
        // Atomic no-clobber restore. A newly created local file wins the race.
        match fs::hard_link(&displaced, target) {
            Ok(()) => sync_dir(parent)?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    let current = snapshot(target)?;
    ensure!(
        current.as_ref() == Some(&intent.original) || current.as_ref() == Some(incoming),
        "local change during interrupted install; target and recovery files preserved at {}",
        directory.display()
    );
    fs::remove_file(intent_path)?;
    sync_dir(parent)?;
    Ok(())
}

pub struct Receiver {
    target: PathBuf,
    journal_path: PathBuf,
    part_path: PathBuf,
    part: File,
    _lock: File,
    manifest: Manifest,
    original: Option<Manifest>,
    finished: bool,
}
impl Receiver {
    pub fn open(target: &Path, manifest: Manifest) -> Result<Self> {
        manifest.validate()?;
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = parent.canonicalize()?;
        let target = parent.join(target.file_name().context("target must name a file")?);
        regular(&target)?;
        let name = target.file_name().unwrap().as_encoded_bytes();
        let id = blake3::hash(name).to_hex().to_string();
        let lock = open_rw(&parent.join(format!(".everywhere-{id}.lock")))?;
        lock.try_lock_exclusive()
            .context("another receiver owns target")?;
        let part_path = parent.join(format!(".everywhere-{id}-{}.part", manifest.hash));
        let journal_path = parent.join(format!(".everywhere-{id}-{}.json", manifest.hash));
        recover_install(&target, &journal_path.with_extension("install"), &manifest)?;
        let original = snapshot(&target)?;
        if regular(&journal_path)? {
            let journal: Journal =
                serde_json::from_reader(File::open(&journal_path)?.take(160 * 1024 * 1024))?;
            ensure!(journal.manifest == manifest, "resume manifest mismatch");
            ensure!(
                journal.original == original || original.as_ref() == Some(&manifest),
                "target changed while offline; resume blocked"
            );
        } else {
            ensure!(
                !regular(&part_path)?,
                "partial data has no recovery journal"
            );
            let mut journal = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&journal_path)?;
            serde_json::to_writer(
                &mut journal,
                &Journal {
                    manifest: manifest.clone(),
                    original: original.clone(),
                },
            )?;
            journal.sync_all()?;
            sync_dir(&parent)?;
        }
        let part = open_rw(&part_path)?;
        ensure!(
            part.metadata()?.len() <= manifest.size,
            "partial file larger than manifest"
        );
        sync_dir(&parent)?;
        Ok(Self {
            target,
            journal_path,
            part_path,
            part,
            _lock: lock,
            manifest,
            original,
            finished: false,
        })
    }
    pub fn missing(&mut self) -> Result<Vec<u64>> {
        let mut missing = Vec::new();
        let mut buffer = vec![0; BLOCK_SIZE];
        let mut existing = if regular(&self.target)? {
            Some(File::open(&self.target)?)
        } else {
            None
        };
        for index in 0..self.manifest.blocks.len() as u64 {
            let len = block_len(&self.manifest, index)?;
            let offset = index * BLOCK_SIZE as u64;
            self.part.seek(SeekFrom::Start(offset))?;
            let valid = self.part.read_exact(&mut buffer[..len]).is_ok()
                && blake3::hash(&buffer[..len]).to_hex().as_str()
                    == self.manifest.blocks[index as usize];
            if valid {
                continue;
            }
            let mut reused = false;
            if let Some(file) = existing.as_mut() {
                file.seek(SeekFrom::Start(offset))?;
                if file.read_exact(&mut buffer[..len]).is_ok()
                    && blake3::hash(&buffer[..len]).to_hex().as_str()
                        == self.manifest.blocks[index as usize]
                {
                    self.put(index, &buffer[..len])?;
                    reused = true;
                }
            }
            if !reused {
                missing.push(index);
            }
        }
        Ok(missing)
    }
    pub fn put(&mut self, index: u64, data: &[u8]) -> Result<()> {
        ensure!(!self.finished, "receiver already complete");
        ensure!(
            data.len() == block_len(&self.manifest, index)?,
            "block length mismatch"
        );
        ensure!(
            blake3::hash(data).to_hex().as_str() == self.manifest.blocks[index as usize],
            "block hash mismatch"
        );
        self.part.seek(SeekFrom::Start(index * BLOCK_SIZE as u64))?;
        self.part.write_all(data)?;
        self.part.sync_data()?;
        Ok(())
    }
    pub fn finish(&mut self) -> Result<()> {
        ensure!(!self.finished, "receiver already complete");
        let intent_path = self.journal_path.with_extension("install");
        recover_install(&self.target, &intent_path, &self.manifest)?;
        self.part.sync_all()?;
        ensure!(
            Manifest::from_path(&self.part_path)? == self.manifest,
            "incomplete or corrupt transfer"
        );
        ensure!(
            snapshot(&self.target)? == self.original,
            "target changed during transfer; local content preserved"
        );
        let parent = self.target.parent().unwrap();
        if let Some(old) = &self.original {
            let versions = parent.join(".everywhere-versions");
            if versions.exists() {
                ensure!(
                    fs::symlink_metadata(&versions)?.file_type().is_dir(),
                    "unsafe versions directory"
                );
            } else {
                fs::create_dir(&versions)?;
            }
            let id = blake3::hash(self.target.file_name().unwrap().as_encoded_bytes()).to_hex();
            let version = versions.join(format!("{id}-{}", old.hash));
            if regular(&version)? && Manifest::from_path(&version)? != *old {
                let mut suffix = 0u64;
                loop {
                    let quarantine = versions.join(format!("{id}-{}.corrupt-{suffix}", old.hash));
                    if !quarantine.try_exists()? {
                        fs::rename(&version, quarantine)?;
                        sync_dir(&versions)?;
                        break;
                    }
                    suffix += 1;
                }
            }
            if !regular(&version)? {
                let mut input = File::open(&self.target)?;
                let mut output = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&version)?;
                std::io::copy(&mut input, &mut output)?;
                output.sync_all()?;
                ensure!(
                    Manifest::from_path(&version)? == *old,
                    "target changed while saving version"
                );
            }
            sync_dir(&versions)?;
            sync_dir(parent)?;
        }
        ensure!(
            snapshot(&self.target)? == self.original,
            "target changed before install"
        );
        if let Some(old) = &self.original {
            let displaced = prepare_install(
                &self.target, &self.part_path, &intent_path, old, &self.manifest,
            )?;
            if snapshot(&displaced)?.as_ref() != Some(old) {
                // Keep a concurrently saved replacement at its original name
                // when possible, and retain its real file in recovery in all cases.
                let _ = fs::hard_link(&displaced, &self.target);
                sync_dir(parent)?;
                anyhow::bail!("target changed at install; local content preserved at {}", displaced.display());
            }
        }
        // Unlike rename, hard_link never replaces an entry that appeared after
        // our check. Unsupported filesystems fail closed with all data retained.
        if let Err(error) = fs::hard_link(&self.part_path, &self.target) {
            if self.original.is_some() {
                let _ = recover_install(&self.target, &intent_path, &self.manifest);
            }
            return Err(error).context("could not publish destination; all versions preserved");
        }
        sync_dir(parent)?;
        if regular(&intent_path)? {
            fs::remove_file(&intent_path)?;
            sync_dir(parent)?;
        }
        fs::remove_file(&self.part_path)?;
        fs::remove_file(&self.journal_path)?;
        sync_dir(parent)?;
        self.finished = true;
        Ok(())
    }
}

pub fn restore(version: &Path, target: &Path) -> Result<()> {
    let manifest = Manifest::from_path(version)?;
    let mut receiver = Receiver::open(target, manifest.clone())?;
    let mut source = File::open(version)?;
    let mut buffer = vec![0; BLOCK_SIZE];
    for index in receiver.missing()? {
        let len = block_len(&manifest, index)?;
        source.seek(SeekFrom::Start(index * BLOCK_SIZE as u64))?;
        source.read_exact(&mut buffer[..len])?;
        receiver.put(index, &buffer[..len])?;
    }
    ensure!(
        Manifest::from_path(version)? == manifest,
        "version changed during restore"
    );
    receiver.finish()
}

#[cfg(test)]
mod install_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn restart_restores_original_after_displacement_before_publication() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::write(&source, b"incoming").unwrap();
        fs::write(&target, b"original").unwrap();
        let manifest = Manifest::from_path(&source).unwrap();
        let mut receiver = Receiver::open(&target, manifest.clone()).unwrap();
        receiver.put(0, b"incoming").unwrap();
        let displaced = prepare_install(
            &target, &receiver.part_path, &receiver.journal_path.with_extension("install"),
            receiver.original.as_ref().unwrap(), &manifest,
        ).unwrap();
        assert!(!target.exists());
        drop(receiver);
        let mut resumed = Receiver::open(&target, manifest).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert!(resumed.missing().unwrap().is_empty());
        resumed.finish().unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"incoming");
        assert_eq!(fs::read(displaced).unwrap(), b"original");
    }

    #[test]
    fn recovery_never_overwrites_a_file_created_during_install() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::write(&source, b"incoming").unwrap();
        fs::write(&target, b"original").unwrap();
        let manifest = Manifest::from_path(&source).unwrap();
        let mut receiver = Receiver::open(&target, manifest.clone()).unwrap();
        receiver.put(0, b"incoming").unwrap();
        let displaced = prepare_install(
            &target, &receiver.part_path, &receiver.journal_path.with_extension("install"),
            receiver.original.as_ref().unwrap(), &manifest,
        ).unwrap();
        fs::write(&target, b"concurrent save").unwrap();
        drop(receiver);
        assert!(Receiver::open(&target, manifest).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"concurrent save");
        assert_eq!(fs::read(displaced).unwrap(), b"original");
    }
}
