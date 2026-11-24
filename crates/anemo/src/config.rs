use crate::{
    crypto::{CertVerifier, ExpectedCertVerifier, PROBE_SERVER_NAME},
    PeerId, PqPrivateKey, Result,
};
use aws_lc_rs::signature::{KeyPair as _, PqdsaKeyPair};
use pkcs8::EncodePrivateKey;
use quinn::VarInt;
use rcgen::{CertificateParams, KeyPair};
use rustls::client::danger::ServerCertVerifier;
use rustls::client::ResolvesClientCert;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::PrivateKeyDer;
use rustls::server::{ClientHello, ResolvesServerCert, ResolvesServerCertUsingSni};
use rustls::sign::CertifiedKey;
use rustls::SignatureScheme;
use serde::{Deserialize, Serialize};
use std::{num::NonZeroU32, sync::Arc, time::Duration};

/// Configuration for a [`Network`](crate::Network).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct Config {
    /// Configuration for the underlying QUIC transport.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quic: Option<QuicConfig>,

    /// Key-exchange groups to offer, see [`KeyExchange`].
    ///
    /// If unspecified, this will default to [`KeyExchange::Hybrid`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_exchange: Option<KeyExchange>,

    /// Certificates to authenticate with, see [`Authentication`].
    ///
    /// If unspecified, this will default to [`Authentication::PostQuantum`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<Authentication>,

    /// Size of the internal `ConnectionManager`s mailbox.
    ///
    /// One example of how this mailbox is used is for submitting
    /// connection requests via [`Network::connect`](crate::Network::connect).
    ///
    /// If unspecified, this will default to `128`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_manager_channel_capacity: Option<usize>,

    /// Trigger connectivity checks every interval.
    ///
    /// If unspecified, this will default to `5,000` milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connectivity_check_interval_ms: Option<u64>,

    /// Maximum delay between 2 consecutive attempts to connect with a peer.
    ///
    /// If unspecified, this will default to `60,000` milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_connection_backoff_ms: Option<u64>,

    /// The backoff step size, in milliseconds, used to calculate the delay between two consecutive
    /// attempts to connect with a peer.
    ///
    /// If unspecified, this will default to `10,000` milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_backoff_ms: Option<u64>,

    /// Set a timeout, in milliseconds, for all inbound and outbound connects.
    ///
    /// In unspecified, this will default to `10,000` milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_timeout_ms: Option<u64>,

    /// Maximum number of concurrent connections to attempt to establish at a given point in time.
    ///
    /// If unspecified, this will default to `100`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_outstanding_connecting_connections: Option<usize>,

    /// Maximum number of concurrent connections to have established at a given point in time.
    ///
    /// This limit is applied in the following ways:
    ///  - Inbound connections from [`KnownPeers`] with [`PeerAffinity::High`] or
    ///    [`PeerAffinity::Allowed`] bypass this limit. All other inbound
    ///    connections are only accepted if the total number of inbound and outbound
    ///    connections, irrespective of affinity, is less than this limit.
    ///  - Outbound connections explicitly made by the application via [`Network::connect`] or
    ///    [`Network::connect_with_peer_id`] bypass this limit.
    ///  - Outbound connections made in the background, due to configured [`KnownPeers`], to peers with
    ///    [`PeerAffinity::High`] bypass this limit and are always attempted.
    ///
    /// If unspecified, there will be no limit on the number of concurrent connections.
    ///
    /// [`KnownPeers`]: crate::KnownPeers
    /// [`PeerAffinity::Allowed`]: crate::types::PeerAffinity::Allowed
    /// [`PeerAffinity::High`]: crate::types::PeerAffinity::High
    /// [`Network::connect`]: crate::Network::connect
    /// [`Network::connect_with_peer_id`]: crate::Network::connect_with_peer_id
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_connections: Option<usize>,

    /// Whether to require QUIC source-address validation (via a stateless Retry
    /// packet) before beginning the TLS handshake for an inbound connection whose
    /// source address has not yet been validated.
    ///
    /// When enabled, the first attempt from an unvalidated address is answered with
    /// a Retry instead of starting the handshake, forcing the peer to prove it can
    /// receive traffic at its claimed address.
    ///
    /// If unspecified, this defaults to `true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_inbound_address_validation: Option<bool>,

    /// Maximum sustained rate, per source IP, of new inbound connections admitted to
    /// the TLS handshake, as a token-bucket refill rate in connections per second.
    /// Inbound attempts from a source IP exceeding this rate are dropped before the
    /// handshake begins.
    ///
    /// If unspecified, this defaults to `10`. Setting it to `0` disables per-IP
    /// inbound connection rate limiting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inbound_connection_rate_limit_per_ip: Option<u32>,

    /// Token-bucket burst size for `inbound_connection_rate_limit_per_ip`, i.e. the
    /// maximum number of inbound connections from a single source IP that may be
    /// admitted in an instantaneous burst.
    ///
    /// If unspecified, this defaults to `100`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inbound_connection_rate_limit_burst_per_ip: Option<u32>,

    /// Size of the broadcast channel use for subscribing to
    /// [`PeerEvent`](crate::types::PeerEvent)s via
    /// [`Network::subscribe`](crate::Network::subscribe).
    ///
    /// If unspecified, this will default to `128`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_event_broadcast_channel_capacity: Option<usize>,

    /// Set the maximum frame size in bytes.
    ///
    /// This controls the maximum size of a request or response. Acts as the default
    /// for both directions when [`Config::max_request_frame_size`] or
    /// [`Config::max_response_frame_size`] are not set.
    ///
    /// If unspecified, there will be no limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_frame_size: Option<usize>,

    /// Set the maximum frame size in bytes for request messages.
    ///
    /// If unspecified, falls back to [`Config::max_frame_size`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_request_frame_size: Option<usize>,

    /// Set the maximum frame size in bytes for response messages.
    ///
    /// If unspecified, falls back to [`Config::max_frame_size`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_response_frame_size: Option<usize>,

    /// Set a timeout, in milliseconds, for all inbound requests.
    ///
    /// When an inbound timeout is hit when processing a request a Response is sent to the
    /// requestor with a [`StatusCode::RequestTimeout`] status code.
    ///
    /// In unspecified, no default timeout will be configured.
    ///
    /// [`StatusCode::RequestTimeout`]: crate::types::response::StatusCode
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inbound_request_timeout_ms: Option<u64>,

    /// Set a timeout, in milliseconds, for all outbound requests.
    ///
    /// When an outbound timeout is hit a timeout error will be returned.
    ///
    /// In unspecified, no default timeout will be configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outbound_request_timeout_ms: Option<u64>,

    /// Set a timeout, in milliseconds, until the peers are notified when network
    /// is shutting down
    ///
    /// If unspecified, then this will default to 1 minute (60 * 1_000 ms)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutdown_idle_timeout_ms: Option<u64>,
}

