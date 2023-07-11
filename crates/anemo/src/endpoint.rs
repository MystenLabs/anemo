use crate::config::ClientConfig;
use crate::connection::ConnectionInner;
use crate::{
    config::EndpointConfig, connection::Connection, types::Address, ConnectionOrigin, PeerId,
    Result,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{trace, warn};

enum Transport {
    Quic(quinn::Endpoint),
    Tls(anemo_tls::Endpoint),
}

impl Transport {
    async fn wait_idle(&self) {
        match self {
            Transport::Quic(inner) => inner.wait_idle().await,
            Transport::Tls(_inner) => (), // TODO: add wait_idle
        }
    }

    fn drop_socket(&self) -> std::io::Result<()> {
        match self {
            Transport::Quic(inner) => {
                let socket = std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
                inner.rebind(socket)
            }
            Transport::Tls(_inner) => unimplemented!(),
        }
    }
}

/// A transport endpoint.
///
/// An endpoint may host many connections, and may act as both client and server for different
/// connections.
pub(crate) struct Endpoint {
    config: EndpointConfig,
    local_addr: SocketAddr,
    transport: Transport,
}

impl Endpoint {
    pub fn new_quic(config: EndpointConfig, socket: std::net::UdpSocket) -> Result<Self> {
        let local_addr = socket.local_addr()?;
        let server_config = config.server_config().clone();
        let endpoint = quinn::Endpoint::new(
            config.transport_endpoint_config().as_quic().clone(),
            Some(server_config.try_quic()?.clone()),
            socket,
            Arc::new(quinn::TokioRuntime),
        )?;

        let endpoint = Self {
            config,
            local_addr,
            transport: Transport::Quic(endpoint),
        };

        Ok(endpoint)
    }

    /// WARNING: TLS support is unstable, experimental, and incomplete.
    pub fn new_tls(config: EndpointConfig, listener: tokio::net::TcpListener) -> Result<Self> {
        let local_addr = listener.local_addr()?;
        let endpoint =
            anemo_tls::Endpoint::new(config.server_config().try_tls()?.clone(), listener);
        Ok(Self {
            config,
            local_addr,
            transport: Transport::Tls(endpoint),
        })
    }

    pub async fn connect(&self, address: Address) -> Result<Connection> {
        self.connect_with_client_config(self.config.client_config().clone(), address)
            .await
    }

    pub async fn connect_with_expected_peer_id(
        &self,
        address: Address,
        peer_id: PeerId,
    ) -> Result<Connection> {
        let config = self
            .config
            .client_config_with_expected_server_identity(peer_id);
        self.connect_with_client_config(config, address).await
    }

    async fn connect_with_client_config(
        &self,
        config: ClientConfig,
        address: Address,
    ) -> Result<Connection> {
        let addr = address.resolve()?;
        match self.transport {
            Transport::Quic(ref inner) => inner
                .connect_with(
                    config.try_quic()?.to_owned(),
                    addr,
                    self.config.server_name(),
                )?
                .await
                .map_err(anyhow::Error::from)
                .and_then(|connection| {
                    Connection::new(
                        ConnectionInner::Quic(connection),
                        ConnectionOrigin::Outbound,
                    )
                }),
            Transport::Tls(ref inner) => {
                inner
                    .connect_with(
                        config.try_tls()?.to_owned(),
                        addr,
                        self.config.server_name(),
                    )
                    .await
                    .map_err(anyhow::Error::from)
                    .and_then(|connection| {
                        Connection::new(
                            ConnectionInner::Tls(connection),
                            ConnectionOrigin::Outbound,
                        )
                    })
            }
            .map_err(|e| {
                anyhow::anyhow!(
                    "failed establishing {} connection: {e}",
                    ConnectionOrigin::Outbound
                )
            }),
        }
    }

    /// Returns the socket address that this Endpoint is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
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
        match self.transport {
            Transport::Quic(ref inner) => inner.close(0_u32.into(), b"endpoint closed"),
            // TODO: add close for TLS.
            Transport::Tls(ref _inner) => (),
        }
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
        if timeout(max_timeout, self.transport.wait_idle())
            .await
            .is_err()
        {
            warn!(
                "Max timeout reached {}s while waiting for connections clean shutdown",
                max_timeout.as_secs_f64()
            );
        }
    }

    /// Ensures that the underlying socket we're bound to is dropped and immediately able to be
    /// rebound to once this function exits.
    ///
    /// Behavior of the endpoint is undefined after this function is called.
    pub fn drop_socket(&self) -> std::io::Result<()> {
        self.transport.drop_socket()
    }

    /// Get the next incoming connection attempt from a client, or
    /// `None` if the endpoint is [`close`](Self::close)d.
    pub(crate) async fn accept(&self) -> Option<Result<Connection>> {
        match self.transport {
            Transport::Quic(ref inner) => {
                let connecting = inner.accept().await;
                if let Some(connecting) = connecting {
                    Some(
                        connecting
                            .await
                            .map_err(anyhow::Error::from)
                            .and_then(|connection| {
                                Connection::new(
                                    ConnectionInner::Quic(connection),
                                    ConnectionOrigin::Inbound,
                                )
                            }),
                    )
                } else {
                    None
                }
            }
            Transport::Tls(ref inner) => Some(
                inner
                    .accept()
                    .await
                    .map_err(anyhow::Error::from)
                    .and_then(|connection| {
                        Connection::new(ConnectionInner::Tls(connection), ConnectionOrigin::Inbound)
                    }),
            ),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use futures::{future::join, io::AsyncReadExt};
    use rstest::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    #[fixture]
    async fn quic_endpoint() -> Endpoint {
        let config = EndpointConfig::random_quic("test");
        let socket = std::net::UdpSocket::bind("localhost:0").unwrap();
        Endpoint::new_quic(config, socket).unwrap()
    }

    #[fixture]
    async fn tls_endpoint() -> Endpoint {
        let config = EndpointConfig::random_tls("test");
        let listener = tokio::net::TcpListener::bind("localhost:0").await.unwrap();
        Endpoint::new_tls(config, listener).unwrap()
    }

    #[rstest]
    #[case::quic(quic_endpoint(), quic_endpoint())]
    #[case::tls(tls_endpoint(), tls_endpoint())]
    #[awt]
    #[tokio::test]
    async fn basic_endpoint(
        #[future]
        #[case]
        endpoint_1: Endpoint,
        #[future]
        #[case]
        endpoint_2: Endpoint,
    ) -> Result<()> {
        let _guard = crate::init_tracing_for_testing();

        let msg = b"hello";
        let peer_id_1 = endpoint_1.config.peer_id();

        println!("1: {}", endpoint_1.local_addr());

        let peer_id_2 = endpoint_2.config.peer_id();
        let addr_2 = endpoint_2.local_addr();
        println!("2: {}", endpoint_2.local_addr());

        let peer_1 = async move {
            let connection = endpoint_1.connect(addr_2.into()).await.unwrap();
            assert_eq!(connection.peer_id(), peer_id_2);
            {
                let mut send_stream = connection.open_uni().await.unwrap();
                send_stream.write_all(msg).await.unwrap();
                send_stream.finish().await.unwrap();
            }
            endpoint_1.close();
            endpoint_1.transport.wait_idle().await;
            // Result::<()>::Ok(())
        };

        let peer_2 = async move {
            let connection = endpoint_2.accept().await.unwrap().unwrap();
            assert_eq!(connection.peer_id(), peer_id_1);

            let mut recv = connection.accept_uni().await.unwrap();
            let mut buf = Vec::new();
            AsyncReadExt::read_to_end(&mut recv, &mut buf)
                .await
                .unwrap();
            println!("from remote: {}", buf.escape_ascii());
            assert_eq!(buf, msg);
            endpoint_2.close();
            endpoint_2.transport.wait_idle().await;
            // Result::<()>::Ok(())
        };

        timeout(join(peer_1, peer_2)).await?;
        Ok(())
    }

    // Test to verify that multiple connections to the same endpoint can be open simultaneously.
    // While we don't currently allow for this, we may want to eventually enable/allow for it.
    #[rstest]
    #[case::quic(quic_endpoint(), quic_endpoint())]
    #[case::tls(tls_endpoint(), tls_endpoint())]
    #[awt]
    #[tokio::test]
    async fn multiple_connections(
        #[future]
        #[case]
        endpoint_1: Endpoint,
        #[future]
        #[case]
        endpoint_2: Endpoint,
    ) -> Result<()> {
        let _guard = crate::init_tracing_for_testing();

        let msg = b"hello";
        let peer_id_1 = endpoint_1.config.peer_id();

        println!("1: {}", endpoint_1.local_addr());

        let peer_id_2 = endpoint_2.config.peer_id();
        let addr_2 = endpoint_2.local_addr();
        println!("2: {}", endpoint_2.local_addr());

        let peer_1 = async move {
            let connection_1 = endpoint_1.connect(addr_2.into()).await.unwrap();
            assert_eq!(connection_1.peer_id(), peer_id_2);
            let connection_2 = endpoint_1.connect(addr_2.into()).await.unwrap();
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
            endpoint_1.transport.wait_idle().await;
            // Result::<()>::Ok(())
        };

        let peer_2 = async move {
            let connection_1 = endpoint_2.accept().await.unwrap().unwrap();
            assert_eq!(connection_1.peer_id(), peer_id_1);

            let connection_2 = endpoint_2.accept().await.unwrap().unwrap();
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
            endpoint_2.transport.wait_idle().await;
            // Result::<()>::Ok(())
        };

        timeout(join(peer_1, peer_2)).await?;
        Ok(())
    }

    #[rstest]
    #[case::quic(quic_endpoint(), quic_endpoint())]
    #[case::tls(tls_endpoint(), tls_endpoint())]
    #[awt]
    #[tokio::test]
    async fn peers_concurrently_finishing_uni_stream_before_accepting(
        #[future]
        #[case]
        endpoint_1: Endpoint,
        #[future]
        #[case]
        endpoint_2: Endpoint,
    ) -> Result<()> {
        let _guard = crate::init_tracing_for_testing();

        let msg = b"hello";
        let addr_2 = endpoint_2.local_addr();

        let (connection_1_to_2, connection_2_to_1) = timeout(join(
            async { endpoint_1.connect(addr_2.into()).await.unwrap() },
            async { endpoint_2.accept().await.unwrap().unwrap() },
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
