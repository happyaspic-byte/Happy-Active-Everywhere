//! Deliberately narrow framing; never extract general archive paths.
use super::*;
const MAGIC: &[u8] = b"EVERYWHERE-DEVICE-1\n";
const MAX_HEADER: usize = 4 * 1024 * 1024;

pub(super) fn publish_directory(source: &Path, target: &Path) -> Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use rustix::fs::{CWD, RenameFlags, renameat_with};
        renameat_with(CWD, source, CWD, target, RenameFlags::NOREPLACE)?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        ensure!(
            !source[..source.len() - 1].contains(&0) && !target[..target.len() - 1].contains(&0),
            "invalid output path"
        );
        // SAFETY: live NUL-terminated UTF-16 arrays; no replace, cross-volume
        // copy or deferred-reboot flags. Both directories share a parent.
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    anyhow::bail!("atomic device recovery publication is unsupported on this OS");
    Ok(())
}

fn write_record(output: &mut impl Write, path: &str, source: &Path) -> Result<()> {
    regular(source)?;
    let mut input = File::open(source)?;
    let length = input.metadata()?.len();
    output.write_all(&(path.len() as u32).to_be_bytes())?;
    output.write_all(path.as_bytes())?;
    output.write_all(&length.to_be_bytes())?;
    ensure!(
        std::io::copy(&mut input, output)? == length,
        "checkpoint changed during encoding"
    );
    Ok(())
}
pub(super) fn encode(
    header: &Header,
    staging: &Path,
    recipient: &age::x25519::Recipient,
    output: &Path,
) -> Result<()> {
    let mut temporary = tempfile::Builder::new()
        .prefix(".everywhere-ciphertext-")
        .tempfile_in(output.parent().unwrap())?;
    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(recipient as &dyn age::Recipient))?;
    let mut writer = encryptor.wrap_output(temporary.as_file_mut())?;
    writer.write_all(MAGIC)?;
    let bytes = Zeroizing::new(serde_json::to_vec(header)?);
    ensure!(bytes.len() <= MAX_HEADER, "device header too large");
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(&bytes)?;
    for id in &header.folders {
        let prefix = format!("folders/{id}");
        for name in ["manifest.json", "index.sqlite"] {
            write_record(
                &mut writer,
                &format!("{prefix}/{name}"),
                &staging.join(&prefix).join(name),
            )?;
        }
        for entry in fs::read_dir(staging.join(&prefix).join("objects"))? {
            let entry = entry?;
            let hash = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("invalid object filename"))?;
            identity::valid_peer(&hash)?;
            write_record(
                &mut writer,
                &format!("{prefix}/objects/{hash}"),
                &entry.path(),
            )?;
        }
    }
    writer.write_all(&0u32.to_be_bytes())?;
    writer.finish()?.sync_all()?;
    temporary.persist_noclobber(output).map_err(|e| e.error)?;
    sync_dir(output.parent().unwrap())
}
fn number<const N: usize>(input: &mut impl Read) -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn extract(input: &mut impl Read, staging: &Path) -> Result<Header> {
    let mut magic = vec![0; MAGIC.len()];
    input.read_exact(&mut magic)?;
    ensure!(magic == MAGIC, "invalid device backup magic");
    let length = u32::from_be_bytes(number(input)?) as usize;
    ensure!(length <= MAX_HEADER, "device header too large");
    let mut bytes = Zeroizing::new(vec![0; length]);
    input.read_exact(&mut bytes)?;
    let header: Header = serde_json::from_slice(&bytes).context("invalid device archive header")?;
    header.validate()?;
    private_dir(&staging.join("folders"))?;
    for id in &header.folders {
        private_dir(&staging.join("folders").join(id))?;
        private_dir(&staging.join("folders").join(id).join("objects"))?;
    }
    loop {
        let length = u32::from_be_bytes(number(input)?) as usize;
        if length == 0 {
            break;
        }
        ensure!(length <= 256, "archive path too long");
        let mut name = vec![0; length];
        input.read_exact(&mut name)?;
        let name = std::str::from_utf8(&name)?;
        let parts: Vec<_> = name.split('/').collect();
        ensure!(
            parts.len() >= 3 && parts[0] == "folders" && header.folders.contains(parts[1]),
            "unexpected archive path"
        );
        match &parts[2..] {
            ["manifest.json" | "index.sqlite"] => {}
            ["objects", hash] => identity::valid_peer(hash)?,
            _ => anyhow::bail!("unexpected archive path"),
        }
        let size = u64::from_be_bytes(number(input)?);
        // Manifest is bounded before decoding; content and SQLite are streamed.
        ensure!(
            parts[2] != "manifest.json" || size <= 128 * 1024,
            "manifest too large"
        );
        let mut output =
            private_file(&staging.join(name)).context("duplicate or unsafe archive entry")?;
        ensure!(
            std::io::copy(&mut input.take(size), &mut output)? == size,
            "truncated archive record"
        );
        output.sync_all()?;
    }
    ensure!(
        input.read(&mut [0u8; 1])? == 0,
        "trailing archive plaintext"
    );
    Ok(header)
}
pub(super) fn decode(
    backup: &Path,
    key: &Path,
    parent: Option<&Path>,
) -> Result<(tempfile::TempDir, Header)> {
    regular(backup)?;
    let key = Zeroizing::new(bounded(key, 4096)?);
    let key: age::x25519::Identity = std::str::from_utf8(&key)?
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid age X25519 identity file"))?;
    let decryptor = age::Decryptor::new(File::open(backup)?)?;
    let mut reader = decryptor.decrypt(std::iter::once(&key as &dyn age::Identity))?;
    let staging = staging(".everywhere-decrypted-", parent)?;
    let header = extract(&mut reader, staging.path())?;
    // Authentication at EOF precedes all SQLite parsing/materialization.
    header.summary(staging.path())?;
    Ok((staging, header))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Header, age::x25519::Identity) {
        let base = tempfile::tempdir().unwrap();
        let state = base.path().join("source");
        identity::init(&state).unwrap();
        let header = Header {
            format: 1,
            certificate: fs::read(state.join("identity.der")).unwrap(),
            private_key: fs::read(state.join("identity.key.der")).unwrap(),
            peers: BTreeMap::new(),
            jobs: BTreeMap::new(),
            folders: BTreeSet::new(),
        };
        let key = age::x25519::Identity::generate();
        write_private(
            &base.path().join("key"),
            key.to_string().expose_secret().as_bytes(),
        )
        .unwrap();
        (base, header, key)
    }
    fn plaintext(header: &Header) -> Vec<u8> {
        let json = serde_json::to_vec(header).unwrap();
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&(json.len() as u32).to_be_bytes());
        bytes.extend(json);
        bytes
    }
    fn record(bytes: &mut Vec<u8>, name: &str, data: &[u8]) {
        bytes.extend_from_slice(&(name.len() as u32).to_be_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&(data.len() as u64).to_be_bytes());
        bytes.extend_from_slice(data);
    }
    fn ciphertext(base: &Path, key: &age::x25519::Identity, bytes: &[u8]) -> PathBuf {
        let path = base.join(crate::root::random_id().unwrap());
        let recipient = key.to_public();
        let encryptor =
            age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
                .unwrap();
        let mut writer = encryptor.wrap_output(private_file(&path).unwrap()).unwrap();
        writer.write_all(bytes).unwrap();
        writer.finish().unwrap();
        path
    }
    #[test]
    fn authenticated_archives_reject_paths_duplicates_and_trailing_plaintext() {
        let (base, mut header, key) = fixture();
        header.folders.insert("personal".into());
        for name in [
            "../escape",
            "folders/personal/../../escape",
            "folders/personal/objects/not-a-hash",
            "folders/unknown/index.sqlite",
            "folders/personal/config.json",
        ] {
            let mut bytes = plaintext(&header);
            record(&mut bytes, name, b"payload");
            bytes.extend(0u32.to_be_bytes());
            let path = ciphertext(base.path(), &key, &bytes);
            assert!(
                decode(&path, &base.path().join("key"), Some(base.path())).is_err(),
                "accepted {name}"
            );
        }
        let mut bytes = plaintext(&header);
        record(&mut bytes, "folders/personal/manifest.json", b"{}");
        record(&mut bytes, "folders/personal/manifest.json", b"{}");
        bytes.extend(0u32.to_be_bytes());
        let path = ciphertext(base.path(), &key, &bytes);
        assert!(
            decode(&path, &base.path().join("key"), Some(base.path()))
                .err()
                .unwrap()
                .to_string()
                .contains("duplicate")
        );
        header.folders.clear();
        let mut bytes = plaintext(&header);
        bytes.extend(0u32.to_be_bytes());
        bytes.push(1);
        let path = ciphertext(base.path(), &key, &bytes);
        assert!(
            decode(&path, &base.path().join("key"), Some(base.path()))
                .err()
                .unwrap()
                .to_string()
                .contains("trailing")
        );
        assert!(!base.path().join("escape").exists());
    }
    #[test]
    fn authenticated_archive_still_validates_sqlite_schema() {
        let (base, mut header, key) = fixture();
        header.folders.insert("personal".into());
        let db = base.path().join("malicious.sqlite");
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch("CREATE TABLE unexpected(secret TEXT)")
            .unwrap();
        drop(connection);
        let config = share::Config {
            id: "personal".into(),
            root: PathBuf::from("/untrusted/source"),
            identity: identity::fingerprint(&header.certificate),
            epoch: "a".repeat(64),
            marker: "b".repeat(64),
            mode: crate::versions::Mode::Bidirectional,
            peers: BTreeSet::new(),
        };
        let manifest = json!({"format":1,"config":config,"database_hash":crate::storage::Manifest::from_path(&db).unwrap().hash,"objects":0});
        let mut bytes = plaintext(&header);
        record(
            &mut bytes,
            "folders/personal/manifest.json",
            &serde_json::to_vec(&manifest).unwrap(),
        );
        record(
            &mut bytes,
            "folders/personal/index.sqlite",
            &fs::read(db).unwrap(),
        );
        bytes.extend(0u32.to_be_bytes());
        let path = ciphertext(base.path(), &key, &bytes);
        assert!(decode(&path, &base.path().join("key"), Some(base.path())).is_err());
    }
    #[test]
    fn publication_never_replaces_a_competing_file_or_directory() {
        let base = tempfile::tempdir().unwrap();
        let source = base.path().join("source");
        private_dir(&source).unwrap();
        write_private(&source.join("payload"), b"restored").unwrap();
        let target = base.path().join("target");
        write_private(&target, b"other user's file").unwrap();
        assert!(publish_directory(&source, &target).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"other user's file");
        fs::remove_file(&target).unwrap();
        private_dir(&target).unwrap();
        assert!(publish_directory(&source, &target).is_err());
        assert!(source.join("payload").is_file());
        assert!(fs::read_dir(target).unwrap().next().is_none());
    }
}
