use crate::{error::AsStdError, PeerId};

use anyhow::anyhow;
use rustls::client::danger::ServerCertVerified;
use rustls::client::danger::ServerCertVerifier;
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::ServerName;
use rustls::pki_types::SignatureVerificationAlgorithm;
use rustls::pki_types::TrustAnchor;
use rustls::pki_types::UnixTime;
use rustls::server::danger::ClientCertVerified;
use rustls::server::danger::ClientCertVerifier;
use std::sync::Arc;

static SUPPORTED_SIG_ALGS: &[&dyn SignatureVerificationAlgorithm] = &[webpki::ring::ED25519];

static SUPPORTED_ALGORITHMS: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
    all: SUPPORTED_SIG_ALGS,
    mapping: &[(rustls::SignatureScheme::ED25519, SUPPORTED_SIG_ALGS)],
};

#[derive(Clone, Debug)]
pub(crate) struct CertVerifier {
    pub(crate) server_names: Vec<String>,
}

/// A `ClientCertVerifier` that will ensure that every client provides a valid, expected
/// certificate, without any name checking.
impl ClientCertVerifier for CertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        // Since we're relying on self-signed certificates and not on CAs, continue the handshake
        // without passing a list of CA DNs
        &[]
    }

    // // Verifies this is a valid ed25519 self-signed certificate
    // // 1. we prepare arguments for webpki's certificate verification (following the rustls implementation)
    // //    placing the public key at the root of the certificate chain (as it should be for a self-signed certificate)
    // // 2. we call webpki's certificate verification
    // fn verify_client_cert(
    //     &self,
    //     end_entity: &CertificateDer,
    //     intermediates: &[CertificateDer],
    //     now: UnixTime,
    // ) -> Result<ClientCertVerified, rustls::Error> {
    //     // Step 1: Check this matches the key we expect
    //     let public_key = public_key_from_certificate(end_entity)?;
    //     if !self.allower.allowed(&public_key) {
    //         return Err(rustls::Error::General(format!(
    //             "invalid certificate: {:?} is not in the validator set",
    //             public_key,
    //         )));
    //     }

    //     // Step 2: verify the certificate signature and server name with webpki.
    //     verify_self_signed_cert(
    //         end_entity,
    //         intermediates,
    //         webpki::KeyUsage::client_auth(),
    //         &self.name,
    //         now,
    //     )
    //     .map(|_| ClientCertVerified::assertion())
    // }

    // Verifies this is a valid ed25519 self-signed certificate
    // 1. we prepare arguments for webpki's certificate verification (following the rustls implementation)
    //    placing the public key at the root of the certificate chain (as it should be for a self-signed certificate)
    // 2. we call webpki's certificate verification
    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer,
        intermediates: &[CertificateDer],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        // We now check we're receiving correctly signed data with the expected key
        // Step 1: prepare arguments
        let (cert, chain, trustroots) = prepare_for_self_signed(end_entity, intermediates)?;

        // Step 2: call verification from webpki

        let verified_cert = cert
            .verify_for_usage(
                SUPPORTED_SIG_ALGS,
                &trustroots,
                chain,
                now,
                webpki::KeyUsage::client_auth(),
                None,
                None,
            )
            .map_err(pki_error)?;

        // Ensure the cert is valid for the network name
        let subject_name_refs = self
            .server_names
            .iter()
            .map(|name| ServerName::try_from(name.as_str()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| rustls::Error::UnsupportedNameType)?;

        if subject_name_refs.into_iter().any(|name| {
            verified_cert
                .end_entity()
                .verify_is_valid_for_subject_name(&name)
                .is_ok()
        }) {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(rustls::Error::General("no valid subject name".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &SUPPORTED_ALGORITHMS)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &SUPPORTED_ALGORITHMS)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        SUPPORTED_ALGORITHMS.supported_schemes()
    }
}

impl ServerCertVerifier for CertVerifier {
    // Verifies this is a valid ed25519 self-signed certificate
    // 1. we prepare arguments for webpki's certificate verification (following the rustls implementation)
    //    placing the public key at the root of the certificate chain (as it should be for a self-signed certificate)
    // 2. we call webpki's certificate verification
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        // Then we check this is actually a valid self-signed certificate with matching name
        // Step 1: prepare arguments
        let (cert, chain, trustroots) = prepare_for_self_signed(end_entity, intermediates)?;

        let dns_name = match server_name {
            ServerName::DnsName(dns_name) => dns_name,
            _ => return Err(rustls::Error::UnsupportedNameType),
        };
        // Client server_name needs to match one of our server_names
        self.server_names
            .iter()
            .find(|name| name.as_str() == dns_name.as_ref())
            .ok_or(rustls::Error::UnsupportedNameType)?;

