use crate::{ConnectionOrigin, PeerId, Result};
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
    inner: ConnectionInner,
    peer_id: PeerId,
    origin: ConnectionOrigin,

    // Time that the connection was established
    time_established: std::time::Instant,
}

impl Connection {
    pub fn new(inner: ConnectionInner, origin: ConnectionOrigin) -> Result<Self> {
        let peer_id = Self::try_peer_id(&inner)?;
        Ok(Self {
            inner,
            peer_id,
            origin,
            time_established: std::time::Instant::now(),
        })
    }

    /// Try to query Cryptographic identity of the peer
    fn try_peer_id(connection: &ConnectionInner) -> Result<PeerId> {
        match connection {
            ConnectionInner::Quic(connection) => {
                // Query the certificate chain provided by a [TLS
                // Connection](https://docs.rs/rustls/0.20.4/rustls/enum.Connection.html#method.peer_certificates).
                // The first cert in the chain is guaranteed to be the peer
                let cert = &connection
                    .peer_identity()
                    .unwrap()
                    .downcast::<Vec<rustls::Certificate>>()
                    .unwrap()[0];
                crate::crypto::peer_id_from_certificate(cert)
            }
            ConnectionInner::Tls(connection) => {
                let cert = connection.peer_identity().ok_or(anyhow::anyhow!(
                    "TLS connection does not have a peer identity"
                ))?;
                crate::crypto::peer_id_from_certificate(cert)
            }
        }
        .map_err(Into::into)
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
        match &self.inner {
            ConnectionInner::Quic(connection) => connection.stable_id(),
            ConnectionInner::Tls(connection) => connection.stable_id(),
        }
    }

    /// Current best estimate of this connection's latency (round-trip-time)
    #[allow(unused)]
    pub fn rtt(&self) -> Duration {
        match &self.inner {
            ConnectionInner::Quic(connection) => connection.rtt(),
            ConnectionInner::Tls(connection) => {
                // TODO: Implement this for TLS connections, or change the interface.
                Duration::ZERO
            }
        }
    }

    /// Returns connection statistics
    pub fn stats(&self) -> ConnectionStats {
        match &self.inner {
            ConnectionInner::Quic(connection) => connection.stats(),
            ConnectionInner::Tls(_connection) => {
                // TODO: Implement this for TLS connections, or change the interface.
                ConnectionStats::default()
            }
        }
    }

    /// The peer's UDP address
    ///
    /// If `ServerConfig::migration` is `true`, clients may change addresses at will, e.g. when
    /// switching to a cellular internet connection.
    pub fn remote_address(&self) -> SocketAddr {
        match &self.inner {
            ConnectionInner::Quic(connection) => connection.remote_address(),
            ConnectionInner::Tls(connection) => connection.peer_address(),
        }
    }

    /// Open a unidirection stream to the peer.
    ///
    /// Messages sent over the stream will arrive at the peer in the order they were sent.
    pub async fn open_uni(&self) -> Result<SendStream, ConnectionError> {
        match &self.inner {
            ConnectionInner::Quic(connection) => connection
                .open_uni()
                .await
                .map(SendStream::Quic)
                .map_err(Into::into),
            ConnectionInner::Tls(connection) => {
                let (send, _recv) = connection.open_stream().await?;
                Ok(SendStream::Tls(send))
            }
        }
    }

