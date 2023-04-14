use super::{
    wire::{network_message_frame_codec, read_request, write_response},
    ActivePeers,
};
use crate::{
    connection::{Connection, SendStream},
    Config, Request, Response, Result,
};
use bytes::Bytes;
use quinn::RecvStream;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tower::{util::BoxCloneService, ServiceExt};
use tracing::{debug, trace};

/// Manages incoming requests from a peer.
///
/// Currently only bi-directional streams are supported. Whenever a new request (stream) is
/// received a new task is spawn to handle it.
pub(crate) struct InboundRequestHandler {
    config: Arc<Config>,
    connection: Connection,

    service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    active_peers: ActivePeers,
}

impl InboundRequestHandler {
    pub fn new(
        config: Arc<Config>,
        connection: Connection,
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
        active_peers: ActivePeers,
    ) -> Self {
        Self {
            config,
            connection,
            service,
            active_peers,
        }
    }

    pub async fn start(self) {
        debug!(peer =% self.connection.peer_id(), "InboundRequestHandler started");

        let mut inflight_requests = tokio::task::JoinSet::new();

        let close_reason = loop {
            tokio::select! {
                // anemo does not currently use uni streams so we can
                // just ignore and drop the stream
                uni = self.connection.accept_uni() => {
                    match uni {
                        Ok(recv_stream) => trace!("incoming uni stream! {}", recv_stream.id()),
                        Err(e) => {
                            trace!("error listening for incoming uni streams: {e}");
                            break e;
                        }
                    }
                },
                bi = self.connection.accept_bi() => {
                    match bi {
                        Ok((bi_tx, bi_rx)) => {
                            trace!("incoming bi stream! {}", bi_tx.id());
                            let request_handler =
                                BiStreamRequestHandler::new(&self.config, self.connection.clone(), self.service.clone(), bi_tx, bi_rx);
                            inflight_requests.spawn(request_handler.handle());
                        }
                        Err(e) => {
                            trace!("error listening for incoming bi streams: {e}");
                            break e;
                        }
                    }
                },
                // anemo does not currently use datagrams so we can
                // just ignore them
                datagram = self.connection.read_datagram() => {
                    match datagram {
                        Ok(datagram) => trace!("incoming datagram of length: {}", datagram.len()),
                        Err(e) => {
                            trace!("error listening for datagrams: {e}");
                            break e;
                        }
                    }
                },
                Some(completed_request) = inflight_requests.join_next() => {
                    match completed_request {
                        Ok(()) => {
                            trace!("request handler task completed");
                        },
                        Err(e) => {
                            if e.is_cancelled() {
                                trace!("request handler task was cancelled");
                            } else if e.is_panic() {
                                // If a task panics, just propagate it
                                std::panic::resume_unwind(e.into_panic());
                            } else {
                                panic!("request handler task failed: {e}");
                            }
                        },
                    };
                },
            }
        };

        self.active_peers.remove_with_stable_id(
            self.connection.peer_id(),
            self.connection.stable_id(),
            crate::types::DisconnectReason::from_quinn_error(&close_reason),
        );

        inflight_requests.shutdown().await;

        debug!(peer =% self.connection.peer_id(), "InboundRequestHandler ended");
    }
}

/// Handles a single incoming request from a peer. It receives the request, forwards it
/// to the service for processing and the sends back to peer the response.
struct BiStreamRequestHandler {
    connection: Connection,
    service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    send_stream: FramedWrite<SendStream, LengthDelimitedCodec>,
    recv_stream: FramedRead<RecvStream, LengthDelimitedCodec>,
}

impl BiStreamRequestHandler {
    fn new(
        config: &Config,
        connection: Connection,
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
        send_stream: SendStream,
        recv_stream: RecvStream,
    ) -> Self {
        Self {
            connection,
            service,
            send_stream: FramedWrite::new(send_stream, network_message_frame_codec(config)),
            recv_stream: FramedRead::new(recv_stream, network_message_frame_codec(config)),
        }
    }

    async fn handle(self) {
        if let Err(e) = self.do_handle().await {
            trace!("handling request failed: {e}");
        }
    }

    async fn do_handle(mut self) -> Result<()> {
        //
        // Read Request
        //

        let mut request = read_request(&mut self.recv_stream).await?;

        // TODO maybe provide all of this via a single ConnectionMetadata type
        //
        // Provide Connection Metadata to the handler via extensions including:
        // * PeerId
        // * ConnectionOrigin
        // * Remote SocketAddr
        // * Direction of the Request
        request.extensions_mut().insert(self.connection.peer_id());
        request.extensions_mut().insert(self.connection.origin());
        request
            .extensions_mut()
            .insert(self.connection.remote_address());
        request.extensions_mut().insert(crate::Direction::Inbound);

        // Issue request to configured Service
        // We also watch the send_stream and see if it has been prematurely terminated by the
        // remote side indicating that this RPC was canceled.
        let response = {
            let handler = self.service.oneshot(request);
            let stopped = self.send_stream.get_mut().stopped();
            tokio::select! {
                response = handler => response.expect("Infallible"),
                _ = stopped => return Err(anyhow::anyhow!("send_stream closed by remote")),
            }
        };

        //
        // Write Response
        //

        write_response(&mut self.send_stream, response).await?;
        self.send_stream.get_mut().finish().await?;

        Ok(())
    }
}
