use crate::{ConnectionOrigin, PeerId, Result};
use quinn::{ConnectionError, RecvStream};
use quinn_proto::ConnectionStats;
use std::{
    fmt, io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tracing::trace;

#[derive(Clone)]
pub(crate) struct Connection {
    inner: quinn::Connection,
    peer_id: PeerId,
    origin: ConnectionOrigin,

    // Time that the connection was established
    time_established: std::time::Instant,
}

impl Connection {
    pub fn new(inner: quinn::Connection, origin: ConnectionOrigin) -> Result<Self> {
        let peer_id = Self::try_peer_id(&inner)?;
        Ok(Self {
            inner,
            peer_id,
            origin,
            time_established: std::time::Instant::now(),
        })
    }

    /// Try to query Cryptographic identity of the peer
    fn try_peer_id(connection: &quinn::Connection) -> Result<PeerId> {
        // Query the certificate chain provided by a [TLS
        // Connection](https://docs.rs/rustls/0.20.4/rustls/enum.Connection.html#method.peer_certificates).
        // The first cert in the chain is guaranteed to be the peer
        let peer_cert = &connection
            .peer_identity()
            .unwrap()
            .downcast::<Vec<rustls::Certificate>>()
            .unwrap()[0];

        let peer_id = crate::crypto::peer_id_from_certificate(peer_cert)?;

        Ok(peer_id)
    }

    /// PeerId of the Remote Peer
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// Origin of the Connection
    pub fn origin(&self) -> ConnectionOrigin {
        self.origin
    }

    /// Time the Connection was established
    #[allow(unused)]
    pub fn time_established(&self) -> std::time::Instant {
        self.time_established
    }

    /// A stable identifier for this connection
    ///
    /// Peer addresses and connection IDs can change, but this value will remain
    /// fixed for the lifetime of the connection.
    pub fn stable_id(&self) -> usize {
        self.inner.stable_id()
    }

    /// Current best estimate of this connection's latency (round-trip-time)
    #[allow(unused)]
    pub fn rtt(&self) -> Duration {
        self.inner.rtt()
    }

    /// Returns connection statistics
    pub fn stats(&self) -> ConnectionStats {
        self.inner.stats()
    }

    /// The peer's UDP address
    ///
    /// If `ServerConfig::migration` is `true`, clients may change addresses at will, e.g. when
    /// switching to a cellular internet connection.
    pub fn remote_address(&self) -> SocketAddr {
        self.inner.remote_address()
    }

    /// Open a unidirection stream to the peer.
    ///
    /// Messages sent over the stream will arrive at the peer in the order they were sent.
    #[allow(dead_code)]
    pub async fn open_uni(&self) -> Result<SendStream, ConnectionError> {
        self.inner.open_uni().await.map(SendStream)
    }

    /// Open a bidirectional stream to the peer.
    ///
    /// Bidirectional streams allow messages to be sent in both directions. This can be useful to
    /// automatically correlate response messages, for example.
    ///
    /// Messages sent over the stream will arrive at the peer in the order they were sent.
    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream), ConnectionError> {
        self.inner
            .open_bi()
            .await
            .map(|(send, recv)| (SendStream(send), recv))
    }

    /// Close the connection immediately.
    ///
    /// This is not a graceful close - pending operations will fail immediately and data on
    /// unfinished streams is not guaranteed to be delivered.
    pub fn close(&self) {
        trace!("Closing Connection");
        self.inner.close(0_u32.into(), b"connection closed")
    }

    /// Accept the next incoming uni-directional stream
    pub async fn accept_uni(&self) -> Result<RecvStream, ConnectionError> {
        self.inner.accept_uni().await
    }

    /// Accept the next incoming bidirectional stream
    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream), ConnectionError> {
        self.inner
            .accept_bi()
            .await
            .map(|(send, recv)| (SendStream(send), recv))
    }

    /// Receive an application datagram
    pub async fn read_datagram(&self) -> Result<bytes::Bytes, ConnectionError> {
        self.inner.read_datagram().await
    }
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("origin", &self.origin())
            .field("id", &self.stable_id())
            .field("remote_address", &self.remote_address())
            .field("peer_id", &self.peer_id())
            .finish_non_exhaustive()
    }
}

/// A wrapper around a [quinn::SendStream] that enforces that the stream is shut down immediately
/// when dropped. The proper way to ensure that all data has been successfully transmitted and
/// Ack'd by the remote side is to call [quinn::SendStream::finish] prior to dropping the stream.
pub(crate) struct SendStream(quinn::SendStream);

impl Drop for SendStream {
    fn drop(&mut self) {
        // We don't care if the stream has already been closed
        let _ = self.0.reset(0u8.into());
    }
}

impl std::ops::Deref for SendStream {
    type Target = quinn::SendStream;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for SendStream {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl tokio::io::AsyncWrite for SendStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}
