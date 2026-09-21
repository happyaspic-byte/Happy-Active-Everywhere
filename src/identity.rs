use anyhow::{Context, Result, bail, ensure};
use rustls::{
    ClientConfig, RootCertStore, ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer},
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

pub fn fingerprint(der: &[u8]) -> String {
    blake3::hash(der).to_hex().to_string()
}
pub(crate) fn valid_peer(peer: &str) -> Result<()> {
    ensure!(
        peer.len() == 64
            && peer
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid peer fingerprint"
    );
    Ok(())
}
fn private_write(path: &Path, data: &[u8]) -> Result<()> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(data)?;
    file.sync_all()?;
    Ok(())
}
pub fn init(state: &Path) -> Result<String> {
    fs::create_dir(state)
        .context("state directory must be new; existing identity is never replaced")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(state, fs::Permissions::from_mode(0o700))?;
    }
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["everywhere.local".into()])?;
    private_write(
        &state.join("identity.key.der"),
        &signing_key.serialize_der(),
    )?;
    private_write(&state.join("identity.der"), cert.der())?;
    fs::create_dir(state.join("peers"))?;
    Ok(fingerprint(cert.der()))
}
fn regular_read(path: &Path) -> Result<Vec<u8>> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "identity and trust files must be regular files"
    );
    let data = fs::read(path)?;
    ensure!(data.len() <= 64 * 1024, "certificate/key too large");
    Ok(data)
}
pub fn trust(state: &Path, cert: &Path) -> Result<String> {
    let _device = crate::device::config_guard(state)?;
    let der = regular_read(cert)?;
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(der.clone()))?;
    let id = fingerprint(&der);
    private_write(&state.join("peers").join(format!("{id}.der")), &der)?;
    Ok(id)
}
pub fn revoke(state: &Path, peer: &str) -> Result<()> {
    let _device = crate::device::config_guard(state)?;
    valid_peer(peer)?;
    fs::remove_file(state.join("peers").join(format!("{peer}.der")))?;
    Ok(())
}
pub(crate) const PROTOCOL: &[u8] = b"everywhere/2";

#[derive(Clone)]
pub struct Identity {
    state: PathBuf,
    pub peer: String,
}
impl Identity {
    pub fn new(state: &Path, peer: &str) -> Result<Self> {
        valid_peer(peer)?;
        let result = Self {
            state: state.into(),
            peer: peer.into(),
        };
        result.roots()?;
        Ok(result)
    }
    fn roots(&self) -> Result<RootCertStore> {
        let bytes = regular_read(&self.state.join("peers").join(format!("{}.der", self.peer)))
            .context("peer is unapproved or revoked")?;
        ensure!(
            fingerprint(&bytes) == self.peer,
            "trust file fingerprint mismatch"
        );
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(bytes))?;
        Ok(roots)
    }
    fn own(&self) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        let cert = regular_read(&self.state.join("identity.der"))?;
        let key = regular_read(&self.state.join("identity.key.der"))?;
        Ok((
            vec![CertificateDer::from(cert)],
            PrivateKeyDer::Pkcs8(key.into()),
        ))
    }
    pub fn server(&self) -> Result<Arc<ServerConfig>> {
        self.server_protocol(PROTOCOL)
    }
    pub(crate) fn server_protocol(&self, protocol: &[u8]) -> Result<Arc<ServerConfig>> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(self.roots()?),
            provider.clone(),
        )
        .build()?;
        let (cert, key) = self.own()?;
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_client_cert_verifier(verifier)
            .with_single_cert(cert, key)?;
        config.alpn_protocols = vec![protocol.to_vec()];
        config.send_tls13_tickets = 0;
        Ok(Arc::new(config))
    }
    pub fn client(&self) -> Result<Arc<ClientConfig>> {
        self.client_protocol(PROTOCOL)
    }
    pub(crate) fn client_protocol(&self, protocol: &[u8]) -> Result<Arc<ClientConfig>> {
        let (cert, key) = self.own()?;
        let mut config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])?
                .with_root_certificates(self.roots()?)
                .with_client_auth_cert(cert, key)?;
        config.alpn_protocols = vec![protocol.to_vec()];
        config.resumption = rustls::client::Resumption::disabled();
        Ok(Arc::new(config))
    }
    pub fn check(&self, certificates: Option<&[CertificateDer<'_>]>) -> Result<()> {
        self.roots()?;
        match certificates.and_then(|c| c.first()) {
            Some(cert) if fingerprint(cert) == self.peer => Ok(()),
            _ => bail!("peer certificate does not match approval"),
        }
    }
}
