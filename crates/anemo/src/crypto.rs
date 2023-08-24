use anyhow::anyhow;
use std::sync::Arc;

use crate::{error::AsStdError, PeerId};

static SUPPORTED_SIG_ALGS: &[&webpki::SignatureAlgorithm] = &[&webpki::ED25519];

#[derive(Clone, Debug)]
pub(crate) struct CertVerifier {
    pub(crate) server_names: Vec<String>,
}

/// A `ClientCertVerifier` that will ensure that every client provides a valid, expected
/// certificate, without any name checking.
impl rustls::server::ClientCertVerifier for CertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn client_auth_root_subjects(&self) -> &[rustls::DistinguishedName] {
        // Since we're relying on self-signed certificates and not on CAs, continue the handshake
        // without passing a list of CA DNs
        &[]
    }

    // Verifies this is a valid ed25519 self-signed certificate
    // 1. we prepare arguments for webpki's certificate verification (following the rustls implementation)
    //    placing the public key at the root of the certificate chain (as it should be for a self-signed certificate)
    // 2. we call webpki's certificate verification
    fn verify_client_cert(
        &self,
        end_entity: &rustls::Certificate,
        intermediates: &[rustls::Certificate],
        now: std::time::SystemTime,
    ) -> Result<rustls::server::ClientCertVerified, rustls::Error> {
        // We now check we're receiving correctly signed data with the expected key
        // Step 1: prepare arguments
        let (cert, chain, trustroots) = prepare_for_self_signed(end_entity, intermediates)?;
        let now = webpki::Time::try_from(now).map_err(|_| rustls::Error::FailedToGetCurrentTime)?;

        // Step 2: call verification from webpki
        let cert = cert
            .verify_for_usage(
                SUPPORTED_SIG_ALGS,
                &trustroots,
                &chain,
                now,
                webpki::KeyUsage::client_auth(),
                &[],
            )
            .map_err(pki_error)
            .map(|_| cert)?;

        // Ensure the cert is valid for the network name
        let subject_name_refs = self
            .server_names
            .iter()
            .map(|name| webpki::SubjectNameRef::try_from_ascii_str(name.as_str()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| rustls::Error::UnsupportedNameType)?;

        if subject_name_refs
            .into_iter()
            .any(|name| cert.verify_is_valid_for_subject_name(name).is_ok())
        {
            Ok(rustls::server::ClientCertVerified::assertion())
        } else {
            Err(rustls::Error::General("no valid subject name".into()))
        }
    }
}

impl rustls::client::ServerCertVerifier for CertVerifier {
    // Verifies this is a valid ed25519 self-signed certificate
    // 1. we prepare arguments for webpki's certificate verification (following the rustls implementation)
    //    placing the public key at the root of the certificate chain (as it should be for a self-signed certificate)
    // 2. we call webpki's certificate verification
    fn verify_server_cert(
        &self,
        end_entity: &rustls::Certificate,
        intermediates: &[rustls::Certificate],
        server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        // Then we check this is actually a valid self-signed certificate with matching name
        // Step 1: prepare arguments
        let (cert, chain, trustroots) = prepare_for_self_signed(end_entity, intermediates)?;
        let now = webpki::Time::try_from(now).map_err(|_| rustls::Error::FailedToGetCurrentTime)?;

        let dns_nameref = match server_name {
            rustls::ServerName::DnsName(dns_name) => {
                webpki::SubjectNameRef::try_from_ascii_str(dns_name.as_ref())
                    .map_err(|_| rustls::Error::UnsupportedNameType)?
            }
            _ => return Err(rustls::Error::UnsupportedNameType),
        };
        // Client server_name needs to match one of our server_names
        self.server_names
            .iter()
            .find(
                |name| match webpki::SubjectNameRef::try_from_ascii_str(name.as_ref()) {
                    Ok(dns_name_ref) => dns_name_ref.as_ref() == dns_nameref.as_ref(),
                    Err(_) => {
                        tracing::error!("invalid dns name: {:?}", name);
                        false
                    }
                },
            )
            .ok_or(rustls::Error::UnsupportedNameType)?;

        // Step 2: call verification from webpki
        let cert = cert
            .verify_for_usage(
                SUPPORTED_SIG_ALGS,
                &trustroots,
                &chain,
                now,
                webpki::KeyUsage::server_auth(),
                &[],
            )
            .map_err(pki_error)
            .map(|_| cert)?;

        cert.verify_is_valid_for_subject_name(dns_nameref)
            .map_err(pki_error)
            .map(|_| rustls::client::ServerCertVerified::assertion())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ExpectedCertVerifier(pub(crate) CertVerifier, pub(crate) PeerId);

impl rustls::client::ServerCertVerifier for ExpectedCertVerifier {
    // Verifies this is a valid certificate self-signed by the public key we expect(in PSK)
    // 1. we check the equality of the certificate's public key with the key we expect
    // 2. we prepare arguments for webpki's certificate verification (following the rustls implementation)
    //    placing the public key at the root of the certificate chain (as it should be for a self-signed certificate)
    // 3. we call webpki's certificate verification
    fn verify_server_cert(
        &self,
        end_entity: &rustls::Certificate,
        intermediates: &[rustls::Certificate],
        server_name: &rustls::ServerName,
        scts: &mut dyn Iterator<Item = &[u8]>,
        ocsp_response: &[u8],
        now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        // Step 1: Check this matches the key we expect
        let peer_id = peer_id_from_certificate(end_entity)?;

        if peer_id != self.1 {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::Other(Arc::new(AsStdError::from(anyhow!(
                    "invalid peer certificate: received {:?} instead of expected {:?}",
                    peer_id,
                    self.1,
                )))),
            ));
        }

        // Delegate steps 2 and 3 to CertVerifier's impl
        self.0.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            scts,
            ocsp_response,
            now,
        )
    }
}

