use crate::{storage::Manifest, versions::Clock};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Change {
    Created,
    Modified,
    Deleted,
    DeletionPending,
    DeletionCancelled,
}
#[derive(Debug, Serialize)]
pub struct Event {
    pub path: String,
    pub change: Change,
}
#[derive(Debug, Serialize)]
pub struct Entry {
    pub path: String,
    pub clock: Clock,
    pub hash: Option<String>,
    pub pending_deletion: bool,
}
pub struct Index {
    connection: Connection,
    root: PathBuf,
    device: String,
    marker: String,
}
const MARKER: &str = ".everywhere-folder";
impl Index {
    pub fn create(db: &Path, root: &Path, device: &str) -> Result<Self> {
        Clock::default().advance(device)?;
        let root = root.canonicalize()?;
        ensure!(root.is_dir(), "folder root must be directory");
        ensure!(!db.exists(), "index already exists");
        let db_parent = db
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()?;
        ensure!(
            !db_parent.starts_with(&root),
            "index database must be outside synchronized folder"
        );
        let key = rcgen::KeyPair::generate()?;
        let marker = blake3::hash(&key.serialize_der()).to_hex().to_string();
        let mut marker_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(MARKER))?;
        marker_file.write_all(marker.as_bytes())?;
        marker_file.sync_all()?;
        #[cfg(unix)]
        File::open(&root)?.sync_all()?;
        let connection = Connection::open(db)?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE entries(path TEXT PRIMARY KEY,hash TEXT,clock TEXT NOT NULL);
            CREATE TABLE pending_deletions(path TEXT PRIMARY KEY);",
        )?;
        let root_string = root.to_str().context("folder path must be UTF-8")?;
        for (k, v) in [
            ("root", root_string),
            ("device", device),
            ("marker", &marker),
            ("schema", "1"),
        ] {
            connection.execute("INSERT INTO metadata VALUES(?1,?2)", params![k, v])?;
        }
        Ok(Self {
            connection,
            root,
            device: device.into(),
            marker,
        })
    }
    pub fn open(db: &Path, root: &Path, device: &str) -> Result<Self> {
        ensure!(
            fs::symlink_metadata(db)?.file_type().is_file(),
            "index must be a regular file"
        );
        let connection = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        connection.execute_batch("PRAGMA synchronous=FULL;")?;
        let check: String = connection.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        ensure!(
            check == "ok",
            "index integrity check failed; deletion propagation disabled"
        );
        let get = |key: &str| -> Result<String> {
            Ok(
                connection.query_row("SELECT value FROM metadata WHERE key=?1", [key], |r| {
                    r.get(0)
                })?,
            )
        };
        ensure!(get("schema")? == "1", "unsupported index schema");
        let root = root.canonicalize()?;
        ensure!(
            get("root")? == root.to_str().context("folder path must be UTF-8")?,
            "folder root mismatch"
        );
        ensure!(get("device")? == device, "device mismatch");
        let marker = get("marker")?;
        let result = Self {
            connection,
            root,
            device: device.into(),
            marker,
        };
        result.check_root()?;
        // Additive migration: existing alpha entries and clocks stay untouched.
        result.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS pending_deletions(path TEXT PRIMARY KEY);",
        )?;
        Ok(result)
    }
    fn check_root(&self) -> Result<()> {
        ensure!(
            fs::symlink_metadata(&self.root)?.file_type().is_dir(),
            "folder unavailable"
        );
        let path = self.root.join(MARKER);
        ensure!(
            fs::symlink_metadata(&path)?.file_type().is_file(),
            "folder marker unavailable"
        );
        let mut value = String::new();
        File::open(path)?.take(1024).read_to_string(&mut value)?;
        ensure!(value == self.marker, "folder identity changed; scan paused");
        Ok(())
    }
    pub fn entries(&self) -> Result<Vec<Entry>> {
        let mut statement = self
            .connection
            .prepare("SELECT path,hash,clock,EXISTS(SELECT 1 FROM pending_deletions p WHERE p.path=entries.path) FROM entries ORDER BY path")?;
        let rows = statement.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
            ))
        })?;
        let mut entries = Vec::new();
        for row in rows {
            let (path, hash, clock, pending_deletion) = row?;
            entries.push(Entry {
                path,
                hash,
                clock: serde_json::from_str(&clock)?,
                pending_deletion,
            });
        }
        Ok(entries)
    }
    pub fn scan(&mut self, allow_deletes: bool) -> Result<Vec<Event>> {
        self.check_root()?;
        let mut found = BTreeMap::new();
        collect(&self.root, &self.root, &mut found)?;
        self.check_root()?;
        let old: BTreeMap<String, Entry> = self
            .entries()?
            .into_iter()
            .map(|e| (e.path.clone(), e))
            .collect();
        let deleted: Vec<_> = old
            .values()
            .filter(|e| e.hash.is_some() && !found.contains_key(&e.path))
            .collect();
        let transaction = self.connection.transaction()?;
        let mut events = Vec::new();
        for (path, hash) in found {
            let prior = old.get(&path);
            if transaction.execute("DELETE FROM pending_deletions WHERE path=?1", [&path])? > 0 {
                events.push(Event {
                    path: path.clone(),
                    change: Change::DeletionCancelled,
                });
            }
            if prior.and_then(|e| e.hash.as_deref()) == Some(hash.as_str()) {
                continue;
            }
            let clock = prior
                .map(|e| e.clock.clone())
                .unwrap_or_default()
                .advance(&self.device)?;
            transaction.execute("INSERT INTO entries VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET hash=excluded.hash,clock=excluded.clock",params![path,hash,serde_json::to_string(&clock)?])?;
            events.push(Event {
                path,
                change: if prior.is_some_and(|e| e.hash.is_some()) {
                    Change::Modified
                } else {
                    Change::Created
                },
            });
        }
        for prior in deleted {
            if !allow_deletes {
                if transaction.execute(
                    "INSERT OR IGNORE INTO pending_deletions VALUES(?1)",
                    [&prior.path],
                )? > 0
                {
                    events.push(Event {
                        path: prior.path.clone(),
                        change: Change::DeletionPending,
                    });
                }
                continue;
            }
            let clock = prior.clock.advance(&self.device)?;
            transaction.execute(
                "UPDATE entries SET hash=NULL,clock=?2 WHERE path=?1",
                params![prior.path, serde_json::to_string(&clock)?],
            )?;
            transaction.execute("DELETE FROM pending_deletions WHERE path=?1", [&prior.path])?;
            events.push(Event {
                path: prior.path.clone(),
                change: Change::Deleted,
            });
        }
        transaction.commit()?;
        Ok(events)
    }
}
fn collect(root: &Path, dir: &Path, found: &mut BTreeMap<String, String>) -> Result<()> {
    for item in fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name();
        let name = name.to_str().context("non-UTF-8 filename unsupported")?;
        ensure!(
            !name.contains('\\'),
            "backslash filename is not portable; scan paused"
        );
        if name == MARKER || name.starts_with(".everywhere-") {
            continue;
        }
        let path = item.path();
        let kind = item.file_type()?;
        ensure!(
            !kind.is_symlink(),
            "symlink in synchronized folder; scan paused"
        );
        if kind.is_dir() {
            collect(root, &path, found)?;
        } else {
            ensure!(
                kind.is_file(),
                "special file in synchronized folder; scan paused"
            );
            let relative = path
                .strip_prefix(root)?
                .to_str()
                .context("non-UTF-8 path unsupported")?
                .replace('\\', "/");
            found.insert(relative, Manifest::from_path(&path)?.hash);
        }
    }
    Ok(())
}
