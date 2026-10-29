use crate::identity::{Identity, fingerprint};
use anyhow::Result;
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, DistinguishedName, Error, ServerConfig,
    SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{CryptoProvider, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
    server::danger::{ClientCertVerified, ClientCertVerifier},
};
use std::sync::Arc;

/// Validates certificate possession during bootstrap. Authorization is enforced
/// separately before any control message is processed; this is not PKI trust.
#[derive(Debug)]
struct CertificatePossession {
    provider: Arc<CryptoProvider>,
    expected: Option<String>,
}
impl CertificatePossession {
    fn check(
        &self,
        cert: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
    ) -> std::result::Result<(), Error> {
        if cert.is_empty() || cert.len() > 2048 || !intermediates.is_empty() {
            return Err(Error::InvalidCertificate(CertificateError::BadEncoding));
        }
        if self
            .expected
            .as_ref()
            .is_some_and(|fp| *fp != fingerprint(cert))
        {
            return Err(Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ));
        }
        Ok(())
    }
    fn signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
}
impl ServerCertVerifier for CertificatePossession {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<ServerCertVerified, Error> {
        self.check(end_entity, intermediates)?;
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.2 disabled".into()))
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        self.signature(m, c, d)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
impl ClientCertVerifier for CertificatePossession {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        c: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> std::result::Result<ClientCertVerified, Error> {
        self.check(c, chain)?;
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.2 disabled".into()))
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        self.signature(m, c, d)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
pub fn server(identity: &Identity) -> Result<Arc<ServerConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(CertificatePossession {
        provider: provider.clone(),
        expected: None,
    });
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![identity.certificate.clone()], identity.private_key())?;
    config.max_early_data_size = 0;
    config.send_tls13_tickets = 0;
    config.alpn_protocols = vec![b"ravel/1".to_vec()];
    config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    Ok(Arc::new(config))
}
pub fn client(identity: &Identity, expected: Option<String>) -> Result<Arc<ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(CertificatePossession {
        provider: provider.clone(),
        expected,
    });
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![identity.certificate.clone()], identity.private_key())?;
    config.enable_early_data = false;
    config.resumption = rustls::client::Resumption::disabled();
    config.alpn_protocols = vec![b"ravel/1".to_vec()];
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::{TcpListener, TcpStream};
    use tokio_rustls::{TlsAcceptor, TlsConnector};
    #[tokio::test]
    async fn mutual_certificates_and_exporters_match_and_pin_change_fails() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let a = Identity::load_or_create(a.path()).unwrap();
        let b = Identity::load_or_create(b.path()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_config = server(&b).unwrap();
        let server = async {
            let (s, _) = listener.accept().await.unwrap();
            let tls = TlsAcceptor::from(server_config).accept(s).await.unwrap();
            assert_eq!(
                fingerprint(&tls.get_ref().1.peer_certificates().unwrap()[0]),
                a.fingerprint
            );
            tls.get_ref()
                .1
                .export_keying_material([0u8; 32], b"EXPORTER-Ravel-pair-v1", None)
                .unwrap()
        };
        let client = async {
            let s = TcpStream::connect(addr).await.unwrap();
            let tls = TlsConnector::from(client(&a, Some(b.fingerprint.clone())).unwrap())
                .connect(ServerName::try_from("ravel.local").unwrap(), s)
                .await
                .unwrap();
            tls.get_ref()
                .1
                .export_keying_material([0u8; 32], b"EXPORTER-Ravel-pair-v1", None)
                .unwrap()
        };
        let (x, y) = tokio::join!(server, client);
        assert_eq!(x, y);
        let verifier = CertificatePossession {
            provider: Arc::new(rustls::crypto::ring::default_provider()),
            expected: Some(a.fingerprint),
        };
        assert!(verifier.check(&b.certificate, &[]).is_err());
    }
}
