use crate::{
    config::Authentication,
    error::AsStdError,
    types::{PqPrivateKey, PqPublicKey},
    PeerId,
};

use anyhow::anyhow;
use aws_lc_rs::signature::{
    Ed25519KeyPair, KeyPair as _, PqdsaKeyPair, UnparsedPublicKey, ED25519, ML_DSA_44_SIGNING,
};
use bytes::Bytes;
use quinn::crypto::{CryptoError, HmacKey};
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

static SUPPORTED_SIG_ALGS: &[&dyn SignatureVerificationAlgorithm] =
    &[webpki::aws_lc_rs::ED25519, webpki::aws_lc_rs::ML_DSA_44];

static SUPPORTED_ALGORITHMS: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
    all: SUPPORTED_SIG_ALGS,
    mapping: &[
        (
            rustls::SignatureScheme::ED25519,
            &[webpki::aws_lc_rs::ED25519],
        ),
        (
            rustls::SignatureScheme::ML_DSA_44,
            &[webpki::aws_lc_rs::ML_DSA_44],
        ),
    ],
};

/// What anemo took before post-quantum certs, and all that a classical node takes.
static ED25519_ALGORITHMS: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
    all: &[webpki::aws_lc_rs::ED25519],
    mapping: &[(
        rustls::SignatureScheme::ED25519,
        &[webpki::aws_lc_rs::ED25519],
    )],
};

/// id-ml-dsa-44
const ML_DSA_44_OID: &str = "2.16.840.1.101.3.4.3.17";

// TODO: placeholder under the documentation arc (RFC 5612), needs an OID we own before it ships.
pub(crate) const PQ_IDENTITY_OID: &[u64] = &[1, 3, 6, 1, 4, 1, 32473, 1];

const PQ_IDENTITY_CONTEXT: &[u8] = b"anemo-pq-identity:";

/// Dedicated TLS server-name (SNI) used to mark probe connections.
///
/// A probe verifies that an address is reachable and that the server's cryptographic identity
/// matches an expected [`PeerId`], at the TLS layer only. The dialing client selects this
/// server-name before the handshake.
pub(crate) const PROBE_SERVER_NAME: &str = "anemo-probe";

#[derive(Clone, Debug)]
pub(crate) struct CertVerifier {
    pub(crate) server_names: Vec<String>,
    pub(crate) authentication: Authentication,
}

impl CertVerifier {
    /// What we list in the handshake and take from a peer. A peer only shows its post-quantum
    /// cert if the scheme of that cert is listed. rustls does not hold a peer to the list, so a
    /// classical node also has to refuse that cert here.
    fn algorithms(&self) -> &'static WebPkiSupportedAlgorithms {
        match self.authentication {
            Authentication::PostQuantum => &SUPPORTED_ALGORITHMS,
            Authentication::Classical => &ED25519_ALGORITHMS,
        }
    }
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

    // Verifies this is a valid ed25519 or ML-DSA-44 self-signed certificate
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
                self.algorithms().all,
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
        rustls::crypto::verify_tls12_signature(message, cert, dss, self.algorithms())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, self.algorithms())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.algorithms().supported_schemes()
    }
}

impl ServerCertVerifier for CertVerifier {
    // Verifies this is a valid ed25519 or ML-DSA-44 self-signed certificate
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
                self.algorithms().all,
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
        rustls::crypto::verify_tls12_signature(message, cert, dss, self.algorithms())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, self.algorithms())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.algorithms().supported_schemes()
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
        rustls::crypto::verify_tls12_signature(message, cert, dss, self.0.algorithms())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, self.0.algorithms())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.algorithms().supported_schemes()
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
    peer_identity_from_certificate(certificate).map(|(peer_id, _)| peer_id)
}

