//! Consistent folder checkpoints and restartable index replacement.
use super::*;
use std::time::{Duration, Instant};

mod validate;

const INTENT: &str = "state-recovery.json";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    format: u32,
    config: Config,
    database_hash: String,
    objects: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recovery {
    format: u32,
    nonce: String,
    database_hash: String,
    config: Config,
}
fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn private_dir(path: &Path) -> Result<()> {
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = builder;
        builder.mode(0o700);
        builder
    };
    builder.create(path)?;
    sync_dir(path.parent().context("directory has no parent")?)
}
fn regular(path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "expected regular file: {}",
        path.display()
    );
    Ok(())
}
fn real_dir(path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_dir(),
        "expected non-link directory: {}",
        path.display()
    );
    Ok(())
}
fn digest(path: &Path) -> Result<String> {
    regular(path)?;
    Ok(Manifest::from_path(path)?.hash)
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    regular(path)?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take(128 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 128 * 1024, "recovery metadata too large");
    Ok(serde_json::from_slice(&bytes)?)
}
fn lock(directory: &Path, name: &str) -> Result<File> {
    let path = directory.join(name);
    if path.try_exists()? {
        regular(&path)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock_exclusive()
        .context("share is busy; pause active operations and retry")?;
    Ok(file)
}
fn copy_private(source: &Path, target: &Path, hash: &str) -> Result<()> {
    regular(source)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(target)?;
    std::io::copy(&mut File::open(source)?, &mut output)?;
    output.sync_all()?;
    ensure!(
        digest(target)? == hash,
        "checkpoint content changed or is corrupt: {}",
        source.display()
    );
    sync_dir(target.parent().unwrap())
}
fn snapshot(source: &Connection, path: &Path) -> Result<()> {
    write_new(path, &[])?;
    let mut destination = Connection::open(path)?;
    {
        let backup = rusqlite::backup::Backup::new(source, &mut destination)?;
        let mut progress = Instant::now();
        loop {
            match backup.step(128)? {
                rusqlite::backup::StepResult::Done => break,
                rusqlite::backup::StepResult::More => progress = Instant::now(),
                rusqlite::backup::StepResult::Busy | rusqlite::backup::StepResult::Locked => {
                    ensure!(
                        progress.elapsed() < Duration::from_secs(10),
                        "checkpoint database remained busy"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => anyhow::bail!("unknown SQLite backup result"),
            }
        }
    }
    destination.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; DELETE FROM incoming; DELETE FROM needed;")?;
    drop(destination);
    // Windows requires write access for FlushFileBuffers.
    OpenOptions::new().write(true).open(path)?.sync_all()?;
    sync_dir(path.parent().unwrap())
}
fn no_overlap(state: &Path, path: &Path) -> Result<()> {
    let state = state.canonicalize()?;
    ensure!(
        !path.starts_with(&state) && !state.starts_with(path),
        "checkpoint and device state must not overlap"
    );
    for entry in fs::read_dir(state.join("shares"))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let config = read_config(&entry.path())?;
            ensure!(
                !path.starts_with(&config.root) && !config.root.starts_with(path),
                "checkpoint and data roots must not overlap"
            );
        }
    }
    Ok(())
}
fn binding(state: &Path, id: &str, config: &Config) -> Result<()> {
    ensure!(
        config.id == id
            && config.identity == identity::fingerprint(&fs::read(state.join("identity.der"))?),
        "checkpoint device/folder identity mismatch"
    );
    let root = Root::open(&config.root)?;
    ensure!(
        root.dir
            .symlink_metadata(".everywhere-folder")?
            .file_type()
            .is_file(),
        "share marker unavailable"
    );
    ensure!(
        root.dir.read_to_string(".everywhere-folder")? == config.marker,
        "share mount identity changed"
    );
    Ok(())
}
fn object_hashes(
    connection: &Connection,
    mut visit: impl FnMut(&str) -> Result<()>,
) -> Result<u64> {
    let mut statement = connection.prepare("SELECT DISTINCT hash FROM (
        SELECT json_extract(h.value,'$.content.hash') hash FROM entries e,json_each(e.versions,'$.heads') h
        UNION SELECT json_extract(h.value,'$.content.hash') FROM entries e,json_each(e.observed,'$.heads') h
        UNION SELECT json_extract(materialized,'$.hash') FROM entries
        UNION SELECT json_extract(revision,'$.content.hash') FROM history
        ) WHERE hash IS NOT NULL ORDER BY hash")?;
    let mut count = 0;
    for hash in statement.query_map([], |r| r.get::<_, String>(0))? {
        let hash = hash?;
        identity::valid_peer(&hash)?;
        visit(&hash)?;
        count += 1;
    }
    Ok(count)
}
fn open_checkpoint(path: &Path) -> Result<(Checkpoint, Connection)> {
    real_dir(path)?;
    real_dir(&path.join("objects"))?;
    let manifest: Checkpoint = read_json(&path.join("manifest.json"))?;
    ensure!(manifest.format == 1, "unsupported checkpoint format");
    identity::valid_peer(&manifest.database_hash)?;
    identity::valid_peer(&manifest.config.epoch)?;
    let database = path.join("index.sqlite");
    ensure!(
        digest(&database)? == manifest.database_hash,
        "checkpoint index checksum mismatch"
    );
    for suffix in ["-wal", "-shm", "-journal"] {
        ensure!(
            !path.join(format!("index.sqlite{suffix}")).try_exists()?,
            "checkpoint has an unexpected SQLite sidecar"
        );
    }
    let connection =
        Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    validate::database(&connection, &manifest.config.epoch)?;
    let count = object_hashes(&connection, |hash| {
        ensure!(
            digest(&path.join("objects").join(hash))? == hash,
            "checkpoint object is corrupt: {hash}"
        );
        Ok(())
    })?;
    ensure!(
        count == manifest.objects,
        "checkpoint object count mismatch"
    );
    Ok((manifest, connection))
}

pub fn create(state: &Path, id: &str, output: &Path) -> Result<serde_json::Value> {
    let share = Share::open(state, id)?;
    let output = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()?
        .join(
            output
                .file_name()
                .context("checkpoint requires a new directory name")?,
        );
    no_overlap(state, &output)?;
    let objects = capture(&share, &output)?;
    Ok(
        serde_json::json!({"status":"checkpoint-complete","folder":id,"output":output,"objects":objects}),
    )
}

pub(crate) fn verify(path: &Path) -> Result<Config> {
    let (checkpoint, _) = open_checkpoint(path)?;
    ensure!(
        fs::read_dir(path.join("objects"))?.count() as u64 == checkpoint.objects,
        "unexpected checkpoint objects"
    );
    Ok(checkpoint.config)
}

pub(crate) fn capture(share: &Share, output: &Path) -> Result<u64> {
    private_dir(output).context("checkpoint destination must be new")?;
    private_dir(&output.join("objects"))?;
    let database = output.join("index.sqlite");
    snapshot(&share.connection, &database)?;
    let connection =
        Connection::open_with_flags(&database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    validate::database(&connection, &share.config.epoch)?;
    let objects = object_hashes(&connection, |hash| {
        copy_private(
            &share.object_path(hash)?,
            &output.join("objects").join(hash),
            hash,
        )
    })?;
    share.check_root()?;
    let manifest = Checkpoint {
        format: 1,
        config: share.config.clone(),
        database_hash: digest(&database)?,
        objects,
    };
    write_new(
        &output.join("manifest.json"),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(objects)
}

/// Import into an unpublished private workspace; never use archived root paths.
pub(crate) fn import(
    checkpoint: &Path,
    state: &Path,
    staged_root: &Path,
    final_root: &Path,
) -> Result<()> {
    let (manifest, source) = open_checkpoint(checkpoint)?;
    valid_id(&manifest.config.id)?;
    let directory = directory(state, &manifest.config.id)?;
    private_dir(&directory)?;
    private_dir(&directory.join("objects"))?;
    private_dir(staged_root)?;
    object_hashes(&source, |hash| {
        copy_private(
            &checkpoint.join("objects").join(hash),
            &directory.join("objects").join(hash),
            hash,
        )
    })?;
    snapshot(&source, &directory.join("index.sqlite"))?;
    let mut config = manifest.config.clone();
    config.identity = identity::fingerprint(&fs::read(state.join("identity.der"))?);
    config.epoch = random_id()?;
    config.marker = random_id()?;
    config.root = final_root.into();
    config.peers.clear();
    config.mode = Mode::ReceiveOnly;
    let database = Connection::open(directory.join("index.sqlite"))?;
    database.execute_batch("PRAGMA synchronous=FULL; DELETE FROM cursors; INSERT OR REPLACE INTO meta VALUES('recovery_pending','1');")?;
    database.execute(
        "UPDATE meta SET value=?1 WHERE key='epoch'",
        [&config.epoch],
    )?;
    write_new(
        &staged_root.join(".everywhere-folder"),
        config.marker.as_bytes(),
    )?;
    let root = Root::open(staged_root)?;
    // Restore materialized bytes, not a causal head awaiting application.
    let mut statement = source.prepare("SELECT path,materialized FROM entries WHERE materialized IS NOT NULL AND path NOT IN (SELECT path FROM pending) ORDER BY length(path),path")?;
    for row in statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (path, json) = row?;
        let content: Content = serde_json::from_str(&json)?;
        let object = match &content {
            Content::File(hash) => Some(directory.join("objects").join(hash)),
            _ => None,
        };
        root.apply(&path, None, &content, object.as_deref())?;
    }
    write_new(
        &directory.join("config.json"),
        &serde_json::to_vec_pretty(&config)?,
    )?;
    drop(database);
    OpenOptions::new()
        .write(true)
        .open(directory.join("index.sqlite"))?
        .sync_all()?;
    sync_dir(&directory)?;
    sync_dir(staged_root)
}

pub(crate) fn activate_device(
    state: &Path,
    id: &str,
    mode: Mode,
    offline: bool,
) -> Result<serde_json::Value> {
    let _device = crate::device::config_guard(state)?;
    let share = Share::open(state, id)?;
    let _config = lock(&share.directory, "config.lock")?;
    ensure!(
        offline || !share.recovery_pending()?,
        "restored device must fully reconcile first; offline authority is an explicit override"
    );
    let mut config = read_config(&share.directory)?;
    config.mode = mode;
    let temporary = share.directory.join(format!("config-{}.tmp", random_id()?));
    write_new(&temporary, &serde_json::to_vec_pretty(&config)?)?;
    // Interrupted activation retains the previous receive-only mode.
    if offline {
        share.connection.execute(
            "INSERT OR REPLACE INTO meta VALUES('recovery_pending','0')",
            [],
        )?;
    }
    fs::rename(temporary, share.directory.join("config.json"))?;
    sync_dir(&share.directory)?;
    Ok(
        serde_json::json!({"status":"device-folder-activated","folder":id,"mode":mode,"offline_authority":offline}),
    )
}

pub fn recover(state: &Path, id: &str, checkpoint: &Path) -> Result<serde_json::Value> {
    let directory = directory(state, id)?;
    real_dir(&directory)?;
    let _writer = lock(&directory, "index.lock")?;
    resume(state, id, &directory)?;
    let _config_lock = lock(&directory, "config.lock")?;
    let intent = prepare(state, id, &directory, checkpoint)?;
    finish(state, id, &directory, &intent)?;
    Ok(
        serde_json::json!({"status":"index-recovered","folder":id,"epoch":intent.config.epoch,
        "previous_index":directory.join(format!("index-before-{}",intent.nonce)),
        "recovery_pending":intent.config.mode != Mode::SendOnly}),
    )
}
fn prepare(state: &Path, id: &str, directory: &Path, checkpoint: &Path) -> Result<Recovery> {
    let config = read_config(directory)?;
    binding(state, id, &config)?;
    real_dir(checkpoint)?;
    let checkpoint = checkpoint.canonicalize()?;
    no_overlap(state, &checkpoint)?;
    let (manifest, database) = open_checkpoint(&checkpoint)?;
    ensure!(
        manifest.config.id == config.id
            && manifest.config.identity == config.identity
            && manifest.config.root == config.root
            && manifest.config.marker == config.marker,
        "checkpoint belongs to another device, folder or root"
    );
    let nonce = random_id()?;
    let archive = directory.join(format!("index-before-{nonce}"));
    private_dir(&archive)?;
    real_dir(&directory.join("objects"))?;
    // Retain all newer objects. Repair corrupt cached bytes only after saving
    // them to this operation's private archive; never link a backup into state.
    object_hashes(&database, |hash| {
        let target = directory.join("objects").join(hash);
        if target.try_exists()? {
            if digest(&target)? == hash {
                return Ok(());
            }
            fs::rename(&target, archive.join(format!("corrupt-object-{hash}")))?;
            sync_dir(&archive)?;
            sync_dir(&directory.join("objects"))?;
        }
        let temporary = archive.join(format!("object-{hash}"));
        copy_private(&checkpoint.join("objects").join(hash), &temporary, hash)?;
        fs::hard_link(&temporary, &target)?;
        sync_dir(&directory.join("objects"))?;
        fs::remove_file(temporary)?;
        Ok(())
    })?;
    let candidate = directory.join(format!("index-recover-{nonce}.sqlite"));
    copy_private(
        &checkpoint.join("index.sqlite"),
        &candidate,
        &manifest.database_hash,
    )?;
    let new_epoch = random_id()?;
    {
        let connection = Connection::open(&candidate)?;
        connection.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL;")?;
        let transaction = connection.unchecked_transaction()?;
        connection.execute("UPDATE meta SET value=?1 WHERE key='epoch'", [&new_epoch])?;
        connection
            .execute_batch("DELETE FROM cursors; DELETE FROM incoming; DELETE FROM needed;")?;
        connection.execute("INSERT INTO meta VALUES('recovery_pending',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [if config.mode == Mode::SendOnly { "0" } else { "1" }])?;
        transaction.commit()?;
        validate::database(&connection, &new_epoch)?;
    }
    OpenOptions::new()
        .write(true)
        .open(&candidate)?
        .sync_all()?;
    let mut next_config = config;
    next_config.epoch = new_epoch.clone();
    let intent = Recovery {
        format: 1,
        nonce,
        database_hash: digest(&candidate)?,
        config: next_config,
    };
    // Validate the live binding again before committing the replacement intent.
    binding(state, id, &intent.config)?;
    publish_intent(directory, &intent)?;
    Ok(intent)
}
fn publish_intent(directory: &Path, intent: &Recovery) -> Result<()> {
    let staging = directory.join(format!("state-recovery-{}.tmp", intent.nonce));
    write_new(&staging, &serde_json::to_vec_pretty(intent)?)?;
    fs::hard_link(&staging, directory.join(INTENT))?;
    sync_dir(directory)?;
    fs::remove_file(staging)?;
    sync_dir(directory)
}

pub(super) fn resume(state: &Path, id: &str, directory: &Path) -> Result<()> {
    if !directory.join(INTENT).try_exists()? {
        return Ok(());
    }
    let _config_lock = lock(directory, "config.lock")?;
    let intent: Recovery = read_json(&directory.join(INTENT))?;
    finish(state, id, directory, &intent)
}
fn finish(state: &Path, id: &str, directory: &Path, intent: &Recovery) -> Result<()> {
    ensure!(intent.format == 1, "unsupported state recovery intent");
    identity::valid_peer(&intent.nonce)?;
    identity::valid_peer(&intent.database_hash)?;
    identity::valid_peer(&intent.config.epoch)?;
    binding(state, id, &intent.config)?;
    let current = read_config(directory)?;
    ensure!(
        current.id == intent.config.id
            && current.root == intent.config.root
            && current.identity == intent.config.identity
            && current.marker == intent.config.marker
            && current.mode == intent.config.mode
            && current.peers == intent.config.peers,
        "share configuration changed during index recovery; preserving both states"
    );
    let archive = directory.join(format!("index-before-{}", intent.nonce));
    real_dir(&archive)?;
    let candidate = directory.join(format!("index-recover-{}.sqlite", intent.nonce));
    let active = directory.join("index.sqlite");
    if candidate.try_exists()? {
        ensure!(
            digest(&candidate)? == intent.database_hash,
            "prepared recovery index is corrupt"
        );
        let published = active.try_exists()? && same_file::is_same_file(&candidate, &active)?;
        if !published {
            for name in [
                "index.sqlite-wal",
                "index.sqlite-shm",
                "index.sqlite-journal",
                "index.sqlite",
            ] {
                let source = directory.join(name);
                let target = archive.join(name);
                if source.try_exists()? {
                    regular(&source)?;
                    ensure!(
                        !target.try_exists()?,
                        "index recovery found two different generations; preserving both"
                    );
                    fs::rename(&source, &target)?;
                    sync_dir(&archive)?;
                    sync_dir(directory)?;
                }
            }
            // Publication is no-clobber. Keep the prepared path until publication
            // is durable; a restart recognizes the identical linked file below.
            fs::hard_link(&candidate, &active)?;
            sync_dir(directory)?;
        }
        fs::remove_file(&candidate)?;
        sync_dir(directory)?;
    }
    ensure!(
        digest(&active)? == intent.database_hash,
        "published recovery index differs from the intent"
    );
    // A prior attempt may have left a partial scratch file. A fresh private
    // scratch name lets us retry without trusting, overwriting or deleting it.
    let config_path = directory.join(format!(
        "config-recover-{}-{}.tmp",
        intent.nonce,
        random_id()?
    ));
    let bytes = serde_json::to_vec_pretty(&intent.config)?;
    write_new(&config_path, &bytes)?;
    fs::rename(config_path, directory.join("config.json"))?;
    sync_dir(directory)?;
    fs::remove_file(directory.join(INTENT))?;
    sync_dir(directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn index_recovery_resumes_every_publication_boundary_without_double_displacement() {
        for boundary in 0..6 {
            let reports =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("folder-report/recovery-evidence");
            fs::create_dir_all(&reports).unwrap();
            let temp = tempfile::Builder::new()
                .prefix("publication-")
                .tempdir_in(reports)
                .unwrap();
            let result = std::panic::catch_unwind(|| {
                let state = temp.path().join("state");
                let root = temp.path().join("files");
                fs::create_dir(&root).unwrap();
                identity::init(&state).unwrap();
                let share = Share::create(&state, "personal", &root, Mode::Bidirectional).unwrap();
                fs::write(root.join("note"), b"original").unwrap();
                share.scan(false).unwrap();
                let expected = share.status().unwrap()["entries"].clone();
                drop(share);
                let checkpoint = temp.path().join("checkpoint");
                create(&state, "personal", &checkpoint).unwrap();
                let directory = directory(&state, "personal").unwrap();
                let old = fs::read(directory.join("index.sqlite")).unwrap();
                // prepare() is the same durable boundary called by the CLI.
                let intent = prepare(&state, "personal", &directory, &checkpoint).unwrap();
                let candidate = directory.join(format!("index-recover-{}.sqlite", intent.nonce));
                let archive = directory.join(format!("index-before-{}", intent.nonce));
                if boundary >= 1 {
                    fs::rename(directory.join("index.sqlite"), archive.join("index.sqlite"))
                        .unwrap();
                }
                if boundary >= 2 {
                    fs::hard_link(&candidate, directory.join("index.sqlite")).unwrap();
                }
                if boundary >= 3 {
                    fs::remove_file(&candidate).unwrap();
                }
                if boundary == 4 {
                    fs::write(
                        directory.join("config.json"),
                        serde_json::to_vec_pretty(&intent.config).unwrap(),
                    )
                    .unwrap();
                }
                if boundary == 5 {
                    fs::write(
                        directory.join(format!("config-recover-{}-incomplete.tmp", intent.nonce)),
                        b"{",
                    )
                    .unwrap();
                }
                // A new Share instance/process follows this exact open path.
                let recovered = Share::open(&state, "personal").unwrap();
                assert_eq!(
                    recovered.status().unwrap()["entries"],
                    expected,
                    "boundary {boundary}"
                );
                assert_eq!(recovered.config.epoch, intent.config.epoch);
                assert!(recovered.recovery_pending().unwrap());
                assert!(!directory.join(INTENT).exists());
                assert_eq!(
                    Sha256::digest(fs::read(archive.join("index.sqlite")).unwrap()),
                    Sha256::digest(&old)
                );
                assert_eq!(
                    Sha256::digest(fs::read(root.join("note")).unwrap()),
                    Sha256::digest(b"original")
                );
                drop(recovered);
                assert_eq!(
                    Share::open(&state, "personal").unwrap().status().unwrap()["entries"],
                    expected
                );
            });
            if let Err(error) = result {
                eprintln!(
                    "Boundary {boundary} evidence retained at {}",
                    temp.keep().display()
                );
                std::panic::resume_unwind(error);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn torn_intent_write_does_not_publish_a_final_journal() {
        if let Some(directory) = std::env::var_os("EVERYWHERE_TEST_TORN_INTENT") {
            let intent = Recovery {
                format: 1,
                nonce: "a".repeat(64),
                database_hash: "b".repeat(64),
                config: Config {
                    id: "personal".into(),
                    root: "/fixture".into(),
                    identity: "c".repeat(64),
                    epoch: "d".repeat(64),
                    marker: "e".repeat(64),
                    mode: Mode::Bidirectional,
                    peers: (0..128).map(|n| format!("{n:064x}")).collect(),
                },
            };
            // The OS file-size limit interrupts a real production journal write.
            publish_intent(Path::new(&directory), &intent).unwrap();
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let result = std::process::Command::new("sh")
            .args([
                "-c",
                "ulimit -c 0; ulimit -f 1; exec \"$@\"",
                "intent-fault",
            ])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "share::backup::tests::torn_intent_write_does_not_publish_a_final_journal",
                "--nocapture",
            ])
            .env("EVERYWHERE_TEST_TORN_INTENT", temp.path())
            .output()
            .unwrap();
        assert!(
            !result.status.success(),
            "fault did not interrupt the write"
        );
        assert!(
            !temp.path().join(INTENT).exists(),
            "an incomplete intent was published"
        );
    }
}
