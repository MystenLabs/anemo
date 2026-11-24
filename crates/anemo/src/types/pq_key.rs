use bytes::Bytes;

/// A peer's post-quantum public key. The variant names the signature scheme, so another scheme
/// is a new variant and not a new meaning for the same bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PqPublicKey {
    /// ML-DSA-44 (FIPS 204) public key, 1312 bytes.
    MlDsa44(Bytes),
}

/// A node's post-quantum private key, see [`PqPublicKey`].
#[derive(Clone)]
#[non_exhaustive]
pub enum PqPrivateKey {
    /// ML-DSA-44 (FIPS 204) seed.
    MlDsa44([u8; 32]),
}

// Keeps the seed out of logs.
impl std::fmt::Debug for PqPrivateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MlDsa44(_) => f.write_str("MlDsa44(..)"),
        }
    }
}
