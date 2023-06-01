use crate::{
    config::EndpointConfig,
    endpoint::Endpoint,
    middleware::{add_extension::AddExtensionLayer, timeout},
    types::{Address, DisconnectReason, PeerEvent},
    Config, PeerId, Request, Response, Result,
};
use anyhow::anyhow;
use bytes::Bytes;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::ToSocketAddrs;
use std::{convert::Infallible, net::SocketAddr, sync::Arc};
use tokio::sync::{broadcast, mpsc, oneshot};
use tower::{
    util::{BoxLayer, BoxService},
    Layer, Service, ServiceBuilder, ServiceExt,
};
use tracing::warn;

mod connection_manager;
pub use connection_manager::KnownPeers;
use connection_manager::{
    ActivePeers, ActivePeersRef, ConnectionManager, ConnectionManagerRequest,
};

mod peer;
pub use peer::Peer;

mod request_handler;
mod wire;

#[cfg(test)]
mod tests;

type OutboundRequestLayer = BoxLayer<
    BoxService<Request<Bytes>, Response<Bytes>, crate::Error>,
    Request<Bytes>,
    Response<Bytes>,
    crate::Error,
>;

/// A builder for a [`Network`].
pub struct Builder {
    bind_address: Address,
    config: Option<Config>,
    server_name: Option<String>,
    alternate_server_name: Option<String>,

    /// Ed25519 Private Key
    private_key: Option<[u8; 32]>,

    /// Layer to apply to all outbound requests
    outbound_request_layer: Option<OutboundRequestLayer>,
}

impl Builder {
    /// Set the [`Config`] that this network should use.
    pub fn config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    /// Set the `server-name` that will be used when constructing a self-signed X.509 certificate to
    /// be used in the TLS handshake.
    ///
    /// Traditionally a server name is intended to be the DNS name that is being dialed, although
    /// since Anemo does not require parties to use DNS names, nor does it rely on a central CA,
    /// `server-name` is instead used to identify the network name that this Peer can connect to.
    /// In other words, The TLS handshake will only be successful if all parties use the same
    /// `server-name`.
    pub fn server_name<T: Into<String>>(mut self, server_name: T) -> Self {
        self.server_name = Some(server_name.into());
        self
    }

    /// `alternate-server-name` helps with network name migration.
    /// Server accepts connections from both `server-name` and `alternate-server-name`.
    /// However client only initiates connection with `server-name`.
    pub fn alternate_server_name<T: Into<String>>(mut self, server_name: T) -> Self {
        self.alternate_server_name = Some(server_name.into());
        self
    }

    /// Set the Ed25519 Private Key that will be used to perform the TLS handshake.
    /// The corresponding Public Key will be this node's [`PeerId`].
    pub fn private_key(mut self, private_key: [u8; 32]) -> Self {
        self.private_key = Some(private_key);
        self
    }

    #[cfg(test)]
    pub(crate) fn random_private_key(self) -> Self {
        let mut rng = rand::thread_rng();
        let mut bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rng, &mut bytes[..]);

