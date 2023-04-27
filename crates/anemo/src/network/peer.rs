use super::{
    wire::{network_message_frame_codec, read_response, write_request},
    OutboundRequestLayer,
};
use crate::{connection::Connection, Config, PeerId, Request, Response, Result};
use bytes::Bytes;
use futures::future::BoxFuture;
use quinn_proto::ConnectionStats;
use std::{sync::Arc, time::Duration};
use tokio_util::codec::{FramedRead, FramedWrite};
use tower::{Layer, Service, ServiceExt};

/// Handle to a connection with a remote Peer.
#[derive(Clone)]
pub struct Peer {
    connection: Connection,
    outbound_request_layer: OutboundRequestLayer,
    config: Arc<Config>,
}

impl Peer {
    pub(crate) fn new(
        connection: Connection,
        outbound_request_layer: OutboundRequestLayer,
        config: Arc<Config>,
    ) -> Self {
        Self {
            connection,
            outbound_request_layer,
            config,
        }
    }

    pub fn peer_id(&self) -> PeerId {
        self.connection.peer_id()
    }

    pub fn connection_stats(&self) -> ConnectionStats {
        self.connection.stats()
    }

    pub fn connection_rtt(&self) -> Duration {
        self.connection.rtt()
    }

    pub async fn rpc(&mut self, request: Request<Bytes>) -> Result<Response<Bytes>> {
        self.ready().await?.call(request).await
    }

    async fn do_rpc(&self, request: Request<Bytes>) -> Result<Response<Bytes>> {
        let (send_stream, recv_stream) = self.connection.open_bi().await?;
        let mut send_stream =
            FramedWrite::new(send_stream, network_message_frame_codec(&self.config));
        let mut recv_stream =
            FramedRead::new(recv_stream, network_message_frame_codec(&self.config));

        //
        // Write Request
        //

        write_request(&mut send_stream, request).await?;
        send_stream.get_mut().finish().await?;

        //
        // Read Response
        //

        let mut response = read_response(&mut recv_stream).await?;

        // Set the PeerId of this peer
        response.extensions_mut().insert(self.peer_id());

        Ok(response)
    }
}

impl Service<Request<Bytes>> for Peer {
    type Response = Response<Bytes>;
    type Error = crate::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    #[inline]
    fn poll_ready(
        &mut self,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    #[inline]
    fn call(&mut self, mut request: Request<Bytes>) -> Self::Future {
        // Provide Connection Metadata to middleware layers via extensions including:
        // * PeerId
        // * Direction of the Request
        request.extensions_mut().insert(self.peer_id());
        request.extensions_mut().insert(crate::Direction::Outbound);

        let peer = self.clone();
        let inner = tower::service_fn(move |request| {
            let peer = peer.clone();
            async move { peer.do_rpc(request).await }
        })
        .boxed();

        let mut service = self.outbound_request_layer.layer(inner);
        service.call(request)
    }
}