/// Configuration for the underlying QUIC transport.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct QuicConfig {
    /// Maximum number of incoming bidirectional streams that may be open concurrently.
    ///
    /// Must be nonzero for the peer to open any bidirectional streams.
    ///
    /// If unspecified, this will default to `100`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_bidi_streams: Option<u64>,

    /// Maximum number of incoming unidirectional streams that may be open concurrently.
    ///
    /// Must be nonzero for the peer to open any unidirectional streams.
    ///
    /// If unspecified, this will default to `100`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_uni_streams: Option<u64>,

    /// Maximum number of bytes a peer may transmit without acknowledgement on any one stream
    /// before becoming blocked.
    ///
    /// If unspecified, this will default to 1.25MB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_receive_window: Option<u64>,

    /// Maximum number of bytes a peer may transmit across all streams of a connection before
    /// becoming blocked.
    ///
    /// If unspecified, this will default to unlimited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receive_window: Option<u64>,

    /// Maximum number of bytes to transmit to a peer without acknowledgment
    ///
    /// If unspecified, this will default to 10MB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_window: Option<u64>,

    /// Maximum quantity of out-of-order crypto layer data to buffer
    ///
    /// If unspecified, this will default to 16KiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crypto_buffer_size: Option<usize>,

    /// How long to wait to hear from a peer before timing out a connection.
    ///
    /// In the absence of any keep-alive messages, connections will be closed if they remain idle
    /// for at least this duration.
    ///
    /// If unspecified, this will default to `10,000` milliseconds.
    ///
    /// Maximum possible value is 2^62 milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_idle_timeout_ms: Option<u64>,

    /// Interval at which to send keep-alives to maintain otherwise idle connections.
    ///
    /// Keep-alives prevent otherwise idle connections from timing out.
    ///
    /// If unspecified, this will default to `None`, disabling keep-alives.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_alive_interval_ms: Option<u64>,

    /// Size of the send buffer on the UDP socket (`SO_SNDBUF`).
    ///
    /// If unspecified, this will use the operating system default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socket_send_buffer_size: Option<usize>,

    /// Size of the receive buffer on the UDP socket (`SO_RCVBUF`).
    ///
    /// If unspecified, this will use the operating system default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socket_receive_buffer_size: Option<usize>,

    /// If true, failure to set UDP socket buffer sizes as requested above will not
    /// prevent a Network from starting.
    #[serde(default)]
    pub allow_failed_socket_buffer_size_setting: bool,
}

impl Config {
    pub(crate) fn transport_config(&self) -> quinn::TransportConfig {
        self.quic
            .as_ref()
            .map(QuicConfig::transport_config)
            .unwrap_or_default()
    }

    pub(crate) fn connection_manager_channel_capacity(&self) -> usize {
        const CONNECTION_MANAGER_CHANNEL_CAPACITY: usize = 128;

        self.connection_manager_channel_capacity
            .unwrap_or(CONNECTION_MANAGER_CHANNEL_CAPACITY)
    }

    pub(crate) fn connectivity_check_interval(&self) -> Duration {
        const CONNECTIVITY_CHECK_INTERVAL_MS: u64 = 5_000; // 5 seconds

        Duration::from_millis(
            self.connectivity_check_interval_ms
                .unwrap_or(CONNECTIVITY_CHECK_INTERVAL_MS),
        )
    }

    pub(crate) fn max_connection_backoff(&self) -> Duration {
        const MAX_CONNECTION_BACKOFF_MS: u64 = 60_000; // 1 minute

        Duration::from_millis(
            self.max_connection_backoff_ms
                .unwrap_or(MAX_CONNECTION_BACKOFF_MS),
        )
    }

    pub(crate) fn connection_backoff(&self) -> Duration {
        const CONNECTION_BACKOFF_MS: u64 = 10_000; // 10 seconds

        Duration::from_millis(self.connection_backoff_ms.unwrap_or(CONNECTION_BACKOFF_MS))
    }

    pub(crate) fn connect_timeout(&self) -> Duration {
        const CONNECTION_TIMEOUT_MS: u64 = 10_000; // 10 seconds

        Duration::from_millis(self.connect_timeout_ms.unwrap_or(CONNECTION_TIMEOUT_MS))
    }

    pub(crate) fn max_concurrent_outstanding_connecting_connections(&self) -> usize {
        const MAX_CONCURRENT_OUTSTANDING_CONNECTING_CONNECTIONS: usize = 100;

        self.max_concurrent_outstanding_connecting_connections
            .unwrap_or(MAX_CONCURRENT_OUTSTANDING_CONNECTING_CONNECTIONS)
    }

    pub(crate) fn max_concurrent_connections(&self) -> Option<usize> {
        self.max_concurrent_connections
    }

    pub(crate) fn require_inbound_address_validation(&self) -> bool {
        const DEFAULT: bool = true;

        self.require_inbound_address_validation.unwrap_or(DEFAULT)
    }