        self.private_key(bytes)
    }

    /// Provide an optional [`Layer`] that will be used to wrap all outbound RPCs.
    ///
    /// This could be helpful in providing global metrics and logging for all outbound requests
    pub fn outbound_request_layer<L>(mut self, layer: L) -> Self
    where
        L: Layer<BoxService<Request<Bytes>, Response<Bytes>, crate::Error>> + Send + Sync + 'static,
        L::Service: Service<Request<Bytes>, Response = Response<Bytes>, Error = crate::Error>
            + Send
            + 'static,
        <L::Service as Service<Request<Bytes>>>::Future: Send + 'static,
    {
        let layer = BoxLayer::new(layer);
        self.outbound_request_layer = Some(layer);
        self
    }

    /// Start a [`Network`] and return a handle to it.
    ///
    /// # Panics
    ///
    /// This method will panic if:
    /// * not called from within the context of a tokio runtime.
    /// * no `private-key` or `server-name` were set.
    pub fn start<T>(mut self, service: T) -> Result<Network>
    where
        T: Clone + Send + 'static,
        T: Service<Request<Bytes>, Response = Response<Bytes>, Error = Infallible>,
        <T as Service<Request<Bytes>>>::Future: Send + 'static,
    {
        let config = self.config.unwrap_or_default();
        let quic_config = config.quic.clone().unwrap_or_default();
        let primary_server_name = self.server_name.unwrap();
        let alternate_server_name = self.alternate_server_name;
        let private_key = self.private_key.unwrap();

        let endpoint_config = EndpointConfig::builder()
            .transport_config(config.transport_config())
            .server_name(primary_server_name)
            .alternate_server_name(alternate_server_name)
            .private_key(private_key)
            .build()?;

        let addrs: Vec<_> = self.bind_address.to_socket_addrs()?.collect();
        let socket = (|| {
            let mut result = Err(anyhow!("no addresses to bind to"));
            for addr in addrs.iter() {
                let socket =
                    Socket::new(Domain::for_address(*addr), Type::DGRAM, Some(Protocol::UDP))?;
                result = socket
                    .bind(&socket2::SockAddr::from(*addr))
                    .map_err(|e| e.into());
                if let Ok(()) = result {
                    return Ok(socket);
                }
            }
            Err(result.unwrap_err())
        })()?;
        let socket_send_buf_size = if let Some(send_buffer_size) =
            quic_config.socket_send_buffer_size
        {
            let result = socket.set_send_buffer_size(send_buffer_size);
            if let Err(e) = result {
                if quic_config.allow_failed_socket_buffer_size_setting {
                    warn!("failed to set socket send buffer size to {send_buffer_size}: {e}",);
                } else {
                    return Err(e.into());
                }
            }
            let buf_size = socket.send_buffer_size()?;
            if buf_size < send_buffer_size {
                // Linux doubles requested size, so allow anything greater.
                let msg = format!(
                    "expected socket send buffer size to be at least {send_buffer_size}, got {buf_size}"
                );
                if quic_config.allow_failed_socket_buffer_size_setting {
                    warn!(msg);
                } else {
                    return Err(anyhow!(msg));
                }
            }
            buf_size
        } else {
            socket.send_buffer_size()?
        };
        let socket_receive_buf_size = if let Some(receive_buffer_size) =
            quic_config.socket_receive_buffer_size
        {
            let result = socket.set_recv_buffer_size(receive_buffer_size);
            if let Err(e) = result {
                if quic_config.allow_failed_socket_buffer_size_setting {
                    warn!("failed to set socket receive buffer size to {receive_buffer_size}: {e}",);
                } else {
                    return Err(e.into());
                }
            }
            let buf_size = socket.recv_buffer_size()?;
            if buf_size < receive_buffer_size {
                // Linux doubles requested size, so allow anything greater.
                let msg = format!(
                    "expected socket receive buffer size to be at least {receive_buffer_size}, got {buf_size}",
                );
                if quic_config.allow_failed_socket_buffer_size_setting {
                    warn!(msg);
                } else {
                    return Err(anyhow!(msg));
                }
            }
            buf_size
        } else {
            socket.recv_buffer_size()?
        };

        let endpoint = Endpoint::new(endpoint_config, socket.into())?;

        let config = Arc::new(config);
        let endpoint = Arc::new(endpoint);
        let active_peers = ActivePeers::new(config.peer_event_broadcast_channel_capacity());
        let active_peers_ref = active_peers.downgrade();
        let known_peers = KnownPeers::new();

        // Build the Outbound Request Layer
        let outbound_request_layer = {
            let builder = ServiceBuilder::new()
                // Support timeouts for outbound requests
                .layer(timeout::outbound::TimeoutLayer::new(
                    config.outbound_request_timeout(),
                ));
            if let Some(layer) = self.outbound_request_layer.take() {
                BoxLayer::new(builder.layer(layer).into_inner())
            } else {
                BoxLayer::new(builder.into_inner())
            }
        };

        let inner = Arc::new_cyclic(|weak| {
            let service = ServiceBuilder::new()
                // Support timeouts for inbound requests
                .layer(timeout::inbound::TimeoutLayer::new(
                    config.inbound_request_timeout(),
                ))
                // Supply a weak reference to the network via an Extension
                .layer(AddExtensionLayer::new(NetworkRef(weak.clone())))
                .service(service)
                .boxed_clone();

            let (connection_manager, connection_manager_handle) = ConnectionManager::new(
                config.clone(),
                endpoint.clone(),
                active_peers,
                known_peers.clone(),
                service,
            );

            tokio::spawn(connection_manager.start());

            NetworkInner {
                config,
                endpoint,
                active_peers: active_peers_ref,
                known_peers,
                connection_manager_handle,
                outbound_request_layer,
                socket_send_buf_size,
                socket_receive_buf_size,
            }
        });

        Ok(Network(inner))
    }
}

