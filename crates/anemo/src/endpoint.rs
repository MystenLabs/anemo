use crate::{
    config::EndpointConfig, connection::Connection, types::Address, ConnectionOrigin, PeerId,
    Result,
};
use std::sync::Arc;
use std::time::Duration;
use std::{
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::RwLock,
    task::{Context, Poll},
};
use tap::Pipe;
use tokio::time::timeout;
use tracing::{trace, warn};

/// A QUIC endpoint.
///
/// An endpoint corresponds to a single UDP socket, may host many connections, and may act as both
/// client and server for different connections.
#[derive(Debug)]
pub(crate) struct Endpoint {
    inner: quinn::Endpoint,
    local_addr: RwLock<SocketAddr>,
    config: EndpointConfig,
}

impl Endpoint {
    pub fn new(config: EndpointConfig, socket: std::net::UdpSocket) -> Result<Self> {
        let local_addr = socket.local_addr()?.pipe(RwLock::new);
        let server_config = config.server_config().clone();
        let endpoint = quinn::Endpoint::new(
            config.quinn_endpoint_config(),
            Some(server_config),
            socket,
            Arc::new(quinn::TokioRuntime),
        )?;

        let endpoint = Self {
            inner: endpoint,
            local_addr,
            config,
        };

        Ok(endpoint)
    }

    #[cfg(test)]
    fn new_with_address<A: Into<Address>>(config: EndpointConfig, addr: A) -> Result<Self> {
        let socket = std::net::UdpSocket::bind(addr.into())?;
        Self::new(config, socket)
    }

    pub fn connect(&self, address: Address) -> Result<Connecting> {
        self.connect_with_client_config(self.config.client_config().clone(), address)
    }

    pub fn connect_with_expected_peer_id(
        &self,
        address: Address,
        peer_id: PeerId,
    ) -> Result<Connecting> {
        let config = self
            .config
            .client_config_with_expected_server_identity(peer_id);
        self.connect_with_client_config(config, address)
    }

    fn connect_with_client_config(
        &self,
        config: quinn::ClientConfig,
        address: Address,
    ) -> Result<Connecting> {
        let addr = address.resolve()?;

        self.inner
            .connect_with(config, addr, self.config.server_name())
            .map_err(Into::into)
            .map(Connecting::new_outbound)
    }

    /// Returns the socket address that this Endpoint is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        *self.local_addr.read().unwrap()
    }

    pub fn peer_id(&self) -> PeerId {
        self.config().peer_id()
    }

    pub fn config(&self) -> &EndpointConfig {
        &self.config
    }

    /// Close all of this endpoint's connections immediately and cease accepting new connections.
    pub fn close(&self) {
        trace!("Closing endpoint");
        self.inner.close(0_u32.into(), b"endpoint closed")
    }

    /// Wait for all connections on the endpoint to be cleanly shut down
    ///
    /// Waiting for this condition before exiting ensures that a good-faith effort is made to notify
    /// peers of recent connection closes, whereas exiting immediately could force them to wait out
    /// the idle timeout period.
    ///
    /// Does not proactively close existing connections or cause incoming connections to be
    /// rejected. Consider calling [`close()`] if that is desired.
    ///
    /// A max_timeout property should be provided to ensure that the method
    /// will only wait for the designated duration and exit if the limit has been reached.
    ///
    /// [`close()`]: Endpoint::close
    pub async fn wait_idle(&self, max_timeout: Duration) {
        if timeout(max_timeout, self.inner.wait_idle()).await.is_err() {
            warn!(
                "Max timeout reached {}s while waiting for connections clean shutdown",
                max_timeout.as_secs_f64()
            );
        }
    }

    /// Switch to a new UDP socket
    ///
    /// Allows the endpoint's address to be updated live, affecting all active connections. Incoming
    /// connections and connections to servers unreachable from the new address will be lost.
    ///
    /// On error, the old UDP socket is retained.
    pub fn rebind(&self, socket: std::net::UdpSocket) -> std::io::Result<()> {
        let local_addr = socket.local_addr()?;
        self.inner.rebind(socket)?;
        *self.local_addr.write().unwrap() = local_addr;

        Ok(())
    }

    /// Get the next incoming connection attempt from a client
    ///
    /// Yields [`Connecting`] futures that must be `await`ed to obtain the final `Connection`, or
    /// `None` if the endpoint is [`close`](Self::close)d.
    pub(crate) fn accept(&self) -> Accept<'_> {
        Accept {
            inner: self.inner.accept(),
        }
    }
}