    /// Sustained per-source-IP inbound connection admission rate in connections per
    /// second. Returns `None` only when explicitly disabled by configuring `0`.
    pub(crate) fn inbound_connection_rate_limit_per_ip(&self) -> Option<NonZeroU32> {
        const DEFAULT_RATE_PER_SEC: u32 = 10;

        // `NonZeroU32::new` yields `None` for a configured `0`, i.e. rate limiting off.
        NonZeroU32::new(
            self.inbound_connection_rate_limit_per_ip
                .unwrap_or(DEFAULT_RATE_PER_SEC),
        )
    }

    pub(crate) fn inbound_connection_rate_limit_burst_per_ip(&self) -> NonZeroU32 {
        const DEFAULT_BURST: u32 = 100;

        // A configured burst of `0` is nonsensical (it would admit nothing); fall back
        // to the default in that case as well as when unset.
        self.inbound_connection_rate_limit_burst_per_ip
            .and_then(NonZeroU32::new)
            .unwrap_or_else(|| NonZeroU32::new(DEFAULT_BURST).expect("DEFAULT_BURST is nonzero"))
    }

    pub(crate) fn peer_event_broadcast_channel_capacity(&self) -> usize {
        const PEER_EVENT_BROADCAST_CHANNEL_CAPACITY: usize = 128;

        self.peer_event_broadcast_channel_capacity
            .unwrap_or(PEER_EVENT_BROADCAST_CHANNEL_CAPACITY)
    }

    pub(crate) fn request_frame_size(&self) -> Option<usize> {
        self.max_request_frame_size.or(self.max_frame_size)
    }

    pub(crate) fn response_frame_size(&self) -> Option<usize> {
        self.max_response_frame_size.or(self.max_frame_size)
    }

    pub(crate) fn inbound_request_timeout(&self) -> Option<Duration> {
        self.inbound_request_timeout_ms.map(Duration::from_millis)
    }

    pub(crate) fn outbound_request_timeout(&self) -> Option<Duration> {
        self.outbound_request_timeout_ms.map(Duration::from_millis)
    }

    pub(crate) fn shutdown_idle_timeout(&self) -> Duration {
        const DEFAULT_SHUTDOWN_IDLE_TIMEOUT_MS: u64 = 60_000; // 1 minute

        self.shutdown_idle_timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(DEFAULT_SHUTDOWN_IDLE_TIMEOUT_MS))
    }
}

impl QuicConfig {
    pub(crate) fn transport_config(&self) -> quinn::TransportConfig {
        let mut config = quinn::TransportConfig::default();

        if let Some(max) = self
            .max_concurrent_bidi_streams
            .map(|n| VarInt::try_from(n).unwrap_or(VarInt::MAX))
        {
            config.max_concurrent_bidi_streams(max);
        }

        if let Some(max) = self
            .max_concurrent_uni_streams
            .map(|n| VarInt::try_from(n).unwrap_or(VarInt::MAX))
        {
            config.max_concurrent_uni_streams(max);
        }

        if let Some(max) = self
            .stream_receive_window
            .map(|n| VarInt::try_from(n).unwrap_or(VarInt::MAX))
        {
            config.stream_receive_window(max);
        }

        if let Some(max) = self
            .receive_window
            .map(|n| VarInt::try_from(n).unwrap_or(VarInt::MAX))
        {
            config.receive_window(max);
        }

        if let Some(n) = self.send_window {
            config.send_window(n);
        }

        if let Some(n) = self.crypto_buffer_size {
            config.crypto_buffer_size(n);
        }

        if let Some(max) = self
            .max_idle_timeout_ms
            .map(|n| VarInt::try_from(n).unwrap_or(VarInt::MAX))
            .map(Into::into)
        {
            config.max_idle_timeout(Some(max));
        }

        if let Some(keep_alive_interval) = self.keep_alive_interval_ms.map(Duration::from_millis) {
            config.keep_alive_interval(Some(keep_alive_interval));
        }

        config
    }
}

/// Key-exchange groups offered in the TLS handshake.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum KeyExchange {
    /// X25519MLKEM768 ahead of the classical groups. Peers without it fall back to X25519.
    #[default]
    Hybrid,
    /// What anemo offered before: X25519, secp256r1, secp384r1. This is to roll back.
    Classical,
}

/// Certificates a node authenticates with in the TLS handshake.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Authentication {
    /// Take post-quantum certificates from peers, and show ours to peers that take it if this
    /// node has a `pq_private_key`. Ed25519 with the rest.
    #[default]
    PostQuantum,
    /// What anemo did before: Ed25519 only, even with a `pq_private_key`. It asks peers for the
    /// same and takes nothing else from them. This is to roll back. A recorded post-quantum key
    /// still has to be shown, so those peers can't connect in this mode.
    Classical,
}

/// The aws-lc-rs default groups change with rustls' `prefer-post-quantum` feature, so set ours.
fn crypto_provider(key_exchange: KeyExchange) -> Arc<rustls::crypto::CryptoProvider> {
    use rustls::crypto::aws_lc_rs::{default_provider, kx_group};

    let mut groups = vec![kx_group::X25519, kx_group::SECP256R1, kx_group::SECP384R1];
    match key_exchange {
        KeyExchange::Hybrid => groups.insert(0, kx_group::X25519MLKEM768),
        KeyExchange::Classical => {}
    }

    let mut provider = default_provider();
    provider.kx_groups = groups;
    Arc::new(provider)
}

/// Our certs for one server name. Both stand for the same PeerId. The post-quantum one needs a
/// `pq_private_key` and is left out with [`Authentication::Classical`].
#[derive(Clone, Debug)]
pub(crate) struct CertifiedKeys {
    ed25519: Arc<CertifiedKey>,
    /// With the scheme a peer has to list to be shown this cert. Peers from before it don't,
    /// and would fail on it, so they keep getting the Ed25519 one.
    pq: Option<(SignatureScheme, Arc<CertifiedKey>)>,
}

impl ResolvesClientCert for CertifiedKeys {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        sigschemes: &[SignatureScheme],
    ) -> Option<Arc<CertifiedKey>> {
        Some(match &self.pq {
            Some((scheme, pq)) if sigschemes.contains(scheme) => pq.clone(),
            _ => self.ed25519.clone(),
        })
    }

    fn has_certs(&self) -> bool {
        true
    }
}