    /// Open a bidirectional stream to the peer.
    ///
    /// Bidirectional streams allow messages to be sent in both directions. This can be useful to
    /// automatically correlate response messages, for example.
    ///
    /// Messages sent over the stream will arrive at the peer in the order they were sent.
    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream), ConnectionError> {
        match &self.inner {
            ConnectionInner::Quic(connection) => connection
                .open_bi()
                .await
                .map(|(send, recv)| (SendStream::Quic(send), RecvStream::Quic(recv)))
                .map_err(Into::into),
            ConnectionInner::Tls(connection) => connection
                .open_stream()
                .await
                .map(|(send, recv)| (SendStream::Tls(send), RecvStream::Tls(recv)))
                .map_err(Into::into),
        }
    }

    /// Close the connection immediately.
    ///
    /// This is not a graceful close - pending operations will fail immediately and data on
    /// unfinished streams is not guaranteed to be delivered.
    pub fn close(&self) {
        trace!("Closing Connection");
        match &self.inner {
            ConnectionInner::Quic(connection) => {
                connection.close(0_u32.into(), b"connection closed")
            }
            // TODO: Implement close for TLS.
            ConnectionInner::Tls(_connection) => (),
        }
    }

    /// Accept the next incoming uni-directional stream
    pub async fn accept_uni(&self) -> Result<RecvStream, ConnectionError> {
        match &self.inner {
            ConnectionInner::Quic(connection) => connection
                .accept_uni()
                .await
                .map(RecvStream::Quic)
                .map_err(Into::into),
            ConnectionInner::Tls(connection) => {
                let (_send, recv) = connection.accept_stream().await?;
                Ok(RecvStream::Tls(recv))
            }
        }
    }

    /// Accept the next incoming bidirectional stream
    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream), ConnectionError> {
        match &self.inner {
            ConnectionInner::Quic(connection) => connection
                .accept_bi()
                .await
                .map(|(send, recv)| (SendStream::Quic(send), RecvStream::Quic(recv)))
                .map_err(Into::into),
            ConnectionInner::Tls(connection) => connection
                .accept_stream()
                .await
                .map(|(send, recv)| (SendStream::Tls(send), RecvStream::Tls(recv)))
                .map_err(Into::into),
        }
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

#[derive(Clone)]
pub(crate) enum ConnectionInner {
    Quic(quinn::Connection),
    Tls(anemo_tls::Connection),
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error(transparent)]
    Quic(#[from] quinn::ConnectionError),
    #[error(transparent)]
    Tls(#[from] anyhow::Error),
}

/// A wrapper around a transport layer SendStream that enforces that the stream is shut down
/// immediately when dropped. The proper way to ensure that all data has been successfully
/// transmitted and Ack'd by the remote side is to call [SendStream::finish] prior to dropping
/// the stream.
pub(crate) enum SendStream {
    Quic(quinn::SendStream),
    Tls(anemo_tls::SendStream),
}

impl fmt::Display for SendStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SendStream::Quic(stream) => write!(f, "Quic({})", stream.id()),
            SendStream::Tls(stream) => write!(f, "Tls({})", stream),
        }
    }
}

impl SendStream {
    /// Shut down the send stream gracefully. No new data may be written after calling this method.
    pub async fn finish(&mut self) -> Result<()> {
        match self {
            SendStream::Quic(stream) => stream.finish().await.map_err(Into::into),
            SendStream::Tls(stream) => tokio::io::AsyncWriteExt::shutdown(stream)
                .await
                .map_err(Into::into),
        }
    }

    /// Completes if/when the peer stops the stream.
    pub async fn stopped(&mut self) {
        match self {
            SendStream::Quic(stream) => {
                let _ = stream.stopped().await;
            }
            // Stream cannot be stopped/reset for yamux.
            SendStream::Tls(_stream) => futures::future::pending().await,
        }
    }
}

impl Drop for SendStream {
    fn drop(&mut self) {
        match self {
            SendStream::Quic(stream) => {
                // We don't care if the stream has already been closed
                let _ = stream.reset(0u8.into());
            }
            // Nothing to do on drop for TLS.
            SendStream::Tls(_stream) => (),
        }
    }
}

impl tokio::io::AsyncWrite for SendStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            SendStream::Quic(stream) => Pin::new(stream).poll_write(cx, buf),
            SendStream::Tls(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        match self.get_mut() {
            SendStream::Quic(stream) => Pin::new(stream).poll_flush(cx),
            SendStream::Tls(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        match self.get_mut() {
            SendStream::Quic(stream) => Pin::new(stream).poll_shutdown(cx),
            SendStream::Tls(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

pub(crate) enum RecvStream {
    Quic(quinn::RecvStream),
    Tls(anemo_tls::RecvStream),
}

impl fmt::Display for RecvStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecvStream::Quic(stream) => write!(f, "Quic({})", stream.id()),
            RecvStream::Tls(stream) => write!(f, "Tls({})", stream),
        }
    }
}

impl tokio::io::AsyncRead for RecvStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            RecvStream::Quic(stream) => Pin::new(stream).poll_read(cx, buf),
            RecvStream::Tls(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl futures::AsyncRead for RecvStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            RecvStream::Quic(stream) => Pin::new(stream).poll_read(cx, buf),
            RecvStream::Tls(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}
