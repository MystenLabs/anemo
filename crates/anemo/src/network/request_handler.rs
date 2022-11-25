use super::{
    wire::{network_message_frame_codec, read_request, write_response},
    ActivePeers,
};
use crate::{connection::Connection, endpoint::NewConnection, Request, Response, Result};
use bytes::Bytes;
use futures::{
    future::{select, Either},
    stream::{Fuse, FuturesUnordered},
    FutureExt, StreamExt,
};
use quinn::{Datagrams, IncomingBiStreams, IncomingUniStreams, RecvStream, SendStream};
use rand::{rngs::ThreadRng, Rng};
use std::{convert::Infallible, time::Duration};
use tokio::time::sleep;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tower::{util::BoxCloneService, ServiceExt};
use tracing::{debug, trace};

pub(crate) struct InboundRequestHandler {
    connection: Connection,
    incoming_bi: Fuse<IncomingBiStreams>,
    incoming_uni: Fuse<IncomingUniStreams>,
    incoming_datagrams: Fuse<Datagrams>,

    service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    active_peers: ActivePeers,
}

impl InboundRequestHandler {
    pub fn new(
        NewConnection {
            connection,
            uni_streams,
            bi_streams,
            datagrams,
        }: NewConnection,
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
        active_peers: ActivePeers,
    ) -> Self {
        Self {
            connection,
            incoming_uni: uni_streams.fuse(),
            incoming_bi: bi_streams.fuse(),
            incoming_datagrams: datagrams.fuse(),
            service,
            active_peers,
        }
    }

    pub async fn start(mut self) {
        debug!(peer =% self.connection.peer_id(), "InboundRequestHandler started");

        let mut inflight_requests = FuturesUnordered::new();
        let mut expiration_timer = Box::pin(
            sleep(Duration::from_secs(
                ThreadRng::default().gen_range(1800..=5400),
            ))
            .fuse(),
        );

        loop {
            futures::select! {
                // anemo does not currently use uni streams so we can
                // just ignore and drop the stream
                uni = self.incoming_uni.select_next_some() => {
                    match uni {
                        Ok(recv_stream) => trace!("incoming uni stream! {}", recv_stream.id()),
                        Err(e) => {
                            trace!("error listening for incoming uni streams: {e}");
                            break;
                        }
                    }
                },
                bi = self.incoming_bi.next() => {
                    if bi.is_none() {
                        // Nothing to be polled next, connection has been closed
                        debug!(peer =% self.connection.peer_id(), "Connection has been closed locally, now exit");
                        break;
                    }
                    match bi.unwrap() {
                        Ok((bi_tx, bi_rx)) => {
                            trace!("incoming bi stream! {}", bi_tx.id());
                            let request_handler =
                                BiStreamRequestHandler::new(self.connection.clone(), self.service.clone(), bi_tx, bi_rx);
                            inflight_requests.push(request_handler.handle());
                        }
                        Err(e) => {
                            trace!("error listening for incoming bi streams: {e}");
                            break;
                        }
                    }
                },
                // anemo does not currently use datagrams so we can
                // just ignore them
                datagram = self.incoming_datagrams.select_next_some() => {
                    match datagram {
                        Ok(datagram) => trace!("incoming datagram of length: {}", datagram.len()),
                        Err(e) => {
                            trace!("error listening for datagrams: {e}");
                            break;
                        }
                    }
                },
                () = inflight_requests.select_next_some() => {},
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
            match select(handler, stopped).await {
                Either::Left((response, _)) => response.expect("Infallible"),
                Either::Right(_) => return Err(anyhow::anyhow!("send_stream closed by remote")),
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