#[derive(Debug)]
struct ServerCertResolver {
    ed25519: ResolvesServerCertUsingSni,
    pq: Option<(SignatureScheme, ResolvesServerCertUsingSni)>,
}

impl ResolvesServerCert for ServerCertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        match &self.pq {
            Some((scheme, pq)) if client_hello.signature_schemes().contains(scheme) => {
                pq.resolve(client_hello)
            }
            _ => self.ed25519.resolve(client_hello),
        }
    }
}

/// rcgen 0.14 can't load an ML-DSA key (`from_der_and_sign_algo` panics on it), so it signs the
/// cert through ours.
struct PqKeyPair(PqdsaKeyPair, &'static rcgen::SignatureAlgorithm);

impl rcgen::PublicKeyData for PqKeyPair {
    fn der_bytes(&self) -> &[u8] {
        self.0.public_key().as_ref()
    }

    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        self.1
    }
}

impl rcgen::SigningKey for PqKeyPair {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
        let mut signature = vec![0; self.0.algorithm().signature_len()];
        let len = self
            .0
            .sign(message, &mut signature)
            .map_err(|_| rcgen::Error::RingUnspecified)?;
        signature.truncate(len);
        Ok(signature)
    }
}

#[derive(Debug, Default)]
pub(crate) struct EndpointConfigBuilder {
    /// Ed25519 Private Key
    pub private_key: Option<[u8; 32]>,

    /// Note that the end-entity certificate must have the
    /// [Subject Alternative Name](https://tools.ietf.org/html/rfc6125#section-4.1)
    /// extension to describe, e.g., the valid DNS name.
    pub server_name: Option<String>,

    /// Server accepts both `server_name` and `alternate_server_name`
    /// connections from clients. However client only uses `server_name` when
    /// initiating outbound connections.
    pub alternate_server_name: Option<String>,

    pub transport_config: Option<quinn::TransportConfig>,

    pub key_exchange: Option<KeyExchange>,

    pub authentication: Option<Authentication>,

    /// Post-quantum Private Key
    pub pq_private_key: Option<PqPrivateKey>,
}

impl EndpointConfigBuilder {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn key_exchange(mut self, key_exchange: KeyExchange) -> Self {
        self.key_exchange = Some(key_exchange);
        self
    }

    pub fn authentication(mut self, authentication: Authentication) -> Self {
        self.authentication = Some(authentication);
        self
    }

    pub fn server_name<T: Into<String>>(mut self, server_name: T) -> Self {
        self.server_name = Some(server_name.into());
        self
    }

    pub fn alternate_server_name<T: Into<String>>(mut self, server_name: Option<T>) -> Self {
        self.alternate_server_name = server_name.map(Into::into);
        self
    }

    pub fn transport_config(mut self, transport_config: quinn::TransportConfig) -> Self {
        self.transport_config = Some(transport_config);
        self
    }

    pub fn private_key(mut self, private_key: [u8; 32]) -> Self {
        self.private_key = Some(private_key);
        self
    }

    pub fn pq_private_key(mut self, pq_private_key: Option<PqPrivateKey>) -> Self {
        self.pq_private_key = pq_private_key;
        self
    }

    #[cfg(test)]
    pub(crate) fn random_private_key(self) -> Self {
        let mut rng = rand::thread_rng();
        let mut bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rng, &mut bytes[..]);