pin_project_lite::pin_project! {
    /// Future produced by [`Endpoint::accept`]
    #[must_use = "futures/streams/sinks do nothing unless you `.await` or poll them"]
    pub(crate) struct Accept<'a> {
        #[pin]
        inner: quinn::Accept<'a>,
    }
}

impl<'a> Future for Accept<'a> {
    type Output = Option<Connecting>;

    fn poll(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project()
            .inner
            .poll(ctx)
            .map(|maybe_connecting| maybe_connecting.map(Connecting::new_inbound))
    }
}

#[derive(Debug)]
#[must_use = "futures/streams/sinks do nothing unless you `.await` or poll them"]
pub(crate) struct Connecting {
    inner: quinn::Connecting,
    origin: ConnectionOrigin,
}

impl Connecting {
    pub(crate) fn new(inner: quinn::Connecting, origin: ConnectionOrigin) -> Self {
        Self { inner, origin }
    }

    pub(crate) fn new_inbound(inner: quinn::Connecting) -> Self {
        Self::new(inner, ConnectionOrigin::Inbound)
    }

    pub(crate) fn new_outbound(inner: quinn::Connecting) -> Self {
        Self::new(inner, ConnectionOrigin::Outbound)
    }
}

impl Future for Connecting {
    type Output = Result<Connection>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        Pin::new(&mut self.inner).poll(cx).map(|result| {
            result
                .map_err(anyhow::Error::from)
                .and_then(|connection| Connection::new(connection, self.origin))
                .map_err(|e| anyhow::anyhow!("failed establishing {} connection: {e}", self.origin))
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use futures::{future::join, io::AsyncReadExt};
    use std::time::Duration;

    #[tokio::test]
    async fn basic_endpoint() -> Result<()> {
        let _guard = crate::init_tracing_for_testing();

        let msg = b"hello";
        let config_1 = EndpointConfig::random("test");
        let endpoint_1 = Endpoint::new_with_address(config_1, "localhost:0")?;
        let peer_id_1 = endpoint_1.config.peer_id();

        println!("1: {}", endpoint_1.local_addr());

        let config_2 = EndpointConfig::random("test");
        let endpoint_2 = Endpoint::new_with_address(config_2, "localhost:0")?;
        let peer_id_2 = endpoint_2.config.peer_id();
        let addr_2 = endpoint_2.local_addr();
        println!("2: {}", endpoint_2.local_addr());

        let peer_1 = async move {
            let connection = endpoint_1.connect(addr_2.into()).unwrap().await.unwrap();
            assert_eq!(connection.peer_id(), peer_id_2);
            {
                let mut send_stream = connection.open_uni().await.unwrap();
                send_stream.write_all(msg).await.unwrap();
                send_stream.finish().await.unwrap();
            }
            endpoint_1.close();
            endpoint_1.inner.wait_idle().await;
            // Result::<()>::Ok(())
        };

        let peer_2 = async move {
            let connection = endpoint_2.accept().await.unwrap().await.unwrap();
            assert_eq!(connection.peer_id(), peer_id_1);

            let mut recv = connection.accept_uni().await.unwrap();
            let mut buf = Vec::new();
            AsyncReadExt::read_to_end(&mut recv, &mut buf)
                .await
                .unwrap();
            println!("from remote: {}", buf.escape_ascii());
            assert_eq!(buf, msg);
            endpoint_2.close();
            endpoint_2.inner.wait_idle().await;
            // Result::<()>::Ok(())
        };

        timeout(join(peer_1, peer_2)).await?;
        Ok(())
    }

    // Test to verify that multiple connections to the same endpoint can be open simultaneously.
    // While we don't currently allow for this, we may want to eventually enable/allow for it.
    #[tokio::test]
    async fn multiple_connections() -> Result<()> {
        let _guard = crate::init_tracing_for_testing();

        let msg = b"hello";
        let config_1 = EndpointConfig::random("test");
        let endpoint_1 = Endpoint::new_with_address(config_1, "localhost:0")?;
        let peer_id_1 = endpoint_1.config.peer_id();

        println!("1: {}", endpoint_1.local_addr());

        let config_2 = EndpointConfig::random("test");
        let endpoint_2 = Endpoint::new_with_address(config_2, "localhost:0")?;
        let peer_id_2 = endpoint_2.config.peer_id();
        let addr_2 = endpoint_2.local_addr();
        println!("2: {}", endpoint_2.local_addr());

        let peer_1 = async move {
            let connection_1 = endpoint_1.connect(addr_2.into()).unwrap().await.unwrap();
            assert_eq!(connection_1.peer_id(), peer_id_2);
            let connection_2 = endpoint_1.connect(addr_2.into()).unwrap().await.unwrap();
            assert_eq!(connection_2.peer_id(), peer_id_2);
            let req_1 = async {
                let mut send_stream = connection_2.open_uni().await.unwrap();
                send_stream.write_all(msg).await.unwrap();
                send_stream.finish().await.unwrap();
            };
            let req_2 = async {
                let mut send_stream = connection_1.open_uni().await.unwrap();
                send_stream.write_all(msg).await.unwrap();
                send_stream.finish().await.unwrap();
            };
            join(req_1, req_2).await;
            endpoint_1.close();
            endpoint_1.inner.wait_idle().await;
            // Result::<()>::Ok(())
        };

        let peer_2 = async move {
            let connection_1 = endpoint_2.accept().await.unwrap().await.unwrap();
            assert_eq!(connection_1.peer_id(), peer_id_1);

            let connection_2 = endpoint_2.accept().await.unwrap().await.unwrap();
            assert_eq!(connection_2.peer_id(), peer_id_1);
            assert_ne!(connection_1.stable_id(), connection_2.stable_id());

            println!("connection_1: {connection_1:#?}");
            println!("connection_2: {connection_2:#?}");

            let req_1 = async move {
                let mut recv = connection_1.accept_uni().await.unwrap();
                let mut buf = Vec::new();
                AsyncReadExt::read_to_end(&mut recv, &mut buf)
                    .await
                    .unwrap();
                println!("from remote: {}", buf.escape_ascii());
                assert_eq!(buf, msg);
            };
            let req_2 = async move {
                let mut recv = connection_2.accept_uni().await.unwrap();
                let mut buf = Vec::new();
                AsyncReadExt::read_to_end(&mut recv, &mut buf)
                    .await
                    .unwrap();
                println!("from remote: {}", buf.escape_ascii());
                assert_eq!(buf, msg);
            };

            join(req_1, req_2).await;
            endpoint_2.close();
            endpoint_2.inner.wait_idle().await;
            // Result::<()>::Ok(())
        };

        timeout(join(peer_1, peer_2)).await?;
        Ok(())
    }

    #[tokio::test]
    async fn peers_concurrently_finishing_uni_stream_before_accepting() -> Result<()> {
        let _guard = crate::init_tracing_for_testing();

        let msg = b"hello";
        let config_1 = EndpointConfig::random("test");
        let endpoint_1 = Endpoint::new_with_address(config_1, "localhost:0")?;

        let config_2 = EndpointConfig::random("test");
        let endpoint_2 = Endpoint::new_with_address(config_2, "localhost:0")?;
        let addr_2 = endpoint_2.local_addr();

        let (connection_1_to_2, connection_2_to_1) = timeout(join(
            async { endpoint_1.connect(addr_2.into()).unwrap().await.unwrap() },
            async { endpoint_2.accept().await.unwrap().await.unwrap() },
        ))
        .await
        .unwrap();

        // Send all the data
        {
            let mut send_stream = connection_1_to_2.open_uni().await.unwrap();
            send_stream.write_all(msg).await.unwrap();
            send_stream.finish().await.unwrap();
        }

        // Read it all
        {
            let mut recv = connection_2_to_1.accept_uni().await.unwrap();
            let mut buf = Vec::new();
            AsyncReadExt::read_to_end(&mut recv, &mut buf)
                .await
                .unwrap();
            assert_eq!(buf, msg);
        }

        Ok(())
    }

    async fn timeout<F: std::future::Future>(
        f: F,
    ) -> Result<F::Output, tokio::time::error::Elapsed> {
        tokio::time::timeout(Duration::from_millis(500), f).await
    }
}
