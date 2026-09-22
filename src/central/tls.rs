//! Enrollment proves possession before registry approval. A valid self-signed
//! certificate is only a device identity, never authorization by itself.
use rustls::{
    DigitallySignedStruct, DistinguishedName, Error, RootCertStore, SignatureScheme,
    client::danger::HandshakeSignatureValid,
    pki_types::{CertificateDer, UnixTime},
    server::{
        WebPkiClientVerifier,
        danger::{ClientCertVerified, ClientCertVerifier},
    },
};
use std::sync::Arc;

#[derive(Debug)]
pub(super) struct EnrollmentVerifier;
fn verifier(cert: &CertificateDer<'_>) -> Result<Arc<dyn ClientCertVerifier>, Error> {
    if cert.len() > 8192 {
        return Err(Error::General("device certificate too large".into()));
    }
    let mut roots = RootCertStore::empty();
    roots.add(cert.clone().into_owned())?;
    WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots),
        Arc::new(rustls::crypto::ring::default_provider()),
    )
    .build()
    .map_err(|e| Error::General(e.to_string()))
}
impl ClientCertVerifier for EnrollmentVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        cert: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        if !intermediates.is_empty() {
            return Err(Error::General(
                "device must present its own single certificate".into(),
            ));
        }
        verifier(cert)?.verify_client_cert(cert, intermediates, now)
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verifier(cert)?.verify_tls12_signature(message, cert, signature)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verifier(cert)?.verify_tls13_signature(message, cert, signature)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