        self.private_key(bytes)
    }

    pub fn build(self) -> Result<EndpointConfig> {
        let keypair = ed25519::KeypairBytes {
            secret_key: self.private_key.unwrap(),
            public_key: None,
        };

        // Derive our quic reset key from our private key using an HKDF
        let reset_key = crate::crypto::construct_reset_key(&keypair.secret_key);
        let quinn_endpoint_config = quinn::EndpointConfig::new(Arc::new(reset_key));

        let primary_server_name = self.server_name.unwrap();
        let transport_config = Arc::new(self.transport_config.unwrap_or_default());
        let crypto_provider = crypto_provider(self.key_exchange.unwrap_or_default());
        let authentication = self.authentication.unwrap_or_default();
        let pq_private_key = self
            .pq_private_key
            .filter(|_| authentication == Authentication::PostQuantum);

        let cert_verifier = Arc::new(CertVerifier {
            server_names: vec![primary_server_name.clone()],
            authentication,
        });
        let certified_keys = |server_name: &str| {
            let pq_private_key = pq_private_key.as_ref();
            Self::certified_keys(&keypair, pq_private_key, server_name, &crypto_provider)
        };
        let primary_certs = Arc::new(certified_keys(&primary_server_name)?);

        // Client only uses the primary `server_name` when initiating outbound connections
        // so only needs the primary certificates.
        let client_config = Self::client_config(
            primary_certs.clone(),
            cert_verifier.clone(),
            transport_config.clone(),
            crypto_provider.clone(),
        )?;

        // Always register a dedicated probe certificate/server-name in the cert resolver so any node
        // can be probed (the client selects it via SNI). It is intentionally not added to the
        // client-cert verifier: probing clients still authenticate with their primary certificate.
        let probe_cert_entry = (
            PROBE_SERVER_NAME.to_owned(),
            certified_keys(PROBE_SERVER_NAME)?,
        );
        let primary_cert_entry = (
            primary_server_name.clone(),
            CertifiedKeys::clone(&primary_certs),
        );

        let alternate_server_name = self.alternate_server_name;
        let server_config = match alternate_server_name {
            Some(alternate_server_name) => {
                let alternate_certs = certified_keys(&alternate_server_name)?;
                let cert_verifier = Arc::new(CertVerifier {
                    server_names: vec![primary_server_name.clone(), alternate_server_name.clone()],
                    authentication,
                });
                Self::server_config(
                    vec![
                        primary_cert_entry,
                        (alternate_server_name, alternate_certs),
                        probe_cert_entry,
                    ],
                    cert_verifier,
                    transport_config.clone(),
                    crypto_provider.clone(),
                )
            }
            _ => Self::server_config(
                vec![primary_cert_entry, probe_cert_entry],
                cert_verifier,
                transport_config.clone(),
                crypto_provider.clone(),
            ),
        }?;

        let peer_id =
            crate::crypto::peer_id_from_certificate(&primary_certs.ed25519.cert[0]).unwrap();

        Ok(EndpointConfig {
            peer_id,
            client_certs: primary_certs,
            authentication,
            crypto_provider,
            quinn_server_config: server_config,
            quinn_client_config: client_config,
            server_name: primary_server_name,
            transport_config,
            quinn_endpoint_config,
        })
    }

    fn generate_cert(
        keypair: &ed25519::KeypairBytes,
        server_name: &str,
    ) -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
        let pkcs8 = keypair.to_pkcs8_der().unwrap();
        let key_der = PrivateKeyDer::Pkcs8(pkcs8.as_bytes().to_vec().into());
        let keypair = KeyPair::from_der_and_sign_algo(&key_der, &rcgen::PKCS_ED25519).unwrap();
        let certificate = CertificateParams::new(vec![server_name.to_owned()])
            .unwrap()
            .self_signed(&keypair)
            .unwrap();

        let certificate_der = certificate.der().to_owned();

        (certificate_der, key_der)
    }

    /// Cert for our post-quantum key. The extension ties that key to the Ed25519 one, so a peer
    /// reads the same PeerId from either cert.
    fn generate_pq_cert(
        private_key: &[u8; 32],
        pq_private_key: &PqPrivateKey,
        server_name: &str,
    ) -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
        use rcgen::PublicKeyData;

        let keypair = PqKeyPair(pq_private_key.keypair(), pq_private_key.rcgen_algorithm());
        let pkcs8 = keypair.0.to_pkcs8v1().unwrap();
        let key_der = PrivateKeyDer::Pkcs8(pkcs8.as_ref().to_vec().into());

        let mut params = CertificateParams::new(vec![server_name.to_owned()]).unwrap();
        params
            .custom_extensions
            .push(rcgen::CustomExtension::from_oid_content(
                crate::crypto::PQ_IDENTITY_OID,
                crate::crypto::pq_identity_extension(
                    private_key,
                    &keypair.subject_public_key_info(),
                ),
            ));
        let certificate = params.self_signed(&keypair).unwrap();

        (certificate.der().to_owned(), key_der)
    }

    fn certified_keys(
        keypair: &ed25519::KeypairBytes,
        pq_private_key: Option<&PqPrivateKey>,
        server_name: &str,
        crypto_provider: &rustls::crypto::CryptoProvider,
    ) -> Result<CertifiedKeys> {
        let certified_key = |(cert, key_der)| -> Result<Arc<CertifiedKey>> {
            let key = crypto_provider
                .key_provider
                .load_private_key(key_der)
                .map_err(|_| anyhow::anyhow!("invalid private key"))?;
            Ok(Arc::new(CertifiedKey::new(vec![cert], key)))
        };

        let pq = pq_private_key
            .map(|pq_private_key| {
                let private_key = &keypair.secret_key;
                let pq_cert = Self::generate_pq_cert(private_key, pq_private_key, server_name);
                Ok::<_, crate::Error>((pq_private_key.scheme(), certified_key(pq_cert)?))
            })
            .transpose()?;

        Ok(CertifiedKeys {
            ed25519: certified_key(Self::generate_cert(keypair, server_name))?,
            pq,
        })
    }

    fn server_crypto(
        certs: Vec<(String, CertifiedKeys)>,
        cert_verifier: Arc<CertVerifier>,
        crypto_provider: Arc<rustls::crypto::CryptoProvider>,
    ) -> Result<rustls::ServerConfig> {
        let mut server_cert_resolver = ServerCertResolver {
            ed25519: ResolvesServerCertUsingSni::new(),
            pq: None,
        };
        for (server_name, certs) in certs {
            server_cert_resolver
                .ed25519
                .add(&server_name, CertifiedKey::clone(&certs.ed25519))?;
            if let Some((scheme, pq)) = &certs.pq {
                server_cert_resolver
                    .pq
                    .get_or_insert_with(|| (*scheme, ResolvesServerCertUsingSni::new()))
                    .1
                    .add(&server_name, CertifiedKey::clone(pq))?;
            }
        }

        Ok(rustls::ServerConfig::builder_with_provider(crypto_provider)
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_client_cert_verifier(cert_verifier)
            .with_cert_resolver(Arc::new(server_cert_resolver)))
    }

    fn server_config(
        certs: Vec<(String, CertifiedKeys)>,
        cert_verifier: Arc<CertVerifier>,
        transport_config: Arc<quinn::TransportConfig>,
        crypto_provider: Arc<rustls::crypto::CryptoProvider>,
    ) -> Result<quinn::ServerConfig> {
        let server_crypto = Self::server_crypto(certs, cert_verifier, crypto_provider)?;

        let mut server = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto).unwrap(),
        ));
        server.transport = transport_config;
        Ok(server)
    }

    fn client_crypto(
        certs: Arc<CertifiedKeys>,
        cert_verifier: Arc<dyn ServerCertVerifier>,
        crypto_provider: Arc<rustls::crypto::CryptoProvider>,
    ) -> Result<rustls::ClientConfig> {
        Ok(rustls::ClientConfig::builder_with_provider(crypto_provider)
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .dangerous()
            .with_custom_certificate_verifier(cert_verifier)
            .with_client_cert_resolver(certs))
    }

    fn client_config(
        certs: Arc<CertifiedKeys>,
        cert_verifier: Arc<CertVerifier>,
        transport_config: Arc<quinn::TransportConfig>,
        crypto_provider: Arc<rustls::crypto::CryptoProvider>,
    ) -> Result<quinn::ClientConfig> {
        let client_crypto = Self::client_crypto(certs, cert_verifier, crypto_provider)?;

        let mut client = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap(),
        ));
        client.transport_config(transport_config);
        Ok(client)
    }
}