        // Step 2: call verification from webpki
        let verified_cert = cert
            .verify_for_usage(
                SUPPORTED_SIG_ALGS,
                &trustroots,
                chain,
                now,
                webpki::KeyUsage::server_auth(),
                None,
                None,
            )
            .map_err(pki_error)?;

        verified_cert
            .end_entity()
            .verify_is_valid_for_subject_name(server_name)
            .map_err(pki_error)
            .map(|_| ServerCertVerified::assertion())
    }

    // fn verify_server_cert(
    //     &self,
    //     end_entity: &CertificateDer<'_>,
    //     intermediates: &[CertificateDer<'_>],
    //     _server_name: &ServerName,
    //     _ocsp_response: &[u8],
    //     now: UnixTime,
    // ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
    //     let public_key = public_key_from_certificate(end_entity)?;
    //     if public_key != self.public_key {
    //         return Err(rustls::Error::General(format!(
    //             "invalid certificate: {:?} is not the expected server public key",
    //             public_key,
    //         )));
    //     }

    //     verify_self_signed_cert(
    //         end_entity,
    //         intermediates,
    //         webpki::KeyUsage::server_auth(),
    //         &self.name,
    //         now,
    //     )
    //     .map(|_| rustls::client::danger::ServerCertVerified::assertion())
    // }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &SUPPORTED_ALGORITHMS)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &SUPPORTED_ALGORITHMS)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        SUPPORTED_ALGORITHMS.supported_schemes()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ExpectedCertVerifier(pub(crate) CertVerifier, pub(crate) PeerId);

impl ServerCertVerifier for ExpectedCertVerifier {
    // Verifies this is a valid certificate self-signed by the public key we expect(in PSK)
    // 1. we check the equality of the certificate's public key with the key we expect
    // 2. we prepare arguments for webpki's certificate verification (following the rustls implementation)
    //    placing the public key at the root of the certificate chain (as it should be for a self-signed certificate)
    // 3. we call webpki's certificate verification
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        // Step 1: Check this matches the key we expect
        let peer_id = peer_id_from_certificate(end_entity)?;

        if peer_id != self.1 {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::Other(rustls::OtherError(Arc::new(AsStdError::from(
                    anyhow!(
                        "invalid peer certificate: received {:?} instead of expected {:?}",
                        peer_id,
                        self.1,
                    ),
                )))),
            ));
        }

        // Delegate steps 2 and 3 to CertVerifier's impl
        self.0
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &SUPPORTED_ALGORITHMS)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &SUPPORTED_ALGORITHMS)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        SUPPORTED_ALGORITHMS.supported_schemes()
    }
}

type CertChainAndRoots<'a> = (
    webpki::EndEntityCert<'a>,
    &'a [CertificateDer<'a>],
    Vec<TrustAnchor<'a>>,
);

// This prepares arguments for webpki, including a trust anchor which is the end entity of the certificate
// (which embodies a self-signed certificate by definition)
fn prepare_for_self_signed<'a>(
    end_entity: &'a CertificateDer,
    intermediates: &'a [CertificateDer],
) -> Result<CertChainAndRoots<'a>, rustls::Error> {
    // EE cert must appear first.
    let cert = webpki::EndEntityCert::try_from(end_entity).map_err(pki_error)?;

    // reinterpret the certificate as a root, materializing the self-signed policy
    let root = webpki::anchor_from_trusted_cert(end_entity).map_err(pki_error)?;

    Ok((cert, intermediates, vec![root]))
}

fn pki_error(error: webpki::Error) -> rustls::Error {
    use webpki::Error::*;
    match error {
        BadDer | BadDerTime => {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
        }
        #[allow(deprecated)]
        InvalidSignatureForPublicKey
        | UnsupportedSignatureAlgorithm
        | UnsupportedSignatureAlgorithmForPublicKey
        | UnsupportedSignatureAlgorithmContext(..)
        | UnsupportedSignatureAlgorithmForPublicKeyContext(..) => {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadSignature)
        }
        e => {
            rustls::Error::InvalidCertificate(rustls::CertificateError::Other(rustls::OtherError(
                Arc::new(AsStdError::from(anyhow!("invalid peer certificate: {e}"))),
            )))
        }
    }
}

pub(crate) fn peer_id_from_certificate(
    certificate: &CertificateDer,
) -> Result<PeerId, rustls::Error> {
    use x509_parser::{certificate::X509Certificate, prelude::FromDer};

    let cert = X509Certificate::from_der(certificate.as_ref())
        .map_err(|_| rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding))?;
    let spki = cert.1.public_key();
    let public_key_bytes =
        <ed25519::pkcs8::PublicKeyBytes as pkcs8::DecodePublicKey>::from_public_key_der(spki.raw)
            .map_err(|e| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::Other(rustls::OtherError(
                Arc::new(AsStdError::from(anyhow!("invalid ed25519 public key: {e}"))),
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