/// The PeerId a cert stands for, plus the post-quantum key if it is a cert for one.
pub(crate) fn peer_identity_from_certificate(
    certificate: &CertificateDer,
) -> Result<(PeerId, Option<PqPublicKey>), rustls::Error> {
    use x509_parser::{certificate::X509Certificate, prelude::FromDer};

    let cert = X509Certificate::from_der(certificate.as_ref())
        .map_err(|_| rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding))?;
    let spki = cert.1.public_key();
    if let Some(pq_public_key) = PqPublicKey::from_spki(spki) {
        return pq_identity(&cert.1, pq_public_key);
    }

    let public_key_bytes =
        <ed25519::pkcs8::PublicKeyBytes as pkcs8::DecodePublicKey>::from_public_key_der(spki.raw)
            .map_err(|e| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::Other(rustls::OtherError(
                Arc::new(AsStdError::from(anyhow!("invalid ed25519 public key: {e}"))),
            )))
        })?;

    let peer_id = PeerId(public_key_bytes.to_bytes());

    Ok((peer_id, None))
}

/// A post-quantum cert has no Ed25519 key of its own, so its PeerId is the Ed25519 key that
/// signed for the cert's key in the extension. Without this check a cert could claim any PeerId.
fn pq_identity(
    cert: &x509_parser::certificate::X509Certificate,
    pq_public_key: PqPublicKey,
) -> Result<(PeerId, Option<PqPublicKey>), rustls::Error> {
    let invalid = |why: &str| {
        rustls::Error::InvalidCertificate(rustls::CertificateError::Other(rustls::OtherError(
            Arc::new(AsStdError::from(anyhow!("invalid pq identity: {why}"))),
        )))
    };

    let oid = x509_parser::asn1_rs::Oid::from(PQ_IDENTITY_OID).unwrap();
    let extension = cert
        .get_extension_unique(&oid)
        .ok()
        .flatten()
        .ok_or_else(|| invalid("no identity extension"))?;
    let [0x04, 96, endorsement @ ..] = extension.value else {
        return Err(invalid("malformed identity extension"));
    };
    if endorsement.len() != 96 {
        return Err(invalid("malformed identity extension"));
    }
    let (ed25519_public_key, signature) = endorsement.split_at(32);

    UnparsedPublicKey::new(&ED25519, ed25519_public_key)
        .verify(&endorsed_message(cert.public_key().raw), signature)
        .map_err(|_| invalid("bad ed25519 signature over the post-quantum key"))?;

    Ok((
        PeerId(ed25519_public_key.try_into().unwrap()),
        Some(pq_public_key),
    ))
}

/// What the post-quantum cert carries so a peer still learns our PeerId: the Ed25519 public key
/// and its signature over the cert's key, as one DER OCTET STRING.
pub(crate) fn pq_identity_extension(private_key: &[u8; 32], pq_spki: &[u8]) -> Vec<u8> {
    let ed25519 = Ed25519KeyPair::from_seed_unchecked(private_key).unwrap();

    let mut extension = vec![0x04, 96];
    extension.extend_from_slice(ed25519.public_key().as_ref());
    extension.extend_from_slice(ed25519.sign(&endorsed_message(pq_spki)).as_ref());
    extension
}

/// The signature is over the SubjectPublicKeyInfo and not the bare key, so it also names the
/// scheme the key is for.
fn endorsed_message(pq_spki: &[u8]) -> Vec<u8> {
    [PQ_IDENTITY_CONTEXT, pq_spki].concat()
}

impl PqPrivateKey {
    /// The public key that peers record in
    /// [`PeerInfo::pq_public_key`](crate::types::PeerInfo::pq_public_key).
    pub fn public_key(&self) -> PqPublicKey {
        let public_key = Bytes::copy_from_slice(self.keypair().public_key().as_ref());
        match self {
            Self::MlDsa44(_) => PqPublicKey::MlDsa44(public_key),
        }
    }

    pub(crate) fn keypair(&self) -> PqdsaKeyPair {
        match self {
            // The 32 bytes are the FIPS 204 seed, so other tools get the same key from them.
            Self::MlDsa44(seed) => PqdsaKeyPair::from_seed(&ML_DSA_44_SIGNING, seed).unwrap(),
        }
    }