type CertChainAndRoots<'a> = (
    webpki::EndEntityCert<'a>,
    Vec<&'a [u8]>,
    Vec<webpki::TrustAnchor<'a>>,
);

// This prepares arguments for webpki, including a trust anchor which is the end entity of the certificate
// (which embodies a self-signed certificate by definition)
fn prepare_for_self_signed<'a>(
    end_entity: &'a rustls::Certificate,
    intermediates: &'a [rustls::Certificate],
) -> Result<CertChainAndRoots<'a>, rustls::Error> {
    // EE cert must appear first.
    let cert = webpki::EndEntityCert::try_from(end_entity.0.as_ref()).map_err(pki_error)?;

    let intermediates: Vec<&'a [u8]> = intermediates.iter().map(|cert| cert.0.as_ref()).collect();

    // reinterpret the certificate as a root, materializing the self-signed policy
    let root = webpki::TrustAnchor::try_from_cert_der(end_entity.0.as_ref()).map_err(pki_error)?;

    Ok((cert, intermediates, vec![root]))
}

fn pki_error(error: webpki::Error) -> rustls::Error {
    use webpki::Error::*;
    match error {
        BadDer | BadDerTime => {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
        }
        InvalidSignatureForPublicKey
        | UnsupportedSignatureAlgorithm
        | UnsupportedSignatureAlgorithmForPublicKey => {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadSignature)
        }
        e => rustls::Error::InvalidCertificate(rustls::CertificateError::Other(Arc::new(
            AsStdError::from(anyhow!("invalid peer certificate: {e}")),
        ))),
    }
}

pub(crate) fn peer_id_from_certificate(
    certificate: &rustls::Certificate,
) -> Result<PeerId, rustls::Error> {
    use x509_parser::{certificate::X509Certificate, prelude::FromDer};

    let cert = X509Certificate::from_der(certificate.0.as_ref())
        .map_err(|_| rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding))?;
    let spki = cert.1.public_key();
    let public_key_bytes =
        <ed25519::pkcs8::PublicKeyBytes as pkcs8::DecodePublicKey>::from_public_key_der(spki.raw)
            .map_err(|e| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::Other(Arc::new(
                AsStdError::from(anyhow!("invalid ed25519 public key: {e}")),
            )))
        })?;

    let peer_id = PeerId(public_key_bytes.to_bytes());

    Ok(peer_id)
}

/// Perform a HKDF with the provided ed25519 private key in order to generate a consistent reset
/// key used for quic stateless connection resets.
pub(crate) fn construct_reset_key(private_key: &[u8; 32]) -> ring::hmac::Key {
    const STATELESS_RESET_SALT: &[u8] = b"anemo-stateless-reset";

    let salt = ring::hkdf::Salt::new(ring::hkdf::HKDF_SHA256, STATELESS_RESET_SALT);
    let prk = salt.extract(private_key);
    let okm = prk.expand(&[], ring::hmac::HMAC_SHA256).unwrap();

    let mut reset_key = [0; 32];
    okm.fill(&mut reset_key).unwrap();

    ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &reset_key)
}
