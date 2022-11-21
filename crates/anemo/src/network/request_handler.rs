use super::{
    wire::{network_message_frame_codec, read_request, write_response},
    ActivePeers,
};
use crate::{
    connection::{Connection, SendStream},
    Request, Response, Result,
};
use bytes::Bytes;
use quinn::RecvStream;
use std::convert::Infallible;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tower::{util::BoxCloneService, ServiceExt};
use tracing::{debug, trace};

/// Manages incoming requests from a peer.
///
/// Currently only bi-directional streams are supported. Whenever a new request (stream) is
/// received a new task is spawn to handle it.
pub(crate) struct InboundRequestHandler {
    connection: Connection,

    service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    active_peers: ActivePeers,
}

impl InboundRequestHandler {
    pub fn new(
        connection: Connection,
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
        active_peers: ActivePeers,
    ) -> Self {
        Self {
            connection,
            service,
            active_peers,
        }
    }

    pub async fn start(self) {
        debug!(peer =% self.connection.peer_id(), "InboundRequestHandler started");

        let mut inflight_requests = tokio::task::JoinSet::new();

        // TODO: find the source of the memory growth and remove the expiration of connections.
        //
        // After some experimentation we've found that cycling quinn connections seems to prevent
        // unbounded memory retention/growth. This potentially means there may be some unexplained
        // memory retention/growth issues inside of the quinn connection itself. Until we're able
        // to root-cause the source of the memory problems, we'll work around the problem by
        // putting an upper bound on how long a connection is held on to: between 30-90 minutes.
        let mut expiration_timer = Box::pin(tokio::time::sleep(std::time::Duration::from_secs(
            rand::Rng::gen_range(&mut rand::thread_rng(), 1800..=5400),
        )));

        loop {
            tokio::select! {
                // anemo does not currently use uni streams so we can
                // just ignore and drop the stream
                uni = self.connection.accept_uni() => {
                    match uni {
                        Ok(recv_stream) => trace!("incoming uni stream! {}", recv_stream.id()),
                        Err(e) => {
                            trace!("error listening for incoming uni streams: {e}");
                            break;
                        }
                    }
                },
                bi = self.connection.accept_bi() => {
                    match bi {
                        Ok((bi_tx, bi_rx)) => {
                            trace!("incoming bi stream! {}", bi_tx.id());
                            let request_handler =
                                BiStreamRequestHandler::new(self.connection.clone(), self.service.clone(), bi_tx, bi_rx);
                            inflight_requests.spawn(request_handler.handle());
                        }
                        Err(e) => {
                            trace!("error listening for incoming bi streams: {e}");
                            break;
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
                            break;
                        }
                    }
                },
                Some(completed_request) = inflight_requests.join_next() => {
                    // If a task panics, just propagate it
                    completed_request.unwrap();
                },
                () = &mut expiration_timer => {
                    debug!(peer =% self.connection.peer_id(), "Shutting down at expiration");
                    self.connection.close();
                    break;
                }
            }
        }

        self.active_peers.remove_with_stable_id(
            self.connection.peer_id(),
            self.connection.stable_id(),
            crate::types::DisconnectReason::ConnectionLost,
        );

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
        connection: Connection,
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
        send_stream: SendStream,
        recv_stream: RecvStream,
    ) -> Self {
        Self {
            connection,
            service,
            send_stream: FramedWrite::new(send_stream, network_message_frame_codec()),
            recv_stream: FramedRead::new(recv_stream, network_message_frame_codec()),
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
        // Provide Connection Metadata to the handler via extentions including:
        // * PeerId
        // * ConnectionOrigin
        // * Remote SocketAddr
        request.extensions_mut().insert(self.connection.peer_id());
        request.extensions_mut().insert(self.connection.origin());
        request
            .extensions_mut()
            .insert(self.connection.remote_address());

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
