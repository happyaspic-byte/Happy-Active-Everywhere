use crate::{
    identity,
    model::{Content, Revision, Versions, collision_key, validate_path},
    root::{Root, random_id},
    storage::Manifest,
    versions::Mode,
};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub mod backup;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub id: String,
    pub root: PathBuf,
    pub identity: String,
    pub epoch: String,
    pub marker: String,
    pub mode: Mode,
    pub peers: BTreeSet<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub path: String,
    pub versions: Versions,
    pub seq: u64,
}
#[derive(Clone)]
struct Local {
    record: Record,
    materialized: Option<Content>,
    observed: Versions,
}
#[derive(Debug, Default, Serialize)]
pub struct ScanReport {
    pub changed: usize,
    pub pending_deletions: usize,
}

pub struct Share {
    pub config: Config,
    directory: PathBuf,
    connection: Connection,
    root: Root,
    _lock: File,
}
pub(crate) fn valid_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "invalid share identifier"
    );
    Ok(())
}
fn directory(state: &Path, id: &str) -> Result<PathBuf> {
    valid_id(id)?;
    Ok(state.join("shares").join(id))
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
fn read_config(directory: &Path) -> Result<Config> {
    let path = directory.join("config.json");
    ensure!(
        fs::symlink_metadata(&path)?.file_type().is_file(),
        "unsafe share configuration"
    );
    let mut bytes = Vec::new();
    File::open(path)?
        .take(128 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 128 * 1024, "share configuration too large");
    let config: Config = serde_json::from_slice(&bytes)?;
    valid_id(&config.id)?;
    for value in [&config.identity, &config.epoch, &config.marker] {
        identity::valid_peer(value)?;
    }
    ensure!(config.root.is_absolute(), "share root must be absolute");
    ensure!(config.peers.len() <= 128, "too many approved peers");
    for peer in &config.peers {
        identity::valid_peer(peer)?;
    }
    Ok(config)
}
pub fn grant(state: &Path, id: &str, peer: &str, remove: bool) -> Result<()> {
    let _device = crate::device::config_guard(state)?;
    identity::valid_peer(peer)?;
    if !remove {
        identity::Identity::new(state, peer)?;
    }
    let directory = directory(state, id)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("config.lock"))?;
    lock.try_lock_exclusive()
        .context("share configuration is busy")?;
    ensure!(
        !directory.join("state-recovery.json").try_exists()?,
        "share state recovery must finish before changing folder grants"
    );
    let mut config = read_config(&directory)?;
    if remove {
        config.peers.remove(peer);
    } else {
        config.peers.insert(peer.into());
    }
    let temporary = directory.join(format!("config-{}.tmp", random_id()?));
    write_new(&temporary, &serde_json::to_vec_pretty(&config)?)?;
    fs::rename(temporary, directory.join("config.json"))?;
    #[cfg(unix)]
    File::open(&directory)?.sync_all()?;
    Ok(())
}
pub(crate) fn check_access(state: &Path, id: &str, peer: &str, writing: bool) -> Result<()> {
    let config = read_config(&directory(state, id)?)?;
    ensure!(
        config.peers.contains(peer),
        "peer is not approved for this share"
    );
    ensure!(
        !writing || config.mode != Mode::SendOnly,
        "share is send-only"
    );
    Ok(())
}
impl Share {
    pub fn create(state: &Path, id: &str, root: &Path, mode: Mode) -> Result<Self> {
        let _device = crate::device::config_guard(state)?;
        valid_id(id)?;
        let state = state.canonicalize()?;
        let root = root.canonicalize()?;
        ensure!(
            !root.starts_with(&state) && !state.starts_with(&root),
            "state and share roots must not overlap"
        );
        let shares = state.join("shares");
        fs::create_dir_all(&shares)?;
        for item in fs::read_dir(&shares)? {
            let path = item?.path();
            if !path.is_dir() {
                continue;
            }
            let config = read_config(&path)?;
            ensure!(
                !root.starts_with(&config.root) && !config.root.starts_with(&root),
                "share roots must not overlap"
            );
        }
        let directory = directory(&state, id)?;
        ensure!(!directory.try_exists()?, "share already exists");
        let handle = Root::open(&root)?;
        match handle.dir.symlink_metadata(".everywhere-folder") {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(_) => anyhow::bail!("share root already contains a folder marker"),
        }
        // A refused filesystem must not leave a marker or an incomplete share
        // registry entry that would also block unrelated future registrations.
        handle.preflight()?;
        fs::create_dir(&directory).context("share already exists")?;
        let marker = random_id()?;
        let mut marker_file = handle.dir.open_with(
            ".everywhere-folder",
            cap_std::fs::OpenOptions::new().create_new(true).write(true),
        )?;
        marker_file.write_all(marker.as_bytes())?;
        marker_file.sync_all()?;
        #[cfg(unix)]
        handle.dir.open(".")?.sync_all()?;
        let config = Config {
            id: id.into(),
            root,
            identity: identity::fingerprint(&fs::read(state.join("identity.der"))?),
            epoch: random_id()?,
            marker,
            mode,
            peers: BTreeSet::new(),
        };
        fs::create_dir(directory.join("objects"))?;
        write_new(
            &directory.join("config.json"),
            &serde_json::to_vec_pretty(&config)?,
        )?;
        let db = Connection::open(directory.join("index.sqlite"))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE entries(path TEXT PRIMARY KEY,alias TEXT UNIQUE NOT NULL,versions TEXT NOT NULL,materialized TEXT NOT NULL,observed TEXT NOT NULL,seq INTEGER NOT NULL);
            CREATE TABLE pending(path TEXT PRIMARY KEY);
            CREATE TABLE incoming(path TEXT PRIMARY KEY,versions TEXT NOT NULL);
            CREATE TABLE needed(hash TEXT PRIMARY KEY);
            CREATE TABLE cursors(peer TEXT PRIMARY KEY,epoch TEXT NOT NULL,seq INTEGER NOT NULL);
            INSERT INTO meta VALUES('schema','1'),('seq','0');")?;
        db.execute("INSERT INTO meta VALUES('epoch',?1)", [&config.epoch])?;
        drop(db);
        Self::open(&state, id)
    }
    pub fn open(state: &Path, id: &str) -> Result<Self> {
        let directory = directory(state, id)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("index.lock"))?;
        lock.try_lock_exclusive()
            .context("share is busy; retry after the active operation")?;
        backup::resume(state, id, &directory)?;
        let config = read_config(&directory)?;
        ensure!(config.id == id, "share identifier mismatch");
        ensure!(
            config.identity == identity::fingerprint(&fs::read(state.join("identity.der"))?),
            "share device identity changed"
        );
        let db_path = directory.join("index.sqlite");
        ensure!(
            fs::symlink_metadata(&db_path)?.file_type().is_file(),
            "unsafe share index"
        );
        let connection =
            Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        connection.execute_batch("PRAGMA synchronous=FULL; PRAGMA temp_store=FILE;
            CREATE TABLE IF NOT EXISTS history(path TEXT NOT NULL,id TEXT NOT NULL,revision TEXT NOT NULL,PRIMARY KEY(path,id));")?;
        let integrity: String = connection.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        ensure!(
            integrity == "ok",
            "share index is corrupt; synchronization paused"
        );
        let epoch: String =
            connection.query_row("SELECT value FROM meta WHERE key='epoch'", [], |r| r.get(0))?;
        ensure!(epoch == config.epoch, "share index epoch mismatch");
        let root = Root::open(&config.root)?;
        let result = Self {
            config,
            directory,
            connection,
            root,
            _lock: lock,
        };
        result.check_root()?;
        result.root.recover()?;
        Ok(result)
    }
    pub fn authorize(&self, peer: &str, writing: bool) -> Result<()> {
        let state = self
            .directory
            .parent()
            .and_then(Path::parent)
            .context("invalid share state location")?;
        identity::Identity::new(state, peer)?;
        let current = read_config(&self.directory)?;
        ensure!(
            current.peers.contains(peer),
            "peer is not approved for this share"
        );
        ensure!(
            !writing || current.mode != Mode::SendOnly,
            "share is send-only"
        );
        Ok(())
    }
    pub fn check_root(&self) -> Result<()> {
        let current = Root::open(&self.config.root)?;
        ensure!(
            self.root.same_directory(&current)?,
            "share root directory changed during this session; reconnect to reopen it"
        );
        ensure!(
            current
                .dir
                .symlink_metadata(".everywhere-folder")?
                .file_type()
                .is_file(),
            "share marker unavailable"
        );
        let marker = current.dir.read_to_string(".everywhere-folder")?;
        ensure!(
            marker == self.config.marker,
            "share mount identity changed; synchronization paused"
        );
        Ok(())
    }
    fn replica(&self) -> String {
        format!("{}:{}", self.config.identity, &self.config.epoch[..16])
    }
    fn local(&self, path: &str) -> Result<Option<Local>> {
        let row = self
            .connection
            .query_row(
                "SELECT versions,materialized,observed,seq FROM entries WHERE path=?1",
                [path],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(versions, materialized, observed, seq)| {
            Ok(Local {
                record: Record {
                    path: path.into(),
                    versions: serde_json::from_str(&versions)?,
                    seq: u64::try_from(seq)?,
                },
                materialized: serde_json::from_str(&materialized)?,
                observed: serde_json::from_str(&observed)?,
            })
        })
        .transpose()
    }
    pub fn sequence(&self) -> Result<u64> {
        let value: String =
            self.connection
                .query_row("SELECT value FROM meta WHERE key='seq'", [], |r| r.get(0))?;
        Ok(value.parse()?)
    }
    fn save(
        &self,
        path: &str,
        versions: &Versions,
        materialized: Option<&Content>,
        observed: &Versions,
    ) -> Result<()> {
        validate_path(path)?;
        let prior = self.local(path)?;
        // Preserve superseded revisions before replacing the causal register.
        for revision in prior
            .iter()
            .flat_map(|l| &l.record.versions.heads)
            .chain(&versions.heads)
        {
            self.connection.execute(
                "INSERT OR IGNORE INTO history VALUES(?1,?2,?3)",
                params![path, revision.id()?, serde_json::to_string(revision)?],
            )?;
        }
        let changed = prior
            .as_ref()
            .is_none_or(|l| l.record.versions != *versions);
        let sequence = if changed {
            let value = self
                .sequence()?
                .checked_add(1)
                .context("share sequence exhausted")?;
            ensure!(value <= i64::MAX as u64, "share sequence exhausted");
            self.connection.execute(
                "UPDATE meta SET value=?1 WHERE key='seq'",
                [value.to_string()],
            )?;
            value
        } else {
            prior.as_ref().unwrap().record.seq
        };
        self.connection.execute("INSERT INTO entries VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(path) DO UPDATE SET versions=excluded.versions,materialized=excluded.materialized,observed=excluded.observed,seq=excluded.seq", params![path,collision_key(path),serde_json::to_string(versions)?,serde_json::to_string(&materialized)?,serde_json::to_string(observed)?,i64::try_from(sequence)?])?;
        Ok(())
    }
    pub fn records(&self, since: u64, ceiling: u64, limit: usize) -> Result<Vec<Record>> {
        ensure!(limit <= 256, "metadata page too large");
        let mut statement = self.connection.prepare(
            "SELECT path,versions,seq FROM entries WHERE seq>?1 AND seq<=?2 ORDER BY seq LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![i64::try_from(since)?, i64::try_from(ceiling)?, limit as i64],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            },
        )?;
        rows.map(|r| {
            let (path, versions, seq) = r?;
            Ok(Record {
                path,
                versions: serde_json::from_str(&versions)?,
                seq: u64::try_from(seq)?,
            })
        })
        .collect()
    }
    fn all_paths(&self) -> Result<Vec<String>> {
        let mut statement = self
            .connection
            .prepare("SELECT path FROM entries ORDER BY path")?;
        Ok(statement
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
    pub fn object_path(&self, hash: &str) -> Result<PathBuf> {
        ensure!(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "invalid object identifier"
        );
        Ok(self.directory.join("objects").join(hash))
    }
    fn capture(&self, path: &str) -> Result<Content> {
        let manifest = Manifest::from_file(self.root.file(path)?)?;
        let object = self.object_path(&manifest.hash)?;
        if object.try_exists()? {
            ensure!(
                Manifest::from_path(&object)?.hash == manifest.hash,
                "content cache corrupted"
            );
        } else {
            let temporary = self
                .directory
                .join("objects")
                .join(format!("{}.part", random_id()?));
            let mut input = self.root.file(path)?;
            let mut output = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            std::io::copy(&mut input, &mut output)?;
            output.sync_all()?;
            ensure!(
                Manifest::from_path(&temporary)? == manifest,
                "source changed while capturing content"
            );
            fs::hard_link(&temporary, &object)?;
            fs::remove_file(temporary)?;
            #[cfg(unix)]
            File::open(self.directory.join("objects"))?.sync_all()?;
        }
        Ok(Content::File(manifest.hash))
    }
    pub fn scan(&self, approve_deletes: bool) -> Result<ScanReport> {
        ensure!(
            !self.recovery_pending()?,
            "restored state awaits full reconciliation with an approved peer; connect before scanning or approving deletions"
        );
        self.scan_inner(approve_deletes)
    }
    pub fn recovery_pending(&self) -> Result<bool> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM meta WHERE key='recovery_pending'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        match value.as_deref() {
            None | Some("0") => Ok(false),
            Some("1") => Ok(true),
            _ => anyhow::bail!("invalid recovery state"),
        }
    }
    fn scan_inner(&self, approve_deletes: bool) -> Result<ScanReport> {
        self.check_root()?;
        self.root.recover()?;
        self.connection.execute_batch("CREATE TEMP TABLE IF NOT EXISTS scan_seen(path TEXT PRIMARY KEY,alias TEXT UNIQUE NOT NULL,content TEXT NOT NULL); DELETE FROM scan_seen;")?;
        let transaction = self.connection.unchecked_transaction()?;
        self.root.visit(&mut |path, is_dir| {
            let content = if is_dir {
                Content::Directory
            } else {
                self.capture(path)?
            };
            self.connection
                .execute(
                    "INSERT INTO scan_seen VALUES(?1,?2,?3)",
                    params![path, collision_key(path), serde_json::to_string(&content)?],
                )
                .context("cannot record scanned path; check case/Unicode aliases and free space")?;
            Ok(())
        })?;
        self.check_root()?;
        let mut report = ScanReport::default();
        let mut after = String::new();
        loop {
            let page: Vec<(String, String)> = {
                let mut statement = self.connection.prepare(
                    "SELECT path,content FROM scan_seen WHERE path>?1 ORDER BY path LIMIT 256",
                )?;
                statement
                    .query_map([&after], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<std::result::Result<_, _>>()?
            };
            if page.is_empty() {
                break;
            }
            for (path, json) in &page {
                after = path.clone();
                let parsed: Content = serde_json::from_str(json)?;
                let content = &parsed;
                let old = self.local(path)?;
                self.connection
                    .execute("DELETE FROM pending WHERE path=?1", [path])?;
                if let Some(previous) = &old {
                    if previous.record.versions != previous.observed
                        && previous
                            .record
                            .versions
                            .heads
                            .iter()
                            .any(|r| &r.content == content)
                    {
                        // The filesystem publication completed before the index
                        // acknowledgement. Adopt it without inventing a local edit.
                        let selected = previous
                            .record
                            .versions
                            .selected()
                            .is_some_and(|r| &r.content == content);
                        self.save(
                            path,
                            &previous.record.versions,
                            Some(content),
                            if selected {
                                &previous.record.versions
                            } else {
                                &previous.observed
                            },
                        )?;
                        continue;
                    }
                }
                if old.as_ref().and_then(|l| l.materialized.as_ref()) == Some(content) {
                    continue;
                }
                let observed = old.as_ref().map(|l| l.observed.clone()).unwrap_or_default();
                let local = observed.edit(&self.replica(), content.clone())?;
                let combined = old
                    .as_ref()
                    .map(|l| l.record.versions.join(&local))
                    .transpose()?
                    .unwrap_or_else(|| local.clone());
                self.save(path, &combined, Some(content), &local)?;
                report.changed += 1;
            }
        }
        let mut after = String::new();
        loop {
            let page: Vec<String> = {
                let mut statement = self.connection.prepare("SELECT e.path FROM entries e LEFT JOIN scan_seen s ON s.path=e.path WHERE s.path IS NULL AND e.path>?1 ORDER BY e.path LIMIT 256")?;
                statement
                    .query_map([&after], |r| r.get(0))?
                    .collect::<std::result::Result<_, _>>()?
            };
            if page.is_empty() {
                break;
            }
            for path in page {
                after = path.clone();
                let old = self.local(&path)?.unwrap();
                if old.record.versions != old.observed
                    && old
                        .record
                        .versions
                        .selected()
                        .is_some_and(|r| r.content == Content::Deleted)
                {
                    self.save(&path, &old.record.versions, None, &old.record.versions)?;
                    self.connection
                        .execute("DELETE FROM pending WHERE path=?1", [&path])?;
                    continue;
                }
                if old.materialized.is_none() {
                    continue;
                }
                if approve_deletes {
                    let deleted = old.observed.edit(&self.replica(), Content::Deleted)?;
                    self.save(&path, &old.record.versions.join(&deleted)?, None, &deleted)?;
                    self.connection
                        .execute("DELETE FROM pending WHERE path=?1", [&path])?;
                    report.changed += 1;
                } else {
                    self.connection
                        .execute("INSERT OR IGNORE INTO pending VALUES(?1)", [&path])?;
                }
            }
        }
        report.pending_deletions = usize::try_from(self.connection.query_row(
            "SELECT count(*) FROM pending",
            [],
            |r| r.get::<_, i64>(0),
        )?)?;
        self.check_root()?;
        transaction.commit()?;
        Ok(report)
    }
    pub fn begin_incoming(&self) -> Result<()> {
        self.connection
            .execute_batch("DELETE FROM incoming; DELETE FROM needed;")?;
        Ok(())
    }
    pub fn stage(&self, peer: &str, records: &[Record]) -> Result<()> {
        self.authorize(peer, true)?;
        ensure!(records.len() <= 256, "metadata page too large");
        let transaction = self.connection.unchecked_transaction()?;
        for record in records {
            validate_path(&record.path)?;
            let validated = Versions::default().join(&record.versions)?;
            ensure!(!validated.heads.is_empty(), "empty remote version set");
            self.connection.execute("INSERT INTO incoming VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET versions=excluded.versions", params![record.path,serde_json::to_string(&validated)?])?;
            for head in &validated.heads {
                if let Content::File(hash) = &head.content {
                    let path = self.object_path(hash)?;
                    if !path.try_exists()? {
                        self.connection
                            .execute("INSERT OR IGNORE INTO needed VALUES(?1)", [hash])?;
                    }
                }
            }
        }
        transaction.commit()?;
        Ok(())
    }
    pub fn basis_object(&self, hash: &str) -> Result<Option<PathBuf>> {
        self.object_path(hash)?;
        let old: Option<String> = self.connection.query_row(
            "SELECT json_extract(e.materialized,'$.hash') FROM incoming i JOIN entries e ON e.path=i.path JOIN json_each(i.versions,'$.heads') h WHERE json_extract(h.value,'$.content.hash')=?1 AND json_extract(e.materialized,'$.kind')='file' LIMIT 1",
            [hash], |r| r.get(0),
        ).optional()?;
        old.map(|hash| self.object_path(&hash)).transpose()
    }
    pub fn missing_objects(&self, limit: usize) -> Result<Vec<String>> {
        ensure!(limit <= 64, "object request page too large");
        let mut statement = self
            .connection
            .prepare("SELECT hash FROM needed ORDER BY hash LIMIT ?1")?;
        Ok(statement
            .query_map([limit as i64], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
    pub fn object_received(&self, hash: &str) -> Result<()> {
        ensure!(
            Manifest::from_path(&self.object_path(hash)?)?.hash == hash,
            "received object hash mismatch"
        );
        self.connection
            .execute("DELETE FROM needed WHERE hash=?1", [hash])?;
        Ok(())
    }
    pub fn commit_incoming(&self, peer: &str) -> Result<()> {
        self.authorize(peer, true)?;
        self.check_root()?;
        ensure!(
            self.missing_objects(1)?.is_empty(),
            "content objects are still missing"
        );
        // Capture changes made by applications during network transfer before
        // joining remote heads; they remain concurrent with the remote edits.
        let recovering = self.recovery_pending()?;
        if !recovering {
            self.scan(false)?;
        }
        let transaction = self.connection.unchecked_transaction()?;
        let mut after = String::new();
        loop {
            let rows: Vec<(String, String)> = {
                let mut statement = self.connection.prepare(
                    "SELECT path,versions FROM incoming WHERE path>?1 ORDER BY path LIMIT 256",
                )?;
                statement
                    .query_map([&after], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<std::result::Result<_, _>>()?
            };
            if rows.is_empty() {
                break;
            }
            for (path, json) in rows {
                let remote: Versions = serde_json::from_str(&json)?;
                let old = self.local(&path)?;
                let versions = old
                    .as_ref()
                    .map(|l| l.record.versions.join(&remote))
                    .transpose()?
                    .unwrap_or(remote);
                let materialized = old.as_ref().and_then(|l| l.materialized.as_ref());
                let observed = old.as_ref().map(|l| l.observed.clone()).unwrap_or_default();
                self.save(&path, &versions, materialized, &observed)?;
                after = path;
            }
        }
        self.connection.execute("DELETE FROM incoming", [])?;
        transaction.commit()?;
        if recovering {
            // Reconcile against authenticated remote heads before deciding
            // whether current bytes are a new edit or an already known revision.
            self.scan_inner(false)?;
        }
        self.apply_pending(peer)?;
        if recovering {
            self.connection
                .execute("DELETE FROM meta WHERE key='recovery_pending'", [])?;
        }
        Ok(())
    }
    fn apply_pending(&self, peer: &str) -> Result<()> {
        loop {
            let paths: Vec<String> = {
                let mut statement = self.connection.prepare("WITH candidates AS (SELECT e.path, NOT EXISTS(SELECT 1 FROM json_each(e.versions,'$.heads') h WHERE json_extract(h.value,'$.content.kind')!='deleted') AS removing, length(e.path)-length(replace(e.path,'/','')) AS depth FROM entries e WHERE e.versions!=e.observed AND NOT EXISTS(SELECT 1 FROM pending p WHERE p.path=e.path)) SELECT path FROM candidates ORDER BY removing, CASE WHEN removing THEN -depth ELSE depth END, path LIMIT 256")?;
                statement
                    .query_map([], |r| r.get(0))?
                    .collect::<std::result::Result<_, _>>()?
            };
            if paths.is_empty() {
                break;
            }
            for path in paths {
                let local = self.local(&path)?.unwrap();
                self.authorize(peer, true)?;
                self.apply_local(&path, &local)?;
            }
        }
        Ok(())
    }
    fn apply_local(&self, path: &str, local: &Local) -> Result<()> {
        let selected = &local
            .record
            .versions
            .selected()
            .context("empty version set")?
            .content;
        let object = match selected {
            Content::File(hash) => Some(self.object_path(hash)?),
            _ => None,
        };
        self.check_root()?;
        self.root.apply(
            path,
            local.materialized.as_ref(),
            selected,
            object.as_deref(),
        )?;
        let transaction = self.connection.unchecked_transaction()?;
        let actual = if *selected == Content::Deleted {
            None
        } else {
            Some(selected)
        };
        self.save(path, &local.record.versions, actual, &local.record.versions)?;
        transaction.commit()?;
        Ok(())
    }
    /// Resolve against all observed heads, or restore a locally retained revision.
    pub fn choose(&self, path: &str, revision: &str, historical: bool) -> Result<()> {
        validate_path(path)?;
        self.scan(false)?;
        let local = self.local(path)?.context("unknown path")?;
        let chosen = if historical {
            let json: String = self
                .connection
                .query_row(
                    "SELECT revision FROM history WHERE path=?1 AND id=?2",
                    params![path, revision],
                    |r| r.get(0),
                )
                .optional()?
                .context("unknown historical revision")?;
            serde_json::from_str::<Revision>(&json)?
        } else {
            local
                .record
                .versions
                .heads
                .iter()
                .find(|r| r.id().is_ok_and(|id| id == revision))
                .cloned()
                .context("revision is no longer a current conflict head")?
        };
        if let Content::File(hash) = &chosen.content {
            ensure!(
                Manifest::from_path(&self.object_path(hash)?)?.hash == *hash,
                "historical content is corrupt"
            );
        }
        let versions = local
            .record
            .versions
            .edit(&self.replica(), chosen.content)?;
        let actual = self.root.content(path)?;
        let transaction = self.connection.unchecked_transaction()?;
        self.save(path, &versions, actual.as_ref(), &local.observed)?;
        self.connection
            .execute("DELETE FROM pending WHERE path=?1", [path])?;
        transaction.commit()?;
        self.apply_local(path, &self.local(path)?.unwrap())
    }
    pub fn pending(&self) -> Result<serde_json::Value> {
        let mut statement = self.connection.prepare("SELECT e.path,e.versions FROM pending p JOIN entries e ON e.path=p.path ORDER BY e.path LIMIT 100")?;
        let rows =
            statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let entries = rows
            .map(|r| {
                let (path, json) = r?;
                let versions: Versions = serde_json::from_str(&json)?;
                Ok(serde_json::json!({"path":path,"versions":versions}))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(serde_json::Value::Array(entries))
    }
    pub fn approve_deletion(&self, path: &str, expected: &Versions) -> Result<()> {
        validate_path(path)?;
        self.scan(false)?;
        let local = self.local(path)?.context("unknown deletion path")?;
        let pending: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pending WHERE path=?1)",
            [path],
            |r| r.get(0),
        )?;
        ensure!(
            pending && local.record.versions == *expected,
            "deletion review is stale; review the current state again"
        );
        self.check_root()?;
        ensure!(
            self.root.content(path)?.is_none(),
            "local path reappeared after deletion review"
        );
        let deletion = local.observed.edit(&self.replica(), Content::Deleted)?;
        let transaction = self.connection.unchecked_transaction()?;
        self.save(
            path,
            &local.record.versions.join(&deletion)?,
            None,
            &deletion,
        )?;
        self.connection
            .execute("DELETE FROM pending WHERE path=?1", [path])?;
        transaction.commit()?;
        Ok(())
    }
    pub fn history(&self, path: &str) -> Result<serde_json::Value> {
        validate_path(path)?;
        let mut statement = self
            .connection
            .prepare("SELECT id,revision FROM history WHERE path=?1 ORDER BY id")?;
        let rows = statement.query_map([path], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let revisions = rows.map(|row| {
            let (id, json) = row?;
            let revision: Revision = serde_json::from_str(&json)?;
            let object = match &revision.content { Content::File(hash) => Some(self.object_path(hash)?), _ => None };
            Ok(serde_json::json!({"id":id,"content":revision.content,"clock":revision.clock,"object_path":object}))
        }).collect::<Result<Vec<_>>>()?;
        Ok(serde_json::Value::Array(revisions))
    }
    pub fn cursor(&self, peer: &str) -> Result<(String, u64)> {
        let (epoch, sequence): (String, i64) = self
            .connection
            .query_row("SELECT epoch,seq FROM cursors WHERE peer=?1", [peer], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?
            .unwrap_or_default();
        Ok((epoch, u64::try_from(sequence)?))
    }
    pub fn set_cursor(&self, peer: &str, epoch: &str, sequence: u64) -> Result<()> {
        self.connection.execute("INSERT INTO cursors VALUES(?1,?2,?3) ON CONFLICT(peer) DO UPDATE SET epoch=excluded.epoch,seq=excluded.seq", params![peer,epoch,i64::try_from(sequence)?])?;
        Ok(())
    }
    pub fn status(&self) -> Result<serde_json::Value> {
        let entries: Vec<_> = self
            .all_paths()?
            .iter()
            .map(|p| self.local(p).map(|l| l.unwrap().record))
            .collect::<Result<_>>()?;
        let pending: Vec<String> = {
            let mut s = self
                .connection
                .prepare("SELECT path FROM pending ORDER BY path")?;
            s.query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        Ok(
            serde_json::json!({"folder":self.config.id,"root":self.config.root,"epoch":self.config.epoch,"sequence":self.sequence()?,"entries":entries,"pending_deletions":pending,"recovery_pending":self.recovery_pending()?,"sqlite":rusqlite::version()}),
        )
    }
    pub fn conflicts(&self) -> Result<serde_json::Value> {
        let mut conflicts = Vec::new();
        let mut statement = self.connection.prepare("SELECT path,versions FROM entries WHERE json_array_length(versions,'$.heads')>1 ORDER BY path")?;
        let rows =
            statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (path, json) = row?;
            let versions: Versions = serde_json::from_str(&json)?;
            let heads = &versions.heads;
            if heads.len() < 2 || heads.iter().all(|h| h.content == heads[0].content) {
                continue;
            }
            let revisions: Vec<_> = heads.iter().map(|h| {
                let object = if let Content::File(hash) = &h.content { Some(self.object_path(hash)?) } else { None };
                Ok(serde_json::json!({"id":h.id()?,"content":h.content,"clock":h.clock,"object_path":object}))
            }).collect::<Result<_>>()?;
            conflicts.push(serde_json::json!({"path":path,"revisions":revisions}));
        }
        Ok(serde_json::Value::Array(conflicts))
    }
}

/// Bounded summaries can be read while a synchronization holds the writer lock.
pub fn catalog(state: &Path) -> Result<serde_json::Value> {
    let mut folders = Vec::new();
    let shares = state.join("shares");
    if shares.try_exists()? {
        for entry in fs::read_dir(shares)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            ensure!(folders.len() < 1024, "too many registered folders");
            let summary = (|| -> Result<serde_json::Value> {
                let config = read_config(&entry.path())?;
                let db = Connection::open_with_flags(
                    entry.path().join("index.sqlite"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )?;
                let files: i64 = db.query_row(
                    "SELECT count(*) FROM entries WHERE json_extract(materialized,'$.kind')='file'",
                    [],
                    |r| r.get(0),
                )?;
                let pending: i64 =
                    db.query_row("SELECT count(*) FROM pending", [], |r| r.get(0))?;
                let conflicts: i64 = db.query_row(
                    "SELECT count(*) FROM entries WHERE json_array_length(versions,'$.heads')>1",
                    [],
                    |r| r.get(0),
                )?;
                let sequence: String =
                    db.query_row("SELECT value FROM meta WHERE key='seq'", [], |r| r.get(0))?;
                Ok(
                    serde_json::json!({"folder":config.id,"root":config.root,"mode":config.mode,"peers":config.peers,"files":files,"pending_deletions":pending,"concurrent_paths":conflicts,"sequence":sequence}),
                )
            })();
            folders.push(match summary {
                Ok(value) => value,
                Err(error) => serde_json::json!({"folder":entry.file_name().to_string_lossy(),"error":format!("{error:#}")}),
            });
        }
    }
    folders.sort_by_key(|f| f["folder"].as_str().unwrap_or_default().to_owned());
    Ok(
        serde_json::json!({"identity":identity::fingerprint(&fs::read(state.join("identity.der"))?),"version":env!("CARGO_PKG_VERSION"),"folders":folders}),
    )
}

/// Central configuration retries may reuse an identical registration only.
pub(crate) fn ensure_registration(state: &Path, id: &str, root: &Path, mode: Mode) -> Result<()> {
    ensure!(root.is_absolute(), "use the device folder's absolute path");
    let directory = directory(state, id)?;
    if directory.try_exists()? {
        let config = read_config(&directory)?;
        ensure!(
            same_file::is_same_file(&config.root, root)? && config.mode == mode,
            "existing folder has a different path or mode; inspect it before redeploying"
        );
    } else {
        drop(Share::create(state, id, root, mode)?);
    }
    Ok(())
}