/// Handle to a network.
///
/// This handle can be cheaply cloned and shared across many threads.
/// Once all handles have been dropped the network will gracefully shutdown.
#[derive(Clone)]
pub struct Network(Arc<NetworkInner>);

impl Network {
    /// Binds to the provided address, and returns a [`Builder`].
    pub fn bind<A: Into<Address>>(addr: A) -> Builder {
        Builder {
            bind_address: addr.into(),
            config: None,
            server_name: None,
            alternate_server_name: None,
            private_key: None,
            outbound_request_layer: None,
        }
    }

    pub fn peers(&self) -> Vec<PeerId> {
        self.0.peers()
    }

    pub fn subscribe(&self) -> Result<(broadcast::Receiver<PeerEvent>, Vec<PeerId>)> {
        self.0
            .active_peers
            .upgrade()
            .as_ref()
            .map(ActivePeers::subscribe)
            .ok_or_else(|| anyhow!("network has been shutdown"))
    }

    pub fn peer(&self, peer_id: PeerId) -> Option<Peer> {
        self.0.peer(peer_id)
    }

    pub fn known_peers(&self) -> &KnownPeers {
        self.0.known_peers()
    }

    pub async fn connect<A: Into<Address>>(&self, addr: A) -> Result<PeerId> {
        self.0.connect(addr.into(), None).await
    }

    pub async fn connect_with_peer_id<A: Into<Address>>(
        &self,
        addr: A,
        peer_id: PeerId,
    ) -> Result<PeerId> {
        self.0.connect(addr.into(), Some(peer_id)).await
    }

    pub fn disconnect(&self, peer: PeerId) -> Result<()> {
        self.0.disconnect(peer)
    }

    pub async fn rpc(&self, peer: PeerId, request: Request<Bytes>) -> Result<Response<Bytes>> {
        self.0.rpc(peer, request).await
    }

    /// Return the local address that this Network is listening on.
    pub fn local_addr(&self) -> SocketAddr {
        self.0.local_addr()
    }

    /// Return the [`PeerId`] of this Network.
    pub fn peer_id(&self) -> PeerId {
        self.0.peer_id()
    }

    pub fn downgrade(&self) -> NetworkRef {
        NetworkRef(Arc::downgrade(&self.0))
    }

    /// Shutdown the Network.
    pub async fn shutdown(&self) -> Result<()> {
        self.0.shutdown().await
    }

    /// Returns true if the network has been shutdown.
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    /// Returns the size of the network's UDP socket send buffer.
    pub fn socket_send_buf_size(&self) -> usize {
        self.0.socket_send_buf_size()
    }

    /// Returns the size of the network's UDP socket receive buffer.
    pub fn socket_receive_buf_size(&self) -> usize {
        self.0.socket_receive_buf_size()
    }
}