#[derive(Debug)]
pub(crate) struct EndpointConfig {
    peer_id: PeerId,
    // Store client certificates for outbound connections initiation
    client_certs: Arc<CertifiedKeys>,
    authentication: Authentication,
    crypto_provider: Arc<rustls::crypto::CryptoProvider>,
    quinn_server_config: quinn::ServerConfig,
    quinn_client_config: quinn::ClientConfig,

    /// Note that the end-entity certificate must have the
    /// [Subject Alternative Name](https://tools.ietf.org/html/rfc6125#section-4.1)
    /// extension to describe, e.g., the valid DNS name.
    server_name: String,

    transport_config: Arc<quinn::TransportConfig>,
    quinn_endpoint_config: quinn::EndpointConfig,
}

impl EndpointConfig {
    pub fn builder() -> EndpointConfigBuilder {
        EndpointConfigBuilder::new()
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    pub fn quinn_endpoint_config(&self) -> quinn::EndpointConfig {
        self.quinn_endpoint_config.clone()
    }

    pub fn server_config(&self) -> &quinn::ServerConfig {
        &self.quinn_server_config
    }

    pub fn client_config(&self) -> &quinn::ClientConfig {
        &self.quinn_client_config
    }

    fn client_config_with_verifier(&self, verifier: ExpectedCertVerifier) -> quinn::ClientConfig {
        let client_crypto = EndpointConfigBuilder::client_crypto(
            self.client_certs.clone(),
            Arc::new(verifier),
            self.crypto_provider.clone(),
        )
        .unwrap();

        let mut client = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap(),
        ));
        client.transport_config(self.transport_config.clone());
        client
    }

    pub fn client_config_with_expected_server_identity(
        &self,
        peer_id: PeerId,
    ) -> quinn::ClientConfig {
        self.client_config_with_verifier(ExpectedCertVerifier(
            CertVerifier {
                server_names: vec![self.server_name().into()],
                authentication: self.authentication,
            },
            peer_id,
        ))
    }

    /// Client config for a probe connection: verifies the server's identity matches `peer_id` and
    /// uses the dedicated probe server-name so the server can recognize and decline the probe (see
    /// [`PROBE_SERVER_NAME`]). The probe must also pass `PROBE_SERVER_NAME` as the SNI when dialing.
    pub fn client_config_for_probe(&self, peer_id: PeerId) -> quinn::ClientConfig {
        self.client_config_with_verifier(ExpectedCertVerifier(
            CertVerifier {
                server_names: vec![PROBE_SERVER_NAME.into()],
                authentication: self.authentication,
            },
            peer_id,
        ))
    }

    #[cfg(test)]
    pub(crate) fn random(server_name: &str) -> Self {
        Self::builder()
            .random_private_key()
            .server_name(server_name)
            .build()
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::crypto::CryptoProvider;
    use rustls::pki_types::ServerName;
    use rustls::{HandshakeKind, NamedGroup};
    use KeyExchange::{Classical, Hybrid};

    const SERVER_NAME: &str = "test";

    fn random_keypair() -> ed25519::KeypairBytes {
        let mut secret_key = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut secret_key);
        ed25519::KeypairBytes {
            secret_key,
            public_key: None,
        }
    }

    fn verifier(authentication: Authentication) -> Arc<CertVerifier> {
        Arc::new(CertVerifier {
            server_names: vec![SERVER_NAME.to_owned()],
            authentication,
        })
    }

    /// One side of a handshake: its keys and what it lists.
    struct Node {
        keypair: ed25519::KeypairBytes,
        pq_private_key: Option<PqPrivateKey>,
        authentication: Authentication,
    }

    impl Node {
        fn new() -> Self {
            Self {
                keypair: random_keypair(),
                pq_private_key: Some(PqPrivateKey::MlDsa44(random_keypair().secret_key)),
                authentication: Authentication::PostQuantum,
            }
        }

        fn certified_keys(&self, provider: &CryptoProvider) -> CertifiedKeys {
            let pq_private_key = self.pq_private_key.as_ref();
            EndpointConfigBuilder::certified_keys(
                &self.keypair,
                pq_private_key,
                SERVER_NAME,
                provider,
            )
            .unwrap()
        }

        fn client(&self, provider: Arc<CryptoProvider>) -> Arc<rustls::ClientConfig> {
            let certs = Arc::new(self.certified_keys(&provider));
            let verifier = verifier(self.authentication);
            Arc::new(EndpointConfigBuilder::client_crypto(certs, verifier, provider).unwrap())
        }

        fn server(&self, provider: Arc<CryptoProvider>) -> Arc<rustls::ServerConfig> {
            let certs = vec![(SERVER_NAME.to_owned(), self.certified_keys(&provider))];
            let verifier = verifier(self.authentication);
            Arc::new(EndpointConfigBuilder::server_crypto(certs, verifier, provider).unwrap())
        }

        /// What a peer reads from our Ed25519 cert.
        fn ed25519_identity(&self) -> (PeerId, Option<crate::PqPublicKey>) {
            let (cert, _) = EndpointConfigBuilder::generate_cert(&self.keypair, SERVER_NAME);
            (
                crate::crypto::peer_id_from_certificate(&cert).unwrap(),
                None,
            )
        }

        /// What a peer reads from our post-quantum cert.
        fn pq_identity(&self) -> (PeerId, Option<crate::PqPublicKey>) {
            let pq_public_key = self.pq_private_key.as_ref().unwrap().public_key();
            (self.ed25519_identity().0, Some(pq_public_key))
        }
    }

    fn client_tls(provider: Arc<CryptoProvider>) -> Arc<rustls::ClientConfig> {
        Node::new().client(provider)
    }

    fn server_tls(provider: Arc<CryptoProvider>) -> Arc<rustls::ServerConfig> {
        Node::new().server(provider)
    }

    /// quinn has no public API for the negotiated group, so drive rustls directly.
    fn handshake(
        client: &Arc<rustls::ClientConfig>,
        server: &Arc<rustls::ServerConfig>,
    ) -> (NamedGroup, HandshakeKind) {
        let (client, server) = connect(client, server);

        let group = client.negotiated_key_exchange_group().unwrap().name();
        assert_eq!(
            group,
            server.negotiated_key_exchange_group().unwrap().name()
        );
        (group, client.handshake_kind().unwrap())
    }

    fn connect(
        client: &Arc<rustls::ClientConfig>,
        server: &Arc<rustls::ServerConfig>,
    ) -> (rustls::ClientConnection, rustls::ServerConnection) {
        let name = ServerName::try_from(SERVER_NAME).unwrap();
        let mut client = rustls::ClientConnection::new(client.clone(), name).unwrap();
        let mut server = rustls::ServerConnection::new(server.clone()).unwrap();

        // Three round trips cover a HelloRetryRequest, and the extra ones deliver the tickets.
        let mut buf = Vec::new();
        for _ in 0..5 {
            buf.clear();
            client.write_tls(&mut buf).unwrap();
            let mut pending = buf.as_slice();
            while !pending.is_empty() {
                server.read_tls(&mut pending).unwrap();
                server.process_new_packets().unwrap();
            }

            buf.clear();
            server.write_tls(&mut buf).unwrap();
            let mut pending = buf.as_slice();
            while !pending.is_empty() {
                client.read_tls(&mut pending).unwrap();
                client.process_new_packets().unwrap();
            }
        }
        assert!(!client.is_handshaking() && !server.is_handshaking());

        (client, server)
    }

    fn negotiated_group(client: KeyExchange, server: KeyExchange) -> NamedGroup {
        let client = client_tls(crypto_provider(client));
        handshake(&client, &server_tls(crypto_provider(server))).0
    }

    #[test]
    fn classical_offers_what_anemo_offered_before() {
        let groups = |key_exchange| -> Vec<NamedGroup> {
            let provider = crypto_provider(key_exchange);
            provider.kx_groups.iter().map(|g| g.name()).collect()
        };
        let before = [
            NamedGroup::X25519,
            NamedGroup::secp256r1,
            NamedGroup::secp384r1,
        ];

        assert_eq!(groups(Classical), before);
        assert_eq!(groups(Hybrid)[0], NamedGroup::X25519MLKEM768);
        assert_eq!(groups(Hybrid)[1..], before);
    }

    #[test]
    fn key_exchange_defaults_to_hybrid() {
        let parse = |json| serde_json::from_str::<Config>(json).unwrap().key_exchange;

        assert_eq!(parse("{}").unwrap_or_default(), Hybrid);
        assert_eq!(parse(r#"{"key-exchange":"classical"}"#), Some(Classical));
    }

    #[test]
    fn hybrid_peers_negotiate_x25519mlkem768() {
        assert_eq!(negotiated_group(Hybrid, Hybrid), NamedGroup::X25519MLKEM768);
    }

    #[test]
    fn classical_peer_negotiates_x25519_in_either_direction() {
        for (client, server) in [
            (Hybrid, Classical),
            (Classical, Hybrid),
            (Classical, Classical),
        ] {
            assert_eq!(negotiated_group(client, server), NamedGroup::X25519);
        }
    }

    /// rustls remembers the last group per server name, and every peer of a network shares one.
    #[test]
    fn stale_x25519_hint_costs_a_retry_but_ends_on_hybrid() {
        let client = client_tls(crypto_provider(Hybrid));

        let classical = server_tls(crypto_provider(Classical));
        assert_eq!(
            handshake(&client, &classical),
            (NamedGroup::X25519, HandshakeKind::Full)
        );

        let hybrid = server_tls(crypto_provider(Hybrid));
        assert_eq!(
            handshake(&client, &hybrid),
            (
                NamedGroup::X25519MLKEM768,
                HandshakeKind::FullWithHelloRetryRequest
            )
        );
    }

    #[test]
    fn resumed_session_runs_the_hybrid_exchange_again() {
        let client = client_tls(crypto_provider(Hybrid));
        let server = server_tls(crypto_provider(Hybrid));

        assert_eq!(
            handshake(&client, &server),
            (NamedGroup::X25519MLKEM768, HandshakeKind::Full)
        );
        assert_eq!(
            handshake(&client, &server),
            (NamedGroup::X25519MLKEM768, HandshakeKind::Resumed)
        );

        // No 0-RTT, so nothing is ever sent under resumption keys alone.
        assert!(!client.enable_early_data);
        assert_eq!(server.max_early_data_size, 0);
    }

    /// The PeerId is the Ed25519 public key, whatever backend builds the certificate.
    #[test]
    fn peer_id_for_a_fixed_key() {
        let keypair = ed25519::KeypairBytes {
            secret_key: [7; 32],
            public_key: None,
        };

        for name in [SERVER_NAME, PROBE_SERVER_NAME] {
            let (cert, _) = EndpointConfigBuilder::generate_cert(&keypair, name);
            let peer_id = crate::crypto::peer_id_from_certificate(&cert).unwrap();
            assert_eq!(
                hex::encode(peer_id.0),
                "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c"
            );
        }
    }

    /// PeerId and post-quantum key (if it showed that cert) of the server and of the client, each as
    /// the other side saw it.
    fn identities(
        client: &Arc<rustls::ClientConfig>,
        server: &Arc<rustls::ServerConfig>,
    ) -> [(PeerId, Option<crate::PqPublicKey>); 2] {
        let (client, server) = connect(client, server);
        let identity = |certs: Option<&[CertificateDer<'static>]>| {
            crate::crypto::peer_identity_from_certificate(&certs.unwrap()[0]).unwrap()
        };
        [
            identity(client.peer_certificates()),
            identity(server.peer_certificates()),
        ]
    }

    #[test]
    fn peers_show_pq_certs_under_the_same_peer_id() {
        let (client, server) = (Node::new(), Node::new());
        let provider = crypto_provider(Hybrid);

        let [server_seen, client_seen] =
            identities(&client.client(provider.clone()), &server.server(provider));

        assert_eq!(server_seen, server.pq_identity());
        assert_eq!(client_seen, client.pq_identity());
    }

    /// The post-quantum key is optional. A node without one still takes the cert of a peer
    /// that has one.
    #[test]
    fn node_without_a_pq_key_shows_its_ed25519_cert() {
        let with_key = Node::new();
        let without_key = Node {
            pq_private_key: None,
            ..Node::new()
        };
        let provider = crypto_provider(Hybrid);

        let [server_seen, client_seen] = identities(
            &without_key.client(provider.clone()),
            &with_key.server(provider.clone()),
        );
        assert_eq!(server_seen, with_key.pq_identity());
        assert_eq!(client_seen, without_key.ed25519_identity());

        let [server_seen, client_seen] = identities(
            &with_key.client(provider.clone()),
            &without_key.server(provider),
        );
        assert_eq!(server_seen, without_key.ed25519_identity());
        assert_eq!(client_seen, with_key.pq_identity());
    }

    #[test]
    fn authentication_defaults_to_post_quantum() {
        let parse = |json| serde_json::from_str::<Config>(json).unwrap().authentication;

        assert_eq!(parse("{}").unwrap_or_default(), Authentication::PostQuantum);
        assert_eq!(
            parse(r#"{"authentication":"post-quantum"}"#),
            Some(Authentication::PostQuantum)
        );
        assert_eq!(
            parse(r#"{"authentication":"classical"}"#),
            Some(Authentication::Classical)
        );
    }

    /// A classical peer lists Ed25519 only, like a peer from before this change, so both sides end up
    /// on Ed25519 certs whoever dials.
    #[test]
    fn classical_peer_gets_the_ed25519_cert() {
        let node = Node::new();
        let classical = Node {
            pq_private_key: None,
            authentication: Authentication::Classical,
            ..Node::new()
        };
        assert_eq!(
            ServerCertVerifier::supported_verify_schemes(&*verifier(classical.authentication)),
            [SignatureScheme::ED25519]
        );
        let provider = crypto_provider(Hybrid);

        let [server_seen, client_seen] = identities(
            &classical.client(provider.clone()),
            &node.server(provider.clone()),
        );
        assert_eq!(server_seen, node.ed25519_identity());
        assert_eq!(client_seen, classical.ed25519_identity());

        let [server_seen, client_seen] =
            identities(&node.client(provider.clone()), &classical.server(provider));
        assert_eq!(server_seen, classical.ed25519_identity());
        assert_eq!(client_seen, node.ed25519_identity());
    }

    /// A post-quantum cert only gets a PeerId through that PeerId's signature over its key.
    #[test]
    fn pq_cert_cannot_claim_another_peer_id() {
        use rcgen::PublicKeyData;

        let (victim, attacker) = ([1; 32], [2; 32]);
        let keypair = |seed| {
            let pq_private_key = PqPrivateKey::MlDsa44(seed);
            PqKeyPair(pq_private_key.keypair(), pq_private_key.rcgen_algorithm())
        };
        let (victim_pq, attacker_pq) = (keypair([3; 32]), keypair([4; 32]));
        let identity = |extension: Option<Vec<u8>>| {
            let mut params = CertificateParams::new(vec![SERVER_NAME.to_owned()]).unwrap();
            params.custom_extensions.extend(extension.map(|content| {
                rcgen::CustomExtension::from_oid_content(crate::crypto::PQ_IDENTITY_OID, content)
            }));
            let cert = params.self_signed(&attacker_pq).unwrap();
            crate::crypto::peer_identity_from_certificate(cert.der())
        };
        let extension = crate::crypto::pq_identity_extension;

        let honest = extension(&attacker, &attacker_pq.subject_public_key_info());
        // The victim's own extension, which signs for the victim's key, not this one.
        let replayed = extension(&victim, &victim_pq.subject_public_key_info());
        // The attacker's signature with the victim's Ed25519 key swapped in.
        let mut forged = honest.clone();
        forged[2..34].copy_from_slice(&replayed[2..34]);
        // A signature over the bare key, which would not say what scheme the key is for.
        let schemeless = extension(&attacker, attacker_pq.der_bytes());

        assert!(identity(Some(honest)).is_ok());
        for extension in [None, Some(replayed), Some(forged), Some(schemeless)] {
            assert!(identity(extension).is_err());
        }
    }

    fn pq_cert(node: &Node) -> CertificateDer<'static> {
        let pq_private_key = node.pq_private_key.as_ref().unwrap();
        let private_key = &node.keypair.secret_key;
        EndpointConfigBuilder::generate_pq_cert(private_key, pq_private_key, SERVER_NAME).0
    }

    fn accepts(authentication: Authentication, cert: &CertificateDer<'_>) -> [bool; 2] {
        use rustls::server::danger::ClientCertVerifier;

        let verifier = verifier(authentication);
        let name = ServerName::try_from(SERVER_NAME).unwrap();
        let now = rustls::pki_types::UnixTime::now();
        [
            verifier
                .verify_server_cert(cert, &[], &name, &[], now)
                .is_ok(),
            verifier.verify_client_cert(cert, &[], now).is_ok(),
        ]
    }

    /// Classical is the rollback, so a post-quantum cert must not get through even from a peer
    /// that sends one without being asked.
    #[test]
    fn classical_node_refuses_a_pq_cert() {
        let pq_cert = pq_cert(&Node::new());

        assert_eq!(accepts(Authentication::PostQuantum, &pq_cert), [true; 2]);
        assert_eq!(accepts(Authentication::Classical, &pq_cert), [false; 2]);
    }

    /// The cert is its own trust anchor, and its own signature is still checked.
    #[test]
    fn pq_cert_with_a_broken_self_signature_is_refused() {
        let mut pq_cert = pq_cert(&Node::new()).to_vec();
        *pq_cert.last_mut().unwrap() ^= 1;

        assert_eq!(
            accepts(Authentication::PostQuantum, &pq_cert.into()),
            [false; 2]
        );
    }
}