    /// What a peer has to list in the handshake to be shown this key's cert.
    pub(crate) fn scheme(&self) -> rustls::SignatureScheme {
        match self {
            Self::MlDsa44(_) => rustls::SignatureScheme::ML_DSA_44,
        }
    }

    pub(crate) fn rcgen_algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        match self {
            Self::MlDsa44(_) => &rcgen::PKCS_ML_DSA_44,
        }
    }
}

impl PqPublicKey {
    /// The key of a cert, if it is of a post-quantum scheme we take.
    fn from_spki(spki: &x509_parser::x509::SubjectPublicKeyInfo) -> Option<Self> {
        let scheme = match spki.algorithm.algorithm.to_id_string().as_str() {
            ML_DSA_44_OID => Self::MlDsa44,
            _ => return None,
        };
        Some(scheme(Bytes::copy_from_slice(
            spki.subject_public_key.data.as_ref(),
        )))
    }
}

/// quinn-proto only implements [`HmacKey`] for ring's key if both backends are on, so wrap ours.
pub(crate) struct ResetKey(aws_lc_rs::hmac::Key);

impl HmacKey for ResetKey {
    fn sign(&self, data: &[u8], out: &mut [u8]) {
        // quinn passes a buffer of `signature_len()` bytes.
        out.copy_from_slice(aws_lc_rs::hmac::sign(&self.0, data).as_ref());
    }

    fn signature_len(&self) -> usize {
        32
    }

    fn verify(&self, data: &[u8], signature: &[u8]) -> Result<(), CryptoError> {
        aws_lc_rs::hmac::verify(&self.0, data, signature).map_err(|_| CryptoError)
    }
}

/// Perform a HKDF with the provided ed25519 private key in order to generate a consistent reset
/// key used for quic stateless connection resets.
pub(crate) fn construct_reset_key(private_key: &[u8; 32]) -> ResetKey {
    const STATELESS_RESET_SALT: &[u8] = b"anemo-stateless-reset";

    let salt = aws_lc_rs::hkdf::Salt::new(aws_lc_rs::hkdf::HKDF_SHA256, STATELESS_RESET_SALT);
    let prk = salt.extract(private_key);
    let okm = prk.expand(&[], aws_lc_rs::hmac::HMAC_SHA256).unwrap();

    let mut reset_key = [0; 32];
    okm.fill(&mut reset_key).unwrap();

    ResetKey(aws_lc_rs::hmac::Key::new(
        aws_lc_rs::hmac::HMAC_SHA256,
        &reset_key,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Peers keep reset tokens across our restarts, so these can't change between releases.
    #[test]
    fn reset_key_is_stable() {
        let key = construct_reset_key(&[7; 32]);

        for (connection_id, expected) in [
            (
                &b"some connection id"[..],
                "b953ba6912865c2decca681d99c65706b037d7a15aad06816561025b69d11924",
            ),
            (
                &[0, 1, 2, 3, 4, 5, 6, 7][..],
                "a6b003d51cb5cdff0c34f9b9cfde1b74a5f32d5ca4ee86db67cb1fa986fd8592",
            ),
        ] {
            let mut token = vec![0; key.signature_len()];
            key.sign(connection_id, &mut token);
            assert_eq!(hex::encode(&token), expected);

            assert!(key.verify(connection_id, &token).is_ok());
            token[0] ^= 1;
            assert!(key.verify(connection_id, &token).is_err());
        }
    }

    /// Peers record this key, so what a private key gives can't change between releases.
    #[test]
    fn pq_public_key_is_stable() {
        let PqPublicKey::MlDsa44(pq_public_key) = PqPrivateKey::MlDsa44([7; 32]).public_key();
        assert_eq!(pq_public_key.len(), 1312);

        let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &pq_public_key);
        assert_eq!(
            hex::encode(digest),
            "86c769d3b468de487eb9a09f242f7fa0b997a07ef4ded49627e7bf9539f28b62"
        );
    }
}
