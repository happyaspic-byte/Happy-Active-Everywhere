//! Filesystem operations confined to an explicitly opened share directory.
use crate::{
    model::{Content, validate_path},
    storage::Manifest,
};
use anyhow::{Context, Result, ensure};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) struct Root {
    pub dir: Dir,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    path: String,
    before: Option<Content>,
    after: Content,
}

pub(crate) fn random_id() -> Result<String> {
    Ok(blake3::hash(&rcgen::KeyPair::generate()?.serialize_der())
        .to_hex()
        .to_string())
}
fn sync(dir: &Dir) -> Result<()> {
    #[cfg(unix)]
    dir.open(".")?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}
fn read_content(dir: &Dir, path: &Path) -> Result<Option<Content>> {
    let metadata = match dir.symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    ensure!(
        !metadata.file_type().is_symlink(),
        "links are not supported"
    );
    if metadata.is_dir() {
        return Ok(Some(Content::Directory));
    }
    ensure!(metadata.is_file(), "special files are not supported");
    Ok(Some(Content::File(
        Manifest::from_file(dir.open(path)?.into_std())?.hash,
    )))
}
fn new_file(dir: &Dir, path: &Path) -> Result<cap_std::fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(dir.open_with(path, &options)?)
}
fn finish_record(transaction: &Dir) -> Result<()> {
    let done = new_file(transaction, Path::new("done"))?;
    done.sync_all()?;
    // The displaced file remains linked. The staging link must be removed so
    // a subsequent user edit does not mutate a second apparent incoming copy.
    if transaction.symlink_metadata("incoming").is_ok() {
        transaction.remove_file("incoming")?;
    }
    sync(transaction)
}
impl Root {
    fn sync_parent(&self, path: &Path) -> Result<()> {
        match path.parent().filter(|p| !p.as_os_str().is_empty()) {
            Some(parent) => sync(&self.dir.open_dir(parent)?),
            None => sync(&self.dir),
        }
    }
    pub fn open(path: &Path) -> Result<Self> {
        ensure!(
            std::fs::symlink_metadata(path)?.file_type().is_dir(),
            "share root must be a directory"
        );
        Ok(Self {
            dir: Dir::open_ambient_dir(path, ambient_authority())?,
        })
    }
    pub fn same_directory(&self, other: &Self) -> Result<bool> {
        // Compare the opened handles, not paths or copied marker contents.
        // This identity is session-local: legitimate remounts may change the
        // device identifier and must be reopened by a fresh session.
        Ok(
            same_file::Handle::from_file(self.dir.try_clone()?.into_std_file())?
                == same_file::Handle::from_file(other.dir.try_clone()?.into_std_file())?,
        )
    }
    pub fn content(&self, path: &str) -> Result<Option<Content>> {
        validate_path(path)?;
        read_content(&self.dir, Path::new(path))
    }
    pub fn file(&self, path: &str) -> Result<File> {
        validate_path(path)?;
        ensure!(
            self.dir.symlink_metadata(path)?.file_type().is_file(),
            "not a regular file"
        );
        Ok(self.dir.open(path)?.into_std())
    }
    pub fn visit(&self, visit: &mut impl FnMut(&str, bool) -> Result<()>) -> Result<()> {
        self.walk(Path::new(""), visit)
    }
    fn walk(
        &self,
        relative: &Path,
        visit: &mut impl FnMut(&str, bool) -> Result<()>,
    ) -> Result<()> {
        let directory = if relative.as_os_str().is_empty() {
            self.dir.try_clone()?
        } else {
            self.dir.open_dir(relative)?
        };
        for item in directory.entries()? {
            let item = item?;
            let name = item
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("non-UTF-8 filename unsupported"))?;
            if name.to_ascii_lowercase().starts_with(".everywhere-") {
                continue;
            }
            let path = relative.join(&name);
            let portable = path.to_str().context("non-UTF-8 path")?.replace('\\', "/");
            validate_path(&portable)?;
            // Validate the original component before Windows separator conversion.
            ensure!(!name.contains('\\'), "backslash filename is not portable");
            let kind = item.file_type()?;
            ensure!(
                !kind.is_symlink() && (kind.is_file() || kind.is_dir()),
                "links and special files are not supported"
            );
            visit(&portable, kind.is_dir())?;
            if kind.is_dir() {
                self.walk(&path, visit)?;
            }
        }
        Ok(())
    }
    fn ensure_parents(&self, path: &Path) -> Result<()> {
        let mut current = PathBuf::new();
        if let Some(parent) = path.parent() {
            for component in parent.components() {
                current.push(component);
                match self.dir.create_dir(&current) {
                    Ok(()) => self.sync_parent(&current)?,
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e.into()),
                }
                ensure!(
                    self.dir.symlink_metadata(&current)?.file_type().is_dir(),
                    "unsafe parent directory"
                );
            }
        }
        Ok(())
    }
    pub fn apply(
        &self,
        path: &str,
        before: Option<&Content>,
        after: &Content,
        object: Option<&Path>,
    ) -> Result<()> {
        validate_path(path)?;
        let current = self.content(path)?;
        if current.as_ref() == Some(after) || (current.is_none() && *after == Content::Deleted) {
            return Ok(());
        }
        ensure!(
            current.as_ref() == before,
            "local file changed before application: {path}"
        );
        self.ensure_parents(Path::new(path))?;
        if *after == Content::Directory {
            ensure!(
                current.is_none(),
                "file/directory conflict needs explicit resolution: {path}"
            );
            self.dir.create_dir(path)?;
            self.sync_parent(Path::new(path))?;
            return Ok(());
        }
        if current == Some(Content::Directory) {
            ensure!(
                *after == Content::Deleted,
                "file/directory conflict needs explicit resolution: {path}"
            );
            // Never remove or move a nonempty directory on a tombstone.
            self.dir.remove_dir(path)?;
            self.sync_parent(Path::new(path))?;
            return Ok(());
        }
        let mut builder = cap_std::fs::DirBuilder::new();
        builder.recursive(false);
        #[cfg(unix)]
        {
            use cap_std::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match self.dir.create_dir_with(".everywhere-recovery", &builder) {
            Ok(()) => sync(&self.dir)?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        ensure!(
            self.dir
                .symlink_metadata(".everywhere-recovery")?
                .file_type()
                .is_dir(),
            "unsafe recovery directory"
        );
        let recovery = self.dir.open_dir(".everywhere-recovery")?;
        #[cfg(unix)]
        {
            use cap_std::fs::PermissionsExt;
            recovery.set_permissions(".", cap_std::fs::Permissions::from_mode(0o700))?;
            sync(&recovery)?;
        }
        let id = random_id()?;
        recovery.create_dir(&id)?;
        let transaction = recovery.open_dir(&id)?;
        if let Content::File(hash) = after {
            let mut input = File::open(object.context("missing content object")?)?;
            let mut output = new_file(&transaction, Path::new("incoming"))?;
            std::io::copy(&mut input, &mut output)?;
            output.sync_all()?;
            ensure!(
                read_content(&transaction, Path::new("incoming"))?
                    == Some(Content::File(hash.clone())),
                "content object is corrupt"
            );
            // Preflight no-clobber publication support before displacing data.
            transaction.hard_link("incoming", &transaction, "link-probe")?;
            transaction.remove_file("link-probe")?;
        }
        let record = Intent {
            path: path.into(),
            before: current.clone(),
            after: after.clone(),
        };
        let mut file = new_file(&transaction, Path::new("record.json"))?;
        file.write_all(&serde_json::to_vec(&record)?)?;
        file.sync_all()?;
        sync(&transaction)?;
        sync(&recovery)?;
        sync(&self.dir)?;
        if current.is_some() {
            self.dir.rename(path, &transaction, "previous")?;
            sync(&transaction)?;
            self.sync_parent(Path::new(path))?;
            if read_content(&transaction, Path::new("previous"))? != current {
                let _ = transaction.hard_link("previous", &self.dir, path);
                sync(&self.dir)?;
                anyhow::bail!("local edit preserved in recovery for {path}");
            }
        }
        if matches!(after, Content::File(_)) {
            if let Err(error) = transaction.hard_link("incoming", &self.dir, path) {
                if current.is_some() {
                    let _ = transaction.hard_link("previous", &self.dir, path);
                }
                sync(&self.dir)?;
                return Err(error)
                    .context("destination appeared during publication; data preserved");
            }
        } else {
            ensure!(
                self.content(path)?.is_none(),
                "local file appeared during deletion; content preserved"
            );
        }
        self.sync_parent(Path::new(path))?;
        finish_record(&transaction)
    }
    pub fn recover(&self) -> Result<()> {
        let recovery = match self.dir.open_dir(".everywhere-recovery") {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        for item in recovery.entries()? {
            let item = item?;
            if !item.file_type()?.is_dir() {
                continue;
            }
            let transaction = recovery.open_dir(item.file_name())?;
            if transaction.symlink_metadata("done").is_ok() {
                continue;
            }
            let bytes = match transaction.read("record.json") {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            ensure!(bytes.len() <= 64 * 1024, "oversized recovery record");
            let value: serde_json::Value = serde_json::from_slice(&bytes)?;
            if value.get("intent").is_some() && value.get("target").is_some() {
                // Single-file Receiver owns this journal and its recovery.
                // It can coexist with a folder share without being misparsed.
                continue;
            }
            let intent: Intent = serde_json::from_value(value)?;
            validate_path(&intent.path)?;
            let actual = self.content(&intent.path)?;
            if actual.as_ref() == Some(&intent.after)
                || (actual.is_none() && intent.after == Content::Deleted)
                || actual == intent.before
            {
                finish_record(&transaction)?;
            } else if actual.is_none() && transaction.symlink_metadata("previous").is_ok() {
                transaction.hard_link("previous", &self.dir, &intent.path)?;
                sync(&self.dir)?;
                finish_record(&transaction)?;
            } else {
                anyhow::bail!(
                    "ambiguous interrupted application for {}; local content and recovery retained",
                    intent.path
                );
            }
        }
        Ok(())
    }
}