struct NetworkInner {
    config: Arc<Config>,
    endpoint: Arc<Endpoint>,
    active_peers: ActivePeersRef,
    known_peers: KnownPeers,
    connection_manager_handle: mpsc::Sender<ConnectionManagerRequest>,

    outbound_request_layer: OutboundRequestLayer,

    socket_send_buf_size: usize,
    socket_receive_buf_size: usize,
}

impl NetworkInner {
    fn peers(&self) -> Vec<PeerId> {
        self.active_peers
            .upgrade()
            .as_ref()
            .map(ActivePeers::peers)
            .unwrap_or_default()
    }

    fn known_peers(&self) -> &KnownPeers {
        &self.known_peers
    }

    /// Returns the socket address that this Network is listening on
    fn local_addr(&self) -> SocketAddr {
        self.endpoint.local_addr()
    }

    fn peer_id(&self) -> PeerId {
        self.endpoint.peer_id()
    }

    async fn connect(&self, addr: Address, peer_id: Option<PeerId>) -> Result<PeerId> {
        let (sender, receiver) = oneshot::channel();
        self.connection_manager_handle
            .send(ConnectionManagerRequest::ConnectRequest(
                addr, peer_id, sender,
            ))
            .await
            .map_err(|_| anyhow!("network has been shutdown"))?;
        receiver.await?
    }

    fn disconnect(&self, peer_id: PeerId) -> Result<()> {
        let active_peers = self
            .active_peers
            .upgrade()
            .ok_or_else(|| anyhow!("network has been shutdown"))?;
        active_peers.remove(&peer_id, DisconnectReason::Requested);
        Ok(())
    }

    fn peer(&self, peer_id: PeerId) -> Option<Peer> {
        let active_peers = self.active_peers.upgrade()?;
        let connection = active_peers.get(&peer_id)?;
        Some(Peer::new(
            connection,
            self.outbound_request_layer.clone(),
            self.config.clone(),
        ))
    }

    async fn rpc(&self, peer_id: PeerId, request: Request<Bytes>) -> Result<Response<Bytes>> {
        self.peer(peer_id)
            .ok_or_else(|| anyhow!("not connected to peer {peer_id}"))?
            .rpc(request)
            .await
    }

    async fn shutdown(&self) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        self.connection_manager_handle
            .send(ConnectionManagerRequest::Shutdown(sender))
            .await
            .map_err(|_| anyhow!("network has been shutdown"))?;
        receiver.await.map_err(Into::into)
    }

    /// Returns true if the network has been shutdown.
    fn is_closed(&self) -> bool {
        self.connection_manager_handle.is_closed()
    }

    fn socket_send_buf_size(&self) -> usize {
        self.socket_send_buf_size
    }

    fn socket_receive_buf_size(&self) -> usize {
        self.socket_receive_buf_size
    }
}

/// Weak reference to a [`Network`] handle.
///
/// A `NetworkRef` can be obtained either via [`Network::downgrade`] if you have direct access to a
/// [`Network`] or via the [request extensions] if needed inside of a request handler.
///
/// ## Note
///
/// Care should be taken to avoid holding on to an upgraded pointer for too long if done from
/// inside a request handler as holding a Network can prevent the network from properly shutting
/// down once all other references to the Network, outside the handlers, have been dropped.
///
/// [request extensions]: crate::types::Extensions
#[derive(Clone)]
pub struct NetworkRef(std::sync::Weak<NetworkInner>);

impl NetworkRef {
    /// Attempts to upgrade this weak reference to a [`Network`] handle, delaying the dropping of
    /// the network if successful.
    ///
    /// Returns [`None`] if the network has already been dropped or shutdown.
    pub fn upgrade(&self) -> Option<Network> {
        self.0
            .upgrade()
            .map(Network)
            .and_then(|network| (!network.0.is_closed()).then_some(network))
    }
}
